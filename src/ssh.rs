use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Result};

use crate::config;
use crate::types::Barn;

const CONNECT_TIMEOUT_SECS: u32 = 10;
const CONTROL_PERSIST: &str = "10m";

/// How often ssh asks the far end whether it is still there, and how many
/// unanswered asks end the connection.
///
/// Hoisted out of [`ssh_args`]'s literal list so [`tunnel_args`] uses the same
/// numbers rather than a second opinion about them. Without keepalives a
/// connection whose network went away does not fail, it *hangs* — the local end
/// has nothing to notice, so it sits with the channel open forever. For a probe
/// that is a stuck TUI; for the reverse tunnel it is worse, because a hung
/// tunnel is indistinguishable from a working one and nothing ever reconnects
/// it. 15 × 3 is 45 seconds to a verdict.
pub const SERVER_ALIVE_INTERVAL_SECS: u32 = 15;
pub const SERVER_ALIVE_COUNT_MAX: u32 = 3;

/// Per-call SSH behavior. Defaults are the safe interactive case.
#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// Fail instead of prompting for a password or passphrase. Use for probes
    /// and any call made from the TUI, never for an interactive attach.
    pub batch: bool,
    /// Allocate a remote TTY (`-t`). Required for anything that renders.
    pub tty: bool,
    /// Return whatever the remote command wrote to stdout even when it exits
    /// non-zero, instead of turning the exit code into an `Err`.
    ///
    /// For log reads only. Those pipelines end in a bare `grep`, which exits 1
    /// on "no matches" — a normal result, and exactly what the local branches
    /// report as empty output because they read stdout and ignore the status.
    /// `tail` on a log file that does not exist yet is the same story. Leave
    /// this false anywhere a non-zero exit is a genuine failure (probes, trail
    /// steps, git detection).
    pub allow_failure: bool,
}

fn control_path() -> PathBuf {
    config::yeehaw_dir().join("ssh").join("%r@%h:%p")
}

/// The address to dial for `barn`: its configured `host`, else the first
/// address it advertises about itself.
///
/// # Why the fallback lives here and nowhere else
///
/// `migrate::adopt_this_machine` writes a machine's own barn record with
/// `host: None`, because from that machine there is nothing to dial — you do not
/// ssh to yourself. That record is also exactly what every *other* machine
/// receives on sync, and to them it is remote and unreachable: `ssh_args` refused
/// it with "has no host configured" even though the join that created it had
/// reached that machine over ssh moments earlier.
///
/// `Barn.addresses` is what a machine volunteers instead (see
/// [`crate::migrate::advertise_this_machine`]), and this is the one function
/// that reads it. Every path that dials a barn — [`ssh_args`], and through it
/// `probe`, `connect`, `remote_grid`, the trail runner — goes through here, and
/// `ranch::resolve_target` asks the same question before refusing a target. A
/// fallback written at each of those call sites is five chances to disagree
/// about which address a barn is at.
///
/// **`host` wins.** It is what a human configured for this barn; `addresses` is
/// what the barn said about itself. Reversing that would make a host edited in
/// the TUI lose to a stale advertisement.
///
/// **The first address, not a search.** `ssh_args` builds one destination and
/// ssh has no "try these in turn" option, so an ordering is a choice that has to
/// be made somewhere. It is made at the *writing* end instead:
/// `advertise_this_machine` puts this machine's freshest candidate at the head
/// of the list and `merge::merge_addresses` preserves that order on every peer,
/// so the head of the list is the ranch's current best answer everywhere.
///
/// Blank entries are skipped: `""` would build `cam@` and ssh would then read
/// the next argv element as the destination.
///
/// `None` is not the end of the line, and neither is `Some` a decision.
/// [`route`] asks this question *second*: a barn with a `tunnel_port` goes
/// through the Ranch House whatever this answers, and this is consulted for the
/// barns that have no tunnel. So this function still answers exactly one
/// question, "what direct address does the barn have", and the priority between
/// that and the switchboard is decided one level up where the whole of it is
/// visible.
pub fn dial_host(barn: &Barn) -> Option<&str> {
    if let Some(host) = barn.host.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
        return Some(host);
    }
    barn.addresses.iter().map(|a| a.trim()).find(|a| !a.is_empty())
}

/// How this machine reaches a barn.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Straight at an address the barn has: [`dial_host`]'s answer, on the
    /// barn's own ssh port.
    Direct { host: String, port: u16 },
    /// ProxyJump through the Ranch House, to the port the barn's reverse tunnel
    /// binds there. `jump` is `[user@]host[:port]` for the house, ready for
    /// `-J`.
    ViaHouse { jump: String, port: u16 },
}

/// How to reach `barn`, given the Ranch House if the ranch has one.
///
/// Pure, and the single place the decision is made — [`ssh_args`] is the only
/// caller, so the same answer governs the host-key policy, the `ConnectTimeout`,
/// the `ControlMaster` and the identity file rather than each of them being
/// decided again at a call site.
///
/// # The house wins
///
/// **A barn with a `tunnel_port`, on a ranch whose house has an address, routes
/// through the house.** Direct is for the barns with no tunnel: a plain remote
/// host somebody typed a hostname for, a k8s-discovered node.
///
/// This is the reverse of how the fallback was first written, and the reversal is
/// the point of the design rather than a tuning choice. What a barn advertises
/// about itself is `<hostname>.local` ([`crate::migrate::advertise_this_machine`]),
/// and that is mDNS: it resolves on the LAN and nowhere else. Preferring it meant
/// that from anywhere but the LAN, every connect spent the whole of
/// `ConnectTimeout` on a name that was never going to resolve and then gave up —
/// while the house, reachable the entire time, was never tried. Worse, it is the
/// *silent* failure of the two: a barn that is genuinely off looks exactly the
/// same.
///
/// The house is not a fallback, it is the switchboard. Every barn dials out to it
/// and it forwards back down those tunnels, so "reachable" is a property of the
/// house being up rather than of where the user's laptop happens to be sitting —
/// which is the only way a ranch spread across a LAN, a NAT and a datacenter has
/// one answer to "how do I reach that machine".
///
/// The cost is that a LAN barn is reached over two hops when one would have done.
/// That is paid deliberately: the alternative is the connect that works at the
/// desk and fails on the road, and a routing rule that depends on which network
/// the user is on is a rule that cannot be tested.
///
/// # The house route, and why it exposes nothing
///
/// The barn holds `ssh -N -R <tunnel_port>:localhost:22 <house>`. Read from the
/// house's side, that binds `<tunnel_port>` on the **house's loopback** and
/// forwards anything that connects to it back down the barn's own outbound
/// session to the barn's port 22. Two consequences matter:
///
/// - **Nothing is published.** Without `GatewayPorts yes` in the house's sshd
///   config — and it is not wanted, not set, and not asked for here — `-R` binds
///   `127.0.0.1` only. The port is unreachable from the house's LAN, let alone
///   from the internet. "Reverse tunnel" reads alarming; this one is a loopback
///   listener on one machine.
/// - **So the only way in is from the house itself**, which is what the
///   ProxyJump provides: `ssh -J <house> -p <tunnel_port> <user>@localhost`
///   connects to the house first, and `localhost` is then resolved *on the
///   house*. The destination user is the barn's, because the far end of the
///   forward is the barn's sshd.
///
/// The jump hop authenticates from ssh-agent and `~/.ssh/config`: OpenSSH
/// implements `-J` as a nested `ssh -W`, which does not inherit this argv's `-o`
/// options or its `-i`. That is not a gap in practice — a machine cannot have a
/// tunnel to the house without being able to ssh to the house, so the house's
/// key is already in `known_hosts` and its credentials already work. The
/// *barn's* host key, at `[localhost]:<tunnel_port>`, is governed by this argv's
/// `StrictHostKeyChecking=accept-new` as usual, because that hop is this
/// connection.
///
/// # What is never routed through the house
///
/// The house itself. It is the machine everyone can reach, so it has no tunnel,
/// and jumping through it to get to it is a ProxyJump to the destination.
/// Guarded twice — by its own `is_ranch_house`, and by name against the house
/// passed in — because a copy of the record whose flag has not arrived yet would
/// otherwise slip through the first check.
///
/// A barn with no tunnel, and a barn whose ranch has no house with an address of
/// its own, both fall through to [`dial_host`]. A barn with neither a tunnel nor
/// an address is refused, as it always was.
pub fn route(barn: &Barn, house: Option<&Barn>) -> Option<Route> {
    via_house(barn, house).or_else(|| {
        dial_host(barn)
            .map(|host| Route::Direct { host: host.to_string(), port: barn.port.unwrap_or(22) })
    })
}

/// [`route`]'s first branch, split out only so the guards read as a list of
/// reasons the switchboard does not apply rather than as early returns tangled
/// with the direct case.
fn via_house(barn: &Barn, house: Option<&Barn>) -> Option<Route> {
    if barn.is_ranch_house == Some(true) {
        return None;
    }
    let port = barn.tunnel_port?;
    let house = house?;
    if house.name == barn.name {
        return None;
    }

    // A house with no address of its own is not a jump host. Returning `-J` with
    // an empty spec would have ssh read the next argv element as the jump host.
    let mut jump = match house.user.as_deref() {
        Some(user) => format!("{}@{}", user, dial_host(house)?),
        None => dial_host(house)?.to_string(),
    };
    if let Some(house_port) = house.port {
        jump = format!("{}:{}", jump, house_port);
    }
    Some(Route::ViaHouse { jump, port })
}

/// The Ranch House, read off the roster — but only when it could matter.
///
/// The early return is not a micro-optimization. [`ssh_args`] is on the path of
/// every probe, every connect and every trail step, and a directory walk per call
/// is a real cost to pay for an answer that cannot change the route.
///
/// The guard is the *same condition* [`via_house`] opens with, and it has to be:
/// a barn with no tunnel, or a barn that is itself the house, is dialled directly
/// whatever the roster says, so reading the roster for it buys nothing. Now that
/// the house wins, the guard is no longer "the barn has an address" — that is
/// exactly the case the inversion exists to route through the house.
fn house_for(barn: &Barn) -> Option<Barn> {
    if barn.tunnel_port.is_none() || barn.is_ranch_house == Some(true) {
        return None;
    }
    // `manifest::barns_from_disk`, not `config::load_barns()`: that injects a
    // synthetic `local` and drops a real `barns/local.yaml`, and neither belongs
    // in an answer to "which barn is the house".
    crate::ranch::manifest::barns_from_disk()
        .items
        .into_iter()
        .find(|b| b.is_ranch_house == Some(true))
}

/// Build the full argument vector for an `ssh` invocation against a barn.
///
/// Single source of truth for host-key policy, timeouts, identity, and
/// multiplexing. Returns the args only — the caller appends the remote command.
pub fn ssh_args(barn: &Barn, opts: Opts) -> Result<Vec<String>> {
    let house = house_for(barn);
    let route = route(barn, house.as_ref()).ok_or_else(|| {
        // Two ways to have no route, and they need different remedies. Saying
        // "no host configured" about a barn that has a perfectly good tunnel port
        // sends the user to edit a field that is not the problem.
        let tunnel_needs_a_house = barn.is_ranch_house != Some(true) && house.is_none();
        match barn.tunnel_port {
            Some(port) if tunnel_needs_a_house => anyhow!(
                "barn '{}' has no host configured, and the tunnel port {} the Ranch House gave it \
                 needs a house to jump through — this ranch has none. Run `yeehaw ranch init` on \
                 the machine every other one can reach",
                barn.name,
                port
            ),
            _ => anyhow!("barn '{}' has no host configured", barn.name),
        }
    })?;
    let (host, port, jump) = match &route {
        Route::Direct { host, port } => (host.as_str(), *port, None),
        Route::ViaHouse { jump, port } => ("localhost", *port, Some(jump.as_str())),
    };
    let user = barn.user.as_deref().unwrap_or("root");

    let mut args: Vec<String> = vec![
        "-o".into(), "StrictHostKeyChecking=accept-new".into(),
        "-o".into(), format!("ConnectTimeout={}", CONNECT_TIMEOUT_SECS),
        "-o".into(), "ControlMaster=auto".into(),
        "-o".into(), format!("ControlPath={}", control_path().display()),
        "-o".into(), format!("ControlPersist={}", CONTROL_PERSIST),
        "-o".into(), format!("ServerAliveInterval={}", SERVER_ALIVE_INTERVAL_SECS),
        "-o".into(), format!("ServerAliveCountMax={}", SERVER_ALIVE_COUNT_MAX),
    ];

    if opts.batch {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    if opts.tty {
        args.push("-t".into());
    }

    if let Some(jump) = jump {
        args.push("-J".into());
        args.push(jump.to_string());
    }

    args.push("-p".into());
    args.push(port.to_string());

    if let Some(key) = barn.identity_file.as_deref() {
        args.push("-i".into());
        args.push(key.to_string());
    }

    args.push(format!("{}@{}", user, host));
    Ok(args)
}

/// The argv for the reverse tunnel a barn holds open to the Ranch House:
/// `ssh -N -R <port>:localhost:22 <house>`, with the options that make it fail
/// rather than lie.
///
/// `None` when the house has no address of its own — there is nothing to dial,
/// and an argv ending in an empty destination would have ssh read one of the
/// `-o` values as the host.
///
/// This lives beside [`ssh_args`] rather than in [`crate::tunnel`] so that the
/// host-key policy, the `ConnectTimeout` and the keepalives are decided in one
/// file. It is deliberately *not* `ssh_args` with extra flags, and the reason is
/// the multiplexing: for each option ssh keeps the first value it is given, so
/// options cannot be overridden by appending, and a tunnel that shared
/// `ssh_args`'s `ControlMaster` would be wrong in both directions at once — as a
/// master it takes every probe down with it each time it reconnects, and as a
/// client it dies whenever the master it borrowed goes away. The tunnel is a
/// connection of its own for the life of the process, so it says so.
///
/// # The options, and the failure each one is for
///
/// - **`ExitOnForwardFailure=yes`** — the one that is not optional. Without it,
///   ssh whose `-R` was refused (something else already holds the port on the
///   house, most often a tunnel from an earlier run that has not been cleaned
///   up) connects anyway and sits there with no forward. The supervisor sees a
///   live child and reports a healthy tunnel; every peer that routes through it
///   gets "connection refused" from the house's loopback. A tunnel that looks up
///   and is not is worse than no tunnel, because nothing retries it.
/// - **`ServerAliveInterval` / `ServerAliveCountMax`** — a connection whose
///   network went away does not fail, it hangs: the local end has nothing to
///   notice. Same numbers as every other connection ([`SERVER_ALIVE_INTERVAL_SECS`]).
/// - **`BatchMode=yes`** — this runs on a background thread behind a
///   full-screen TUI. A passphrase prompt there is a prompt nobody can see and
///   nobody will answer, so it has to be a failure instead.
/// - **`-N`** — no remote command, no shell, no pty. The connection exists for
///   the forward and nothing else.
///
/// The forward is `<port>:localhost:22`: read on the house's side, that binds
/// `<port>` on the **house's loopback** — without `GatewayPorts` in its sshd
/// config, `-R` binds `127.0.0.1` only — and forwards back down this outbound
/// session to this machine's own sshd. [`route`] is the other half; see its
/// doc comment for why that publishes nothing.
pub fn tunnel_args(house: &Barn, port: u16) -> Option<Vec<String>> {
    let host = dial_host(house)?;

    let mut args: Vec<String> = vec![
        "-N".into(),
        "-o".into(), "ExitOnForwardFailure=yes".into(),
        "-o".into(), "StrictHostKeyChecking=accept-new".into(),
        "-o".into(), format!("ConnectTimeout={}", CONNECT_TIMEOUT_SECS),
        "-o".into(), format!("ServerAliveInterval={}", SERVER_ALIVE_INTERVAL_SECS),
        "-o".into(), format!("ServerAliveCountMax={}", SERVER_ALIVE_COUNT_MAX),
        "-o".into(), "BatchMode=yes".into(),
        // Not multiplexed, in either direction. See above.
        "-o".into(), "ControlMaster=no".into(),
        "-o".into(), "ControlPath=none".into(),
        "-R".into(), format!("{}:localhost:22", port),
        "-p".into(), house.port.unwrap_or(22).to_string(),
    ];

    if let Some(key) = house.identity_file.as_deref() {
        args.push("-i".into());
        args.push(key.to_string());
    }

    args.push(match house.user.as_deref() {
        Some(user) => format!("{}@{}", user, host),
        // No `user@`: ssh then uses `~/.ssh/config` or the local username,
        // which is the right answer for a house nobody has configured a user
        // for. `@host` would be a request to log in as nobody.
        None => host.to_string(),
    });
    Some(args)
}

/// Ensure the ControlPath parent directory exists, or multiplexing silently
/// falls back to a fresh connection per call.
pub fn ensure_control_dir() {
    if let Some(dir) = control_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
}

/// Build a ready-to-run `ssh` Command. The remote command is passed as a single
/// argv entry — there is no shell on the local side, so nothing can be injected
/// by a barn's host, user, or path.
pub fn command(barn: &Barn, remote_cmd: &str, opts: Opts) -> Result<Command> {
    ensure_control_dir();
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_args(barn, opts)?);
    cmd.arg(remote_cmd);
    Ok(cmd)
}

/// The message a failed remote command surfaces to the caller.
///
/// Split out of [`run`] so the fallback is testable: ssh frequently exits
/// non-zero with nothing on stderr but a newline, and an empty error message
/// reads in the TUI as a silent success.
fn failure_message(barn_name: &str, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("ssh to '{}' failed", barn_name)
    } else {
        stderr.to_string()
    }
}

/// Run a command on a barn and capture stdout. Non-zero exit returns Err with
/// stderr, so callers get a real message instead of empty output — unless
/// `opts.allow_failure` is set, in which case stdout comes back as-is.
pub fn run(barn: &Barn, remote_cmd: &str, opts: Opts) -> Result<String> {
    let output = command(barn, remote_cmd, opts)?
        .output()
        .map_err(|e| anyhow!("failed to spawn ssh: {}", e))?;

    if !opts.allow_failure && !output.status.success() {
        return Err(anyhow!(failure_message(&barn.name, &output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// What a barn reported about itself during pre-flight.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Probe {
    pub has_tmux: bool,
    pub has_yeehaw: bool,
    pub session_live: bool,
}

/// The pre-flight the barn runs. Two properties matter, and both were bugs.
///
/// **It always exits 0.** The findings travel on stdout as flags; the exit
/// status is reserved for whether we reached a shell at all. Without the
/// trailing `exit 0` the status is `tmux has-session`'s, which is 1 whenever no
/// yeehaw session is running — the normal state of a barn nobody has attached to
/// yet. `run` turned that into `Err`, `connect.rs` rendered "unreachable", and
/// `2>/dev/null` left `failure_message` with nothing to say, so the *first*
/// connect to every barn failed with no explanation. It also contradicted
/// `connect::blocker`, which deliberately treats a missing session as not a
/// blocker because remote yeehaw creates its own on launch. Discarding the
/// status costs nothing: ssh reports its own failures — unreachable, auth,
/// host-key — as 255, and a barn with no `bash` still comes back as 127 from
/// the remote shell, both of which `run` still sees.
///
/// **It runs under `bash -lc`, exactly like [`crate::connect`]'s attach.** sshd
/// runs a non-login, non-interactive shell whose PATH has neither Homebrew nor
/// `~/.local/bin`, so probing the raw shell reported "tmux is not installed" for
/// barns where tmux and yeehaw both exist and attaching works fine.
///
/// The inner script holds no `$` and no double quotes, so it nests inside
/// `bash -lc "..."` as one argv element with no further escaping.
const PROBE_CMD: &str = "bash -lc \"\
                         command -v tmux >/dev/null && echo tmux:ok; \
                         command -v yeehaw >/dev/null && echo yeehaw:ok; \
                         tmux has-session -t yeehaw 2>/dev/null && echo session:live; \
                         exit 0\"";

/// Parse probe output. Matches whole lines only — barns print MOTDs, and a
/// substring match would read "yeehaw is great" as a positive flag.
pub fn parse_probe(stdout: &str) -> Probe {
    let mut p = Probe::default();
    for line in stdout.lines() {
        match line.trim() {
            "tmux:ok" => p.has_tmux = true,
            "yeehaw:ok" => p.has_yeehaw = true,
            "session:live" => p.session_live = true,
            _ => {}
        }
    }
    p
}

/// One SSH round trip that distinguishes every connect failure mode.
/// Err means unreachable or auth failure; Ok means we got a shell and can tell
/// exactly what is missing.
pub fn probe(barn: &Barn) -> Result<Probe> {
    let out = run(barn, PROBE_CMD, Opts { batch: true, ..Default::default() })?;
    Ok(parse_probe(&out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Barn;

    // `ssh_args` puts the ControlPath under the ranch and `ssh::command`
    // creates its parent directory, so the tests below open a temp ranch
    // (`let _ranch = ...`) rather than reading and writing the developer's real
    // `~/.yeehaw/ssh`. The guard has to be a binding: it points the ranch at a
    // temp directory only until it drops.

    fn barn(identity: Option<&str>) -> Barn {
        Barn {
            name: "guided".into(),
            host: Some("172.233.141.59".into()),
            user: Some("forge".into()),
            port: Some(2222),
            identity_file: identity.map(|s| s.into()),
            critters: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn builds_target_from_user_and_host() {
        let _ranch = crate::testing::temp_ranch();
        let args = ssh_args(&barn(None), Opts::default()).expect("configured barn");
        assert!(args.contains(&"forge@172.233.141.59".to_string()));
    }

    #[test]
    fn passes_the_configured_port() {
        let _ranch = crate::testing::temp_ranch();
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        let p = args.iter().position(|a| a == "-p").expect("-p present");
        assert_eq!(args[p + 1], "2222");
    }

    #[test]
    fn omits_identity_flag_when_barn_has_no_key() {
        let _ranch = crate::testing::temp_ranch();
        // A barn relying on ssh-agent or ~/.ssh/config must still connect.
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(!args.contains(&"-i".to_string()));
    }

    #[test]
    fn includes_identity_flag_when_barn_has_a_key() {
        let _ranch = crate::testing::temp_ranch();
        let args = ssh_args(&barn(Some("~/.ssh/id_big_ups")), Opts::default()).unwrap();
        let i = args.iter().position(|a| a == "-i").expect("-i present");
        assert_eq!(args[i + 1], "~/.ssh/id_big_ups");
    }

    #[test]
    fn always_pins_host_keys_with_accept_new() {
        let _ranch = crate::testing::temp_ranch();
        // StrictHostKeyChecking=no accepts any key silently and permits MITM.
        // accept-new pins on first use and refuses on change.
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
        assert!(!args.iter().any(|a| a.contains("StrictHostKeyChecking=no")));
    }

    #[test]
    fn always_sets_a_connect_timeout() {
        let _ranch = crate::testing::temp_ranch();
        // Without this an unreachable barn hangs the caller forever.
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(args.iter().any(|a| a.starts_with("ConnectTimeout=")));
    }

    #[test]
    fn enables_connection_multiplexing() {
        let _ranch = crate::testing::temp_ranch();
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(args.contains(&"ControlMaster=auto".to_string()));
        assert!(args.iter().any(|a| a.starts_with("ControlPersist=")));
    }

    #[test]
    fn batch_mode_is_opt_in_so_interactive_auth_still_works() {
        let _ranch = crate::testing::temp_ranch();
        let probe = ssh_args(&barn(None), Opts { batch: true, ..Opts::default() }).unwrap();
        assert!(probe.contains(&"BatchMode=yes".to_string()));

        let interactive = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(!interactive.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn requests_a_tty_only_when_asked() {
        let _ranch = crate::testing::temp_ranch();
        let with = ssh_args(&barn(None), Opts { tty: true, ..Opts::default() }).unwrap();
        assert!(with.contains(&"-t".to_string()));

        let without = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(!without.contains(&"-t".to_string()));
    }

    #[test]
    fn rejects_a_barn_with_no_host() {
        // The harness is needed as of Slice E and was not before: `ssh_args` now
        // consults the roster for a Ranch House when there is no direct address,
        // so this test reaches `yeehaw_dir()` where it used to fail earlier. An
        // empty temp ranch has no house, which is exactly the case asserted.
        let _ranch = crate::testing::temp_ranch();
        let mut b = barn(None);
        b.host = None;
        assert!(ssh_args(&b, Opts::default()).is_err());
    }

    // === the advertised-address fallback ===================================
    //
    // A machine's own barn record is written by `migrate::adopt_this_machine`
    // with `host: None` — from that machine there is nothing to dial. That
    // record is exactly what every *other* machine receives on sync, and to them
    // it is remote. `Barn.addresses` is what it advertises instead, and
    // `dial_host` is the single place that reads it: see its doc comment for why
    // the fallback lives here and not at each call site.

    /// THE BUG. The iMac's own record arrives on the MacBook with `host: null`,
    /// so `ssh_args` refused it outright — `yeehaw connect camerons-imac` said
    /// "barn 'camerons-imac' has no host configured" even though the join that
    /// created the record had reached that machine over ssh moments earlier.
    #[test]
    fn a_hostless_barn_is_dialled_at_the_address_it_advertises() {
        let _ranch = crate::testing::temp_ranch();
        let mut b = barn(None);
        b.host = None;
        b.user = Some("cam".into());
        b.port = None;
        b.addresses = vec!["camerons-imac.local".into()];

        let args = ssh_args(&b, Opts::default()).expect("an advertised address is dialable");
        assert!(
            args.contains(&"cam@camerons-imac.local".to_string()),
            "the advertised address must be dialled: {:?}",
            args
        );
    }

    /// `host` is what a human configured for this barn; `addresses` is what the
    /// barn said about itself. The configured value wins, or editing a host in
    /// the TUI would silently keep dialling a stale advertisement.
    #[test]
    fn a_configured_host_beats_an_advertised_address() {
        let _ranch = crate::testing::temp_ranch();
        let mut b = barn(None);
        b.addresses = vec!["stale.local".into()];

        assert_eq!(dial_host(&b), Some("172.233.141.59"));
        let args = ssh_args(&b, Opts::default()).unwrap();
        assert!(!args.iter().any(|a| a.contains("stale.local")), "{:?}", args);
    }

    /// The order is the ranch's answer to "which address first", and it is
    /// deliberate: `migrate::advertise_this_machine` puts this machine's freshest
    /// candidate at the head of the list and `merge::merge_addresses` preserves
    /// that order on every peer. Dialling anything but the first would discard
    /// it.
    #[test]
    fn the_first_advertised_address_is_the_one_dialled() {
        let mut b = barn(None);
        b.host = None;
        b.addresses = vec!["fresh.local".into(), "older.local".into()];
        assert_eq!(dial_host(&b), Some("fresh.local"));
    }

    /// A k8s-discovered node has neither, and so does a barn record a user
    /// half-filled. The refusal has to survive the fallback.
    #[test]
    fn a_barn_with_neither_a_host_nor_an_address_is_still_refused() {
        // See `rejects_a_barn_with_no_host` for why the harness is here now.
        let _ranch = crate::testing::temp_ranch();
        let mut b = barn(None);
        b.host = None;
        assert!(b.addresses.is_empty(), "the fixture must advertise nothing");
        assert_eq!(dial_host(&b), None);
        assert!(ssh_args(&b, Opts::default()).is_err());
    }

    /// A blank entry is not an address. Dialled, it would build `cam@` and ssh
    /// would read the *next* argv element as the destination.
    #[test]
    fn a_blank_advertised_address_is_not_dialled() {
        let mut b = barn(None);
        b.host = None;
        b.addresses = vec!["".into(), "  ".into(), "real.local".into()];
        assert_eq!(dial_host(&b), Some("real.local"));
    }

    // === the Ranch House route =============================================
    //
    // A barn with no direct address is not out of reach: it holds
    // `ssh -N -R <tunnel_port>:localhost:22 <house>`, so `<tunnel_port>` is bound
    // on the *house's* loopback and the barn can be reached by jumping to the
    // house and then connecting to that port. These tests read the generated
    // argv; nothing here opens a connection.

    /// A Ranch House on the roster of whatever temp ranch is in scope.
    fn a_house_on_the_roster(name: &str) -> Barn {
        let mut house = Barn {
            name: name.into(),
            host: Some("camerons-imac.local".into()),
            user: Some("cam".into()),
            is_ranch_house: Some(true),
            ..Default::default()
        };
        crate::config::save_barn(&mut house).expect("the roster takes a house");
        house
    }

    /// A barn that advertises nothing and has a tunnel: the Pi behind NAT, or a
    /// machine whose own record still says `host: null` and whose `.local` name
    /// never resolved.
    fn a_tunnelled_barn() -> Barn {
        Barn {
            name: "pi".into(),
            host: None,
            user: Some("cam".into()),
            tunnel_port: Some(23007),
            ..Default::default()
        }
    }

    /// THE ROUTE. `ssh -J <house> -p <tunnel_port> <user>@localhost`: jump to the
    /// house, then from the house connect to the port the barn's own reverse
    /// tunnel bound on the house's loopback.
    ///
    /// `localhost` is the point, and it is why nothing here is exposed: `-R`
    /// without `GatewayPorts` binds the loopback interface of the house only, so
    /// the forwarded port is unreachable from anywhere except a process already
    /// on the house — which, after the jump, is what we are.
    #[test]
    fn a_barn_with_no_address_but_a_tunnel_is_reached_through_the_house() {
        let _ranch = crate::testing::temp_ranch();
        a_house_on_the_roster("imac");

        let args = ssh_args(&a_tunnelled_barn(), Opts::default())
            .expect("a tunnelled barn is reachable through the house");

        let j = args.iter().position(|a| a == "-J").expect("-J present");
        assert_eq!(args[j + 1], "cam@camerons-imac.local", "{:?}", args);
        let p = args.iter().position(|a| a == "-p").expect("-p present");
        assert_eq!(args[p + 1], "23007", "the tunnel port, not the barn's ssh port: {:?}", args);
        assert!(
            args.contains(&"cam@localhost".to_string()),
            "the far end of the forward is the house's own loopback: {:?}",
            args
        );
    }

    /// The jump spec carries the house's user and port, because a house on a
    /// non-default ssh port is a house ProxyJump cannot reach without them.
    #[test]
    fn the_jump_spec_carries_the_houses_own_user_and_port() {
        let _ranch = crate::testing::temp_ranch();
        let mut house = a_house_on_the_roster("imac");
        house.port = Some(2022);
        crate::config::save_barn(&mut house).unwrap();

        let args = ssh_args(&a_tunnelled_barn(), Opts::default()).unwrap();
        let j = args.iter().position(|a| a == "-J").unwrap();
        assert_eq!(args[j + 1], "cam@camerons-imac.local:2022", "{:?}", args);
    }

    /// **THE HOUSE WINS.** THE BUG this pass inverts. A barn advertises
    /// `<hostname>.local`, which is mDNS: it resolves on the LAN and nowhere
    /// else. Preferring it meant that off-LAN every connect spent the full
    /// `ConnectTimeout` failing to resolve a name that was never going to
    /// resolve, and then gave up — the house, which was reachable the whole
    /// time, was never tried. A barn with a tunnel routes through the house
    /// whether or not it also advertises an address.
    #[test]
    fn a_tunnel_and_a_house_beat_a_direct_address() {
        let _ranch = crate::testing::temp_ranch();
        a_house_on_the_roster("imac");

        let mut b = a_tunnelled_barn();
        b.addresses = vec!["pi.local".into()];

        let args = ssh_args(&b, Opts::default()).unwrap();
        let j = args.iter().position(|a| a == "-J").expect("-J present");
        assert_eq!(args[j + 1], "cam@camerons-imac.local", "{:?}", args);
        assert!(
            !args.iter().any(|a| a.contains("pi.local")),
            "the mDNS name is not dialled when the switchboard is available: {:?}",
            args
        );
        let p = args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(args[p + 1], "23007", "the tunnel port, not the barn's ssh port: {:?}", args);
    }

    /// Direct is for a barn with **no tunnel** — a plain remote host that was
    /// never enrolled, or a k8s node. It keeps its own address and its own ssh
    /// port, and the roster is not even read for it.
    #[test]
    fn a_barn_with_no_tunnel_is_dialled_directly() {
        let _ranch = crate::testing::temp_ranch();
        a_house_on_the_roster("imac");

        let mut b = a_tunnelled_barn();
        b.tunnel_port = None;
        b.addresses = vec!["pi.local".into()];
        b.port = Some(2222);

        let args = ssh_args(&b, Opts::default()).unwrap();
        assert!(!args.contains(&"-J".to_string()), "no jump for a barn with no tunnel: {:?}", args);
        assert!(args.contains(&"cam@pi.local".to_string()), "{:?}", args);
        let p = args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(args[p + 1], "2222", "the barn's own ssh port: {:?}", args);
    }

    /// And the same, one layer down, without touching the disk: `route` is where
    /// the decision lives, so the precedence is pinned on the function that makes
    /// it rather than only through the store. A configured `host` does not beat
    /// the switchboard either — it is as likely to be the `.local` name the join
    /// wrote as anything else.
    #[test]
    fn route_prefers_the_house_even_when_the_barn_has_a_direct_address() {
        let house = Barn {
            name: "imac".into(),
            host: Some("imac.local".into()),
            is_ranch_house: Some(true),
            ..Default::default()
        };
        let mut b = a_tunnelled_barn();
        b.host = Some("pi.lan".into());
        b.port = Some(2222);

        assert_eq!(
            route(&b, Some(&house)),
            Some(Route::ViaHouse { jump: "imac.local".into(), port: 23007 })
        );
    }

    /// The house itself is always dialled directly, address and all — it is the
    /// machine everyone can reach, which is the whole reason it is the house.
    #[test]
    fn the_house_is_always_dialled_directly() {
        let house = Barn {
            name: "imac".into(),
            host: Some("imac.local".into()),
            user: Some("cam".into()),
            is_ranch_house: Some(true),
            ..Default::default()
        };
        assert_eq!(
            route(&house, Some(&house.clone())),
            Some(Route::Direct { host: "imac.local".into(), port: 22 })
        );
    }

    /// A house with no address of its own is not a jump host, and the barn is
    /// then dialled at whatever it does advertise rather than refused. The
    /// fall-through is the point: inverting the priority must not turn a
    /// half-configured house into a ranch nobody can reach.
    #[test]
    fn a_barn_falls_back_to_its_own_address_when_the_house_has_none() {
        let house = Barn {
            name: "imac".into(),
            host: None,
            is_ranch_house: Some(true),
            ..Default::default()
        };
        let mut b = a_tunnelled_barn();
        b.addresses = vec!["pi.local".into()];

        assert_eq!(
            route(&b, Some(&house)),
            Some(Route::Direct { host: "pi.local".into(), port: 22 })
        );
    }

    /// A ranch with no house has nothing to jump through, so a tunnel port is not
    /// a route. Refused rather than silently dialled at `localhost`, which would
    /// be an ssh to *this* machine.
    #[test]
    fn a_tunnel_port_with_no_house_on_the_ranch_is_not_a_route() {
        assert_eq!(route(&a_tunnelled_barn(), None), None);

        let _ranch = crate::testing::temp_ranch();
        let err = ssh_args(&a_tunnelled_barn(), Opts::default())
            .expect_err("nothing to jump through");
        let why = err.to_string();
        assert!(why.contains("pi"), "the error must name the barn: {}", why);
        // "no host configured" alone sends the user to edit a field that is not
        // the problem: this barn's port is fine, the ranch has no house.
        assert!(why.contains("23007"), "the error must name the port it cannot use: {}", why);
        assert!(why.contains("ranch init"), "and what to do about it: {}", why);
    }

    /// The Ranch House is the thing everyone can reach, so it is never reached
    /// *through itself* — that would be a ProxyJump to the destination.
    #[test]
    fn the_house_is_never_routed_through_itself() {
        let mut house = Barn {
            name: "imac".into(),
            host: None,
            user: Some("cam".into()),
            is_ranch_house: Some(true),
            // Nonsense on the house, and the guard must not depend on it being
            // absent: a record that has one from an earlier build still must not
            // produce `-J imac ... imac`.
            tunnel_port: Some(23000),
            ..Default::default()
        };
        assert_eq!(route(&house, Some(&house.clone())), None);

        // And by name, for a copy whose `is_ranch_house` never arrived.
        house.is_ranch_house = None;
        let flagged = Barn { is_ranch_house: Some(true), ..house.clone() };
        assert_eq!(route(&house, Some(&flagged)), None);
    }

    /// A house whose own record has no address is a house nothing can jump
    /// through. No `-J` with an empty spec, which ssh would read as the next argv
    /// element being the jump host.
    #[test]
    fn a_house_with_no_address_of_its_own_is_not_a_jump_host() {
        let house = Barn {
            name: "imac".into(),
            host: None,
            is_ranch_house: Some(true),
            ..Default::default()
        };
        assert_eq!(route(&a_tunnelled_barn(), Some(&house)), None);
    }

    /// A barn with neither an address nor a tunnel port is still refused — the
    /// k8s-discovered node and the half-filled record from `dial_host`'s own
    /// tests. The fallback must not invent a route for it.
    #[test]
    fn a_barn_with_neither_an_address_nor_a_tunnel_port_is_still_refused() {
        let house = Barn {
            name: "imac".into(),
            host: Some("imac.local".into()),
            is_ranch_house: Some(true),
            ..Default::default()
        };
        let mut b = a_tunnelled_barn();
        b.tunnel_port = None;
        assert_eq!(route(&b, Some(&house)), None);
    }

    /// The host-key policy and the timeout still apply, because the ProxyJump is
    /// an option on the same connection rather than a different code path. This is
    /// the reason the fallback lives in `ssh_args` and not at the call sites.
    #[test]
    fn the_house_route_keeps_every_option_the_direct_one_has() {
        let _ranch = crate::testing::temp_ranch();
        a_house_on_the_roster("imac");

        let args = ssh_args(&a_tunnelled_barn(), Opts { batch: true, tty: true, ..Opts::default() })
            .unwrap();
        for expected in [
            "StrictHostKeyChecking=accept-new",
            "ControlMaster=auto",
            "BatchMode=yes",
            "ServerAliveInterval=15",
        ] {
            assert!(args.contains(&expected.to_string()), "{} missing from {:?}", expected, args);
        }
        assert!(args.iter().any(|a| a.starts_with("ConnectTimeout=")), "{:?}", args);
        assert!(args.contains(&"-t".to_string()), "{:?}", args);
    }

    /// Two tunnelled barns must not share a multiplexed connection. They do not,
    /// and the reason is worth pinning: `ControlPath` is `%r@%h:%p`, and while
    /// `%h` is `localhost` for both, `%p` is the barn's own tunnel port — so the
    /// paths differ. If the port ever stopped being in the control path, `c` into
    /// one barn would reuse the master for another.
    #[test]
    fn two_tunnelled_barns_do_not_share_a_control_path() {
        let _ranch = crate::testing::temp_ranch();
        a_house_on_the_roster("imac");

        let one = ssh_args(&a_tunnelled_barn(), Opts::default()).unwrap();
        let mut other = a_tunnelled_barn();
        other.name = "ascend".into();
        other.tunnel_port = Some(23008);
        let two = ssh_args(&other, Opts::default()).unwrap();

        let port_of = |args: &[String]| {
            let p = args.iter().position(|a| a == "-p").unwrap();
            args[p + 1].clone()
        };
        assert_ne!(port_of(&one), port_of(&two));
        assert!(
            one.iter().any(|a| a.starts_with("ControlPath=")),
            "the control path has to be there for %p to disambiguate it: {:?}",
            one
        );
    }

    #[test]
    fn defaults_the_user_and_port_when_absent() {
        let _ranch = crate::testing::temp_ranch();
        let mut b = barn(None);
        b.user = None;
        b.port = None;
        let args = ssh_args(&b, Opts::default()).unwrap();
        assert!(args.contains(&"root@172.233.141.59".to_string()));
        let p = args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(args[p + 1], "22");
    }

    #[test]
    fn failures_are_surfaced_by_default() {
        // allow_failure suppresses every non-zero exit, including auth and
        // connection failures. Only log reads may opt in; if the default ever
        // flips, a probe or trail step would silently report success on an
        // unreachable barn.
        assert!(!Opts::default().allow_failure);
        assert!(!Opts { batch: true, ..Opts::default() }.allow_failure);
        assert!(!Opts { tty: true, ..Opts::default() }.allow_failure);
    }

    #[test]
    fn allow_failure_does_not_alter_the_ssh_argv() {
        let _ranch = crate::testing::temp_ranch();
        // It is a decision about how `run` reads the exit status, not an ssh
        // flag; the connection must be built identically either way.
        let plain = ssh_args(&barn(Some("~/.ssh/k")), Opts::default()).unwrap();
        let lenient = ssh_args(
            &barn(Some("~/.ssh/k")),
            Opts { allow_failure: true, ..Opts::default() },
        )
        .unwrap();
        assert_eq!(plain, lenient);
    }

    #[test]
    fn failure_reports_what_ssh_said() {
        let msg = failure_message("guided", b"Permission denied (publickey).\n");
        assert_eq!(msg, "Permission denied (publickey).");
    }

    #[test]
    fn failure_is_never_an_empty_message() {
        // A non-zero exit with a blank stderr is common; surfacing "" would be
        // indistinguishable from a command that succeeded with no output.
        for stderr in [&b""[..], b"\n", b"  \n\t"] {
            let msg = failure_message("guided", stderr);
            assert!(!msg.trim().is_empty(), "empty message for {:?}", stderr);
            assert!(msg.contains("guided"), "message should name the barn");
        }
    }

    #[test]
    fn probe_reports_a_fully_ready_barn() {
        let p = parse_probe("tmux:ok\nyeehaw:ok\nsession:live\n");
        assert!(p.has_tmux && p.has_yeehaw && p.session_live);
    }

    #[test]
    fn probe_reports_yeehaw_installed_but_not_running() {
        let p = parse_probe("tmux:ok\nyeehaw:ok\n");
        assert!(p.has_tmux && p.has_yeehaw);
        assert!(!p.session_live);
    }

    #[test]
    fn probe_reports_missing_yeehaw() {
        let p = parse_probe("tmux:ok\n");
        assert!(p.has_tmux);
        assert!(!p.has_yeehaw);
    }

    #[test]
    fn probe_reports_missing_tmux() {
        let p = parse_probe("yeehaw:ok\n");
        assert!(!p.has_tmux);
        assert!(p.has_yeehaw);
    }

    #[test]
    fn probe_of_empty_output_reports_nothing_present() {
        let p = parse_probe("");
        assert!(!p.has_tmux && !p.has_yeehaw && !p.session_live);
    }

    #[test]
    fn probe_ignores_login_banner_noise() {
        // Barns print MOTDs and shell warnings; these must not be mistaken for flags.
        let p = parse_probe("Welcome to Ubuntu\nyeehaw is great\ntmux:ok\n");
        assert!(p.has_tmux);
        assert!(!p.has_yeehaw, "'yeehaw is great' is not the yeehaw:ok flag");
    }

    // === PROBE_CMD through a real shell ====================================
    //
    // PROBE_CMD is a shell script we never parse ourselves — sshd hands it to
    // the barn's shell. Both bugs it has had were in how a real shell ran it,
    // not in how it reads, so these tests run the actual constant through an
    // actual `sh` rather than comparing it to a hand-written expectation that
    // could be wrong in the same way the constant is. Same approach as
    // `tmux::tests::assert_sh_roundtrip` and the mcp_server injection tests.

    /// The PATH sshd gives a non-login shell: no Homebrew, no `~/.local/bin`.
    /// Observed on a real barn as
    /// `/bin:/usr/bin:/usr/ucb:/usr/local/bin`, where `tmux` lives in
    /// `/opt/homebrew/bin` and `yeehaw` in `~/.local/bin`.
    const NON_LOGIN_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

    /// Run `script` through a real `sh`, the way sshd hands a remote command to
    /// the barn's shell. `path` replaces PATH outright when given. `/bin/sh` is
    /// spelled absolutely so a replaced PATH cannot change which shell runs.
    fn sh_probe(script: &str, path: Option<&str>) -> (Option<i32>, String) {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", script]);
        if let Some(p) = path {
            cmd.env("PATH", p);
        }
        let out = cmd.output().expect("sh should be runnable");
        (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Install `script` as an executable named `name` in `dir`, and return `dir`
    /// as a PATH value.
    fn stub_bin(dir: &std::path::Path, name: &str, script: &str) -> String {
        let bin = dir.join(name);
        std::fs::write(&bin, script).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        dir.display().to_string()
    }

    /// `PROBE_CMD` with the probed session renamed to one that cannot exist, so
    /// the "no session running" branch is exercised on any machine — including a
    /// dev box that is running yeehaw in tmux right now.
    fn probe_cmd_with_no_live_session() -> String {
        let absent = format!("yeehaw-absent-{}", std::process::id());
        let swapped = PROBE_CMD.replace("-t yeehaw ", &format!("-t {} ", absent));
        assert_ne!(
            swapped, PROBE_CMD,
            "PROBE_CMD no longer contains '-t yeehaw '; this test has gone vacuous"
        );
        swapped
    }

    #[test]
    fn probe_exits_zero_when_no_session_is_running() {
        // THE regression test for bug A. The old PROBE_CMD ended in
        // `tmux has-session`, so its status was 1 whenever no yeehaw session
        // existed. `run` turns non-zero into Err and connect.rs renders
        // "unreachable — ssh to '<barn>' failed", which made the FIRST connect to
        // every barn fail: no session yet is the normal initial state, and
        // connect::blocker deliberately does not treat it as a blocker.
        let (code, stdout) = sh_probe(&probe_cmd_with_no_live_session(), None);
        assert_eq!(code, Some(0), "probe must exit 0; stdout was {stdout:?}");
        assert!(
            !parse_probe(&stdout).session_live,
            "a session that cannot exist must not report session:live: {stdout:?}"
        );
    }

    #[test]
    fn probe_exits_zero_when_the_barn_has_nothing_installed() {
        // Bug A's worst case: tmux missing, yeehaw missing, and `tmux has-session`
        // therefore failing with 127. The status must still be 0 — "this barn has
        // nothing on it" is a finding for stdout, not a connection failure. A
        // stand-in login shell that runs the script under an empty PATH makes the
        // case reachable regardless of what this machine has installed.
        let dir = tempfile::tempdir().expect("tempdir");
        // Accepts `-lc <script>` and runs the script with PATH still empty.
        let path = stub_bin(dir.path(), "bash", "#!/bin/sh\nexec /bin/sh -c \"$2\"\n");

        let (code, stdout) = sh_probe(PROBE_CMD, Some(&path));
        assert_eq!(code, Some(0), "probe must exit 0; stdout was {stdout:?}");

        let p = parse_probe(&stdout);
        assert_eq!(p, Probe::default(), "nothing is installed, so no flags: {stdout:?}");
    }

    #[test]
    fn probe_runs_under_a_login_shell_as_a_single_argument() {
        // Bug B. A `bash` that prints its argv proves both halves at once: the
        // `-l` really reaches the shell, and the whole inner script arrives as
        // ONE argument instead of being split by the outer shell that sshd uses
        // to run the remote command.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = stub_bin(
            dir.path(),
            "bash",
            "#!/bin/sh\nfor a in \"$@\"; do printf 'ARG[%s]\\n' \"$a\"; done\n",
        );

        let (_, stdout) = sh_probe(PROBE_CMD, Some(&path));
        let args: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("ARG[")?.strip_suffix(']'))
            .collect();

        assert_eq!(args.len(), 2, "expected `bash -lc <script>`, got {args:?}");
        assert_eq!(
            args[0], "-lc",
            "the probe must use a login shell, the same as connect's attach"
        );
        for fragment in ["command -v tmux", "command -v yeehaw", "has-session", "exit 0"] {
            assert!(
                args[1].contains(fragment),
                "the script reached bash mangled — {fragment:?} missing from {:?}",
                args[1]
            );
        }
    }

    #[test]
    fn probe_finds_tools_a_non_login_shell_would_miss() {
        // Bug B end to end, from sshd's actual PATH. Ground truth is what the
        // login shell can reach starting from that same PATH, computed without
        // going through PROBE_CMD. On a machine whose profile adds Homebrew or
        // ~/.local/bin, dropping the `bash -lc` makes this fail: the probe would
        // report "not installed" for tools that are installed, and connect.rs
        // would refuse to attach to a barn that attaches fine.
        let (code, stdout) = sh_probe(&probe_cmd_with_no_live_session(), Some(NON_LOGIN_PATH));
        assert_eq!(code, Some(0), "probe must exit 0; stdout was {stdout:?}");
        let p = parse_probe(&stdout);

        for (tool, found) in [("tmux", p.has_tmux), ("yeehaw", p.has_yeehaw)] {
            let script = format!("bash -lc \"command -v {tool} >/dev/null && echo yes; exit 0\"");
            let (_, truth) = sh_probe(&script, Some(NON_LOGIN_PATH));
            let truth = truth.contains("yes");
            assert_eq!(
                found, truth,
                "the login shell {} reach {tool} but the probe {} it",
                if truth { "can" } else { "cannot" },
                if found { "found" } else { "did not find" }
            );
        }

        assert!(!p.session_live, "the substituted session cannot exist: {stdout:?}");
    }

    #[test]
    fn the_real_probe_command_runs_clean_on_this_machine() {
        // No substitutions, no stubs: the exact constant `probe` ships, through a
        // real shell. Whatever this machine has installed, the status is 0 and
        // the only lines that parse as flags are flags we asked for.
        let (code, stdout) = sh_probe(PROBE_CMD, None);
        assert_eq!(code, Some(0), "probe must exit 0; stdout was {stdout:?}");
        assert!(
            !stdout.contains("command not found"),
            "the probe leaked shell errors onto stdout: {stdout:?}"
        );
    }
}

