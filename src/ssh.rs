use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Result};

use crate::config;
use crate::types::Barn;

const CONNECT_TIMEOUT_SECS: u32 = 10;
const CONTROL_PERSIST: &str = "10m";

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

/// Build the full argument vector for an `ssh` invocation against a barn.
///
/// Single source of truth for host-key policy, timeouts, identity, and
/// multiplexing. Returns the args only — the caller appends the remote command.
pub fn ssh_args(barn: &Barn, opts: Opts) -> Result<Vec<String>> {
    let host = barn
        .host
        .as_deref()
        .ok_or_else(|| anyhow!("barn '{}' has no host configured", barn.name))?;
    let user = barn.user.as_deref().unwrap_or("root");
    let port = barn.port.unwrap_or(22);

    let mut args: Vec<String> = vec![
        "-o".into(), "StrictHostKeyChecking=accept-new".into(),
        "-o".into(), format!("ConnectTimeout={}", CONNECT_TIMEOUT_SECS),
        "-o".into(), "ControlMaster=auto".into(),
        "-o".into(), format!("ControlPath={}", control_path().display()),
        "-o".into(), format!("ControlPersist={}", CONTROL_PERSIST),
        "-o".into(), "ServerAliveInterval=15".into(),
        "-o".into(), "ServerAliveCountMax=3".into(),
    ];

    if opts.batch {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    if opts.tty {
        args.push("-t".into());
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

    fn barn(identity: Option<&str>) -> Barn {
        Barn {
            name: "guided".into(),
            host: Some("172.233.141.59".into()),
            user: Some("forge".into()),
            port: Some(2222),
            identity_file: identity.map(|s| s.into()),
            critters: vec![],
            source: None,
            connection_type: None,
            connection_config: None,
            connectable: None,
        }
    }

    #[test]
    fn builds_target_from_user_and_host() {
        let args = ssh_args(&barn(None), Opts::default()).expect("configured barn");
        assert!(args.contains(&"forge@172.233.141.59".to_string()));
    }

    #[test]
    fn passes_the_configured_port() {
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        let p = args.iter().position(|a| a == "-p").expect("-p present");
        assert_eq!(args[p + 1], "2222");
    }

    #[test]
    fn omits_identity_flag_when_barn_has_no_key() {
        // A barn relying on ssh-agent or ~/.ssh/config must still connect.
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(!args.contains(&"-i".to_string()));
    }

    #[test]
    fn includes_identity_flag_when_barn_has_a_key() {
        let args = ssh_args(&barn(Some("~/.ssh/id_big_ups")), Opts::default()).unwrap();
        let i = args.iter().position(|a| a == "-i").expect("-i present");
        assert_eq!(args[i + 1], "~/.ssh/id_big_ups");
    }

    #[test]
    fn always_pins_host_keys_with_accept_new() {
        // StrictHostKeyChecking=no accepts any key silently and permits MITM.
        // accept-new pins on first use and refuses on change.
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
        assert!(!args.iter().any(|a| a.contains("StrictHostKeyChecking=no")));
    }

    #[test]
    fn always_sets_a_connect_timeout() {
        // Without this an unreachable barn hangs the caller forever.
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(args.iter().any(|a| a.starts_with("ConnectTimeout=")));
    }

    #[test]
    fn enables_connection_multiplexing() {
        let args = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(args.contains(&"ControlMaster=auto".to_string()));
        assert!(args.iter().any(|a| a.starts_with("ControlPersist=")));
    }

    #[test]
    fn batch_mode_is_opt_in_so_interactive_auth_still_works() {
        let probe = ssh_args(&barn(None), Opts { batch: true, ..Opts::default() }).unwrap();
        assert!(probe.contains(&"BatchMode=yes".to_string()));

        let interactive = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(!interactive.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn requests_a_tty_only_when_asked() {
        let with = ssh_args(&barn(None), Opts { tty: true, ..Opts::default() }).unwrap();
        assert!(with.contains(&"-t".to_string()));

        let without = ssh_args(&barn(None), Opts::default()).unwrap();
        assert!(!without.contains(&"-t".to_string()));
    }

    #[test]
    fn rejects_a_barn_with_no_host() {
        let mut b = barn(None);
        b.host = None;
        assert!(ssh_args(&b, Opts::default()).is_err());
    }

    #[test]
    fn defaults_the_user_and_port_when_absent() {
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
