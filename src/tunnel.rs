//! The reverse tunnel this machine holds open to the Ranch House.
//!
//! # What the tunnel is for
//!
//! The Ranch House is a switchboard. Every other machine dials **out** to it
//! with `ssh -N -R <tunnel_port>:localhost:22 <house>`, which binds that port on
//! the house's loopback and forwards anything arriving there back down the same
//! outbound session to this machine's sshd. A peer then reaches this machine with
//! `ssh -J <house> -p <tunnel_port> <user>@localhost` — see [`crate::ssh::route`],
//! which builds exactly that and explains why none of it is published.
//!
//! Outbound is the whole trick. A machine behind NAT, on hotel wifi, or on a LAN
//! whose `.local` names resolve nowhere else can all dial out; none of them can
//! be dialled. `tunnel_port` is assigned once by the house at join time
//! ([`crate::ranch::assign_tunnel_port`]) and is stable across re-joins, so the
//! port a peer routes to is the port this machine binds.
//!
//! Which makes this module the half of the design without which the other half
//! does nothing: `ssh::route` will happily build a ProxyJump to a port that
//! nobody is holding open.
//!
//! # What holds it open
//!
//! One background thread per process, started once from [`crate::app::run`] and
//! living exactly as long as the process. Deliberately **not** tied to the TUI
//! being on screen, to the session grid, or to a client being attached: yeehaw
//! runs inside a detached tmux session, so process-scoped is in practice
//! always-on, and a tunnel that came and went with the foreground would make
//! "can I reach that machine" depend on what its owner was looking at.
//!
//! It does not outlive the process, and nothing here installs a launchd or
//! systemd unit. The tmux session is what survives a terminal closing.
//!
//! The plan is read **once**, at start, on the thread that starts it — see
//! [`Hold::args`] for why it cannot be read from the worker. The consequence is
//! worth stating plainly: a machine that runs `ranch join` while its TUI is up
//! does not get a tunnel until the process restarts. That is the right trade for
//! now — a worker that re-read the roster on every reconnect would be reading the
//! disk once a minute forever to catch an event that happens once in a machine's
//! life — but it is a real edge, and `ranch join` does not currently say so. The
//! seam to change it is one place: hand [`Tunnel::start_with`] a fresh plan.
//!
//! # Doing nothing is usually right
//!
//! Four states hold no tunnel and none of them is an error — see [`Idle`]. The
//! Ranch House in particular holds none: it is the thing everyone reaches.
//!
//! # Structure
//!
//! - [`plan`] decides, purely, whether a tunnel is wanted and what its argv is.
//! - [`Health`] is the bookkeeping: what the tunnel is doing, how many times it
//!   has failed in a row, and the last thing it said.
//! - [`Tunnel`] is the thread, the child, and the teardown.
//!
//! The spawn is injected ([`Tunnel::start_with`]), the same seam
//! [`crate::remote_grid::RemoteStreams::reconcile_with`] and
//! [`crate::remote_grid::RemoteStream::from_command`] open, and for the same
//! reason: every decision here is testable against local children with no barn
//! and no network in the suite.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::remote_grid::retry_delay;
use crate::ssh;
use crate::types::Barn;

/// How long a tunnel has to stay up before the next drop counts as a fresh
/// failure rather than another flap.
///
/// The backoff needs a notion of "that one worked", and unlike the session grid
/// this has no positive signal to use: `ssh -N` says nothing when it succeeds, so
/// there is no frame arriving to prove anything. Longevity is the only evidence
/// available, and a minute is long enough that nothing which merely reconnected
/// and fell over again can claim it.
///
/// See [`Health::down`] for what turns on it.
pub const HEALTHY: Duration = Duration::from_secs(60);

/// How long the supervisor sleeps between checks that it has been told to stop.
///
/// The backoff wait is slept in slices of this rather than in one go, so a quit
/// does not have to wait out a 60-second retry before the thread notices.
const STOP_CHECK: Duration = Duration::from_millis(100);

/// The longest a single line of ssh's complaining is kept. A tunnel can live for
/// days and the error is headed for a UI, not a log file.
const MAX_REASON: usize = 300;

/// Why this machine is holding no tunnel.
///
/// **Every one of these is a normal state**, not a failure, and none of them sets
/// [`Health::last_error`]. Most machines on most ranches sit in one of them: a
/// ranch with a single machine, the Ranch House itself, a laptop that has not
/// been enrolled yet. Reporting them as errors would put a permanent complaint
/// on screen about the setup working as designed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    /// Nobody has started the supervisor. This is what an [`crate::app::App`]
    /// that has been constructed but not run holds, and it is what keeps every
    /// test in this crate that builds an `App` from opening an ssh connection to
    /// the developer's own Ranch House. [`crate::app::run`] replaces it.
    NotStarted,
    /// This machine has no barn record of its own: it has never run `ranch init`
    /// or `ranch join`.
    NotAdopted,
    /// This machine *is* the Ranch House. It is the machine everyone else
    /// reaches, so it holds no tunnel — a tunnel to itself would be a forward
    /// from its own loopback to its own sshd.
    IsRanchHouse,
    /// Joined, but no `tunnel_port` was assigned — an older build's join, or a
    /// record stamped by hand. Only the house assigns ports
    /// ([`crate::ranch::assign_tunnel_port`]); a machine choosing its own is two
    /// machines colliding on one port with one tunnel silently shadowing the
    /// other.
    NoTunnelPort,
    /// This ranch has no Ranch House with an address of its own to dial. Both
    /// halves are one situation from here — "there is nowhere to dial out to" —
    /// so they share a variant rather than splitting a distinction no caller
    /// acts on differently.
    NoHouse,
}

/// What the supervisor is doing right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Holding nothing, on purpose. See [`Idle`].
    Idle(Idle),
    /// A child is running with the forward requested and accepted. Because
    /// `-o ExitOnForwardFailure=yes` is set, a child that is still alive is a
    /// forward that exists — which is the entire reason that option is not
    /// optional.
    Up { port: u16 },
    /// No child. Another attempt is due after [`Health::down`]'s wait.
    Down { port: u16 },
}

/// Everything the supervisor knows, and the only thing it publishes.
///
/// Read through [`Tunnel::health`] and [`Tunnel::last_error`].
// Read by this module's tests, by `app`'s, and by nothing in the shipping build
// yet. See `Tunnel::last_error` for why the state is kept anyway and who takes
// this off.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub status: Status,
    /// Consecutive failures with no run that lasted [`HEALTHY`] in between.
    ///
    /// Non-zero while `status` is [`Status::Up`] means the tunnel is *flapping*,
    /// and that is the state this field exists to make visible: a flap is `Up`
    /// about half the time, so the status alone reads as healthy at any given
    /// instant.
    pub attempts: u32,
    /// The last thing that went wrong — ssh's own final line where there was
    /// one. **Sticky:** it survives a reconnect, so one sample of a flapping
    /// tunnel still says what is wrong. `None` on a tunnel that has never failed.
    pub last_error: Option<String>,
}

/// A tunnel this machine should be holding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    /// The port the forward binds on the house's loopback.
    pub port: u16,
    /// The house's barn name. For messages; the argv is what is dialled.
    pub house: String,
    /// The full ssh argv, built once by [`plan`] on the thread that read the
    /// roster.
    ///
    /// Built there and not in the worker for a reason that is not style: the
    /// test harness points `~/.yeehaw` at a temp directory through a
    /// *thread-local*, which a spawned thread does not inherit
    /// ([`crate::testing::with_temp_ranch`]). A worker that read the roster
    /// itself would panic in the suite — and would be reading the developer's
    /// real ranch if it ever did not.
    pub args: Vec<String>,
}

/// What this machine should do about its own reverse tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Hold(Hold),
    Idle(Idle),
}

impl Health {
    fn idle(why: Idle) -> Self {
        Health { status: Status::Idle(why), attempts: 0, last_error: None }
    }

    /// Before the first attempt: no child yet, nothing has failed.
    fn waiting(port: u16) -> Self {
        Health { status: Status::Down { port }, attempts: 0, last_error: None }
    }

    /// A child is up with the forward accepted.
    ///
    /// **It does not clear `attempts`, and it does not clear `last_error`.** That
    /// is the load-bearing line in this file. A tunnel that connects, dies two
    /// seconds later, connects again and dies again is the most likely failure
    /// this supervisor will ever meet — a house whose port is still held by a
    /// previous run, a flaky link, a key that authenticates and is then refused
    /// by the forward. If coming up reset the count, the backoff would restart
    /// from [`crate::remote_grid::RETRY_BASE`] on every cycle and the machine
    /// would spend the rest of the day opening ssh connections two seconds apart
    /// with nobody told anything. Only *surviving* clears the count, in
    /// [`Self::down`].
    fn up(&mut self, port: u16) {
        self.status = Status::Up { port };
    }

    /// The tunnel ended. Returns how long to wait before trying again.
    ///
    /// `lived` is how long the attempt lasted, and it is the only evidence
    /// available that an attempt worked (see [`HEALTHY`]). A run that cleared the
    /// bar is a tunnel that genuinely worked and then dropped — a lid closing, a
    /// reboot, a wifi handover — and it must come back as promptly as the first
    /// attempt did rather than inherit an hour-old backoff. A run that did not is
    /// another flap, and the wait doubles.
    ///
    /// The doubling is [`crate::remote_grid::retry_delay`] rather than a second
    /// schedule of its own: the two failures it has to serve are the same two the
    /// session grid's backoff serves, and two curves that disagree are two things
    /// to reason about when a ranch is misbehaving.
    fn down(&mut self, port: u16, lived: Duration, error: String) -> Duration {
        self.attempts = if lived >= HEALTHY { 1 } else { self.attempts.saturating_add(1) };
        self.status = Status::Down { port };
        self.last_error = Some(error);
        retry_delay(self.attempts)
    }
}

/// Whether this machine should hold a tunnel, and what it should run.
///
/// Pure. `this` is this machine's own barn record and `barns` the roster; both
/// are read from disk exactly once, by [`plan_from_disk`].
///
/// The order of the checks is the order of the reasons, and it matters: a Ranch
/// House record that somehow carries a `tunnel_port` has to report
/// [`Idle::IsRanchHouse`] rather than be talked into dialling itself, so the
/// house check comes before the port.
///
/// The house is matched by flag **and** by name, mirroring [`ssh::route`]'s
/// double guard: this machine's own copy of its record may not carry
/// `is_ranch_house` yet while the roster's copy does, and the house dialling a
/// tunnel to itself is a forward from its loopback to its own sshd that shadows
/// the port for everyone else.
pub fn plan(this: Option<&Barn>, barns: &[Barn]) -> Plan {
    let this = match this {
        Some(this) => this,
        None => return Plan::Idle(Idle::NotAdopted),
    };
    if this.is_ranch_house == Some(true) {
        return Plan::Idle(Idle::IsRanchHouse);
    }
    let port = match this.tunnel_port {
        Some(port) => port,
        None => return Plan::Idle(Idle::NoTunnelPort),
    };
    let house = barns.iter().find(|b| b.is_ranch_house == Some(true) && b.name != this.name);
    // A house with no address of its own is the same situation as no house at
    // all: there is nothing to point `ssh -R` at. `tunnel_args` is what decides
    // that, because it is what would have to build the destination.
    let hold = house.and_then(|house| {
        ssh::tunnel_args(house, port)
            .map(|args| Hold { port, house: house.name.clone(), args })
    });
    match hold {
        Some(hold) => Plan::Hold(hold),
        None => Plan::Idle(Idle::NoHouse),
    }
}

/// [`plan`] against the ranch on disk.
///
/// `manifest::barns_from_disk`, not `config::load_barns()`: that injects a
/// synthetic `local` barn and drops a real `barns/local.yaml`, and neither
/// belongs in an answer to "which barn is the house" — the same reason
/// [`ssh::route`]'s own roster read gives.
fn plan_from_disk() -> Plan {
    plan(
        crate::ranch::this_machine_barn().as_ref(),
        &crate::ranch::manifest::barns_from_disk().items,
    )
}

/// The supervisor: one thread, one child at a time, for the life of the process.
///
/// Dropping it is the teardown; see [`Drop`].
pub struct Tunnel {
    /// Published state. The worker writes it, the UI reads it.
    health: Arc<Mutex<Health>>,
    /// The running child, when there is one.
    ///
    /// Shared, and the sharing is the design: [`Self::shutdown`] has to be able
    /// to reach the child while the worker is blocked reading its stderr, so the
    /// worker never holds this lock across a blocking call. `remote_grid` needs
    /// nothing like it because a `RemoteStream` owns its child outright and never
    /// replaces it — this one reconnects.
    child: Arc<Mutex<Option<Child>>>,
    /// Set before the kill, so the worker can tell "we shut this down" from "the
    /// tunnel died". Without it every quit records a tunnel failure, and the last
    /// error the UI could reach would always be the last teardown.
    stopping: Arc<AtomicBool>,
}

impl Tunnel {
    /// A supervisor that is not running and never will be.
    ///
    /// What [`crate::app::App::new`] holds. Constructing an `App` must not open
    /// an ssh connection: `App::new` is called by a dozen tests in this crate,
    /// and one whose temp ranch happened to name a Ranch House would have them
    /// dialling out for real. [`crate::app::run`] — which no test calls — is
    /// where the real one starts.
    pub fn dormant() -> Self {
        Tunnel::idle(Idle::NotStarted)
    }

    fn idle(why: Idle) -> Self {
        Tunnel {
            health: Arc::new(Mutex::new(Health::idle(why))),
            child: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Read the ranch, and hold a tunnel if one is wanted. Returns at once; the
    /// work is on a background thread, because the event loop has 250ms to do
    /// everything in and an ssh handshake takes longer than that on its own.
    pub fn start() -> Self {
        Self::start_with(plan_from_disk(), spawn_ssh)
    }

    /// [`Self::start`] with the plan and the spawn injected.
    ///
    /// The seam. `start` does nothing but read the roster and name the real ssh
    /// spawner; nothing past this point knows or cares that the child is ssh —
    /// which is what lets the reconnect, the backoff and the teardown be tested
    /// against local children, with no barn and no network in the suite.
    ///
    /// The spawner returns a [`Command`], not a [`Child`]: the caller owns
    /// everything about the command except its stdio, which is fixed in
    /// [`start_child`] because getting it wrong is a bug in every caller equally.
    /// Same contract as [`crate::remote_grid::RemoteStream::from_command`].
    pub(crate) fn start_with<S>(plan: Plan, spawn: S) -> Self
    where
        S: Fn(&Hold) -> Result<Command> + Send + 'static,
    {
        let hold = match plan {
            Plan::Idle(why) => return Tunnel::idle(why),
            Plan::Hold(hold) => hold,
        };

        let tunnel = Tunnel {
            health: Arc::new(Mutex::new(Health::waiting(hold.port))),
            child: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
        };

        // The handle is not kept and `Drop` deliberately does not join, for the
        // reason `remote_grid` does not either: the worker spends its life either
        // blocked on a read or asleep in the backoff, so a join would be a wait
        // of up to `RETRY_MAX` on the way out. Setting `stopping` and killing the
        // child is what ends it.
        let health = Arc::clone(&tunnel.health);
        let child = Arc::clone(&tunnel.child);
        let stopping = Arc::clone(&tunnel.stopping);
        thread::spawn(move || supervise(hold, spawn, health, child, stopping));

        tunnel
    }

    /// Everything the supervisor knows right now. Same story as
    /// [`Self::last_error`]: kept, tested, not yet rendered.
    #[allow(dead_code)]
    pub fn health(&self) -> Health {
        self.health.lock().expect("the tunnel's health lock").clone()
    }

    /// The last thing that went wrong, whatever the tunnel is doing now.
    ///
    /// This is the "fail loudly" half of the design, and it is the *whole* of it
    /// on purpose. A flapping tunnel is `Up` about half the time, so an error
    /// cleared on reconnect would be invisible exactly when it mattered — see
    /// [`Health::last_error`], which is why it is sticky.
    ///
    /// **Nothing renders it yet, and that is deliberate.** This pass was scoped
    /// to keeping the error *retrievable*: no health panel, no badge, no manual
    /// reconnect. Shipping the accessor with no caller is the honest version of
    /// that — the state exists, one place owns it, and whatever first shows it
    /// (the dashboard's error line is the obvious candidate) has one thing to
    /// call and takes the `allow` below off. The alternative was to not keep the
    /// error at all, and then a flapping tunnel really is a silent loop.
    #[allow(dead_code)]
    pub fn last_error(&self) -> Option<String> {
        self.health.lock().expect("the tunnel's health lock").last_error.clone()
    }

    /// Stop holding the tunnel: kill **and** reap the child, and tell the worker
    /// not to start another.
    ///
    /// Idempotent, and called twice on purpose. [`Drop`] is the normal teardown,
    /// but the quit paths cannot rely on it — `tmux kill-session` and
    /// `respawn-window -k` end or replace this process without unwinding, so
    /// nothing's `Drop` runs. Those paths call this explicitly, exactly as they
    /// already call [`crate::remote_grid::RemoteStreams::shutdown`].
    ///
    /// `stopping` is set **before** the kill. Reversed, the worker sees its child
    /// vanish, calls it a death and records it — so every quit would leave "the
    /// tunnel to 'imac' ended" as the last error, and a real failure would be
    /// indistinguishable from a normal exit.
    pub fn shutdown(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        reap(self.child.lock().expect("the tunnel's child lock").take());
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Kill and reap, or do nothing.
///
/// `kill` without `wait` leaves a `<defunct>` ssh behind, and this supervisor
/// *reconnects* — so a missing `wait` is not one zombie, it is one per reconnect
/// for the life of the process. A tunnel that flaps every few seconds would fill
/// the process table on its own.
fn reap(child: Option<Child>) {
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The worker: hold the tunnel, and when it ends, hold it again.
///
/// One child at a time, with no upper bound on attempts — a machine that cannot
/// reach the house right now may be able to in an hour, and giving up would need
/// a manual reconnect this design deliberately does not have. What stops it being
/// a silent spin is the backoff ([`Health::down`]) and the fact that every failure
/// is published where the UI can read it.
fn supervise<S>(
    hold: Hold,
    spawn: S,
    health: Arc<Mutex<Health>>,
    slot: Arc<Mutex<Option<Child>>>,
    stopping: Arc<AtomicBool>,
) where
    S: Fn(&Hold) -> Result<Command>,
{
    while !stopping.load(Ordering::SeqCst) {
        let started = Instant::now();

        let wait = match start_child(&hold, &spawn) {
            // Could not even start ssh. The same kind of failure as any other and
            // it lands in the same place — otherwise it is the one failure that
            // loops in silence.
            Err(e) => publish(&health).down(hold.port, Duration::ZERO, e.to_string()),
            Ok((child, stderr)) => {
                *slot.lock().expect("the tunnel's child lock") = Some(child);
                publish(&health).up(hold.port);

                // Blocking, and with no lock held: `shutdown` has to be able to
                // reach the child while this waits. EOF on stderr is how the end
                // of the tunnel is noticed — there is no remote command here, so
                // no forked subshell can hold the pipe open past the child's
                // death the way `remote_grid`'s can.
                let said = last_complaint(stderr);

                // Reap whatever is left, whether ssh exited on its own or is
                // wedged with its stderr closed.
                reap(slot.lock().expect("the tunnel's child lock").take());

                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                publish(&health).down(hold.port, started.elapsed(), reason(&hold.house, said))
            }
        };

        if !nap(wait, &stopping) {
            return;
        }
    }
}

/// The published state, for one update.
///
/// A function rather than a `.lock()` spelled out at four call sites, so the
/// panic message is the same at all of them and no caller is tempted to hold the
/// guard across anything.
fn publish(health: &Arc<Mutex<Health>>) -> std::sync::MutexGuard<'_, Health> {
    health.lock().expect("the tunnel's health lock")
}

/// Build the command, fix its stdio, and start it.
///
/// - **stdin null**: ssh forwards its own stdin to the far end, and a child that
///   inherited ours would sit reading the terminal and race crossterm for every
///   keystroke aimed at the TUI.
/// - **stdout null**: `-N` runs no remote command, so there is nothing on it.
/// - **stderr piped**: this is both how the end of the tunnel is noticed and the
///   only place ssh says *why*. Inherited, it would paint straight over the
///   rendered frame.
fn start_child<S>(hold: &Hold, spawn: &S) -> Result<(Child, ChildStderr)>
where
    S: Fn(&Hold) -> Result<Command>,
{
    let mut cmd = spawn(hold)?;
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("failed to start the tunnel to '{}': {}", hold.house, e))?;
    let stderr = child.stderr.take().expect("stderr was just set to a pipe");
    Ok((child, stderr))
}

/// Read the child's stderr to EOF and keep the last thing it said.
///
/// The *last* non-blank line, and only that one. ssh's final line is its verdict
/// — "Error: remote port forwarding failed for listen port 23007", "Timeout,
/// server imac not responding", "Permission denied (publickey)" — and what comes
/// before it is banner and debug. Keeping the whole stream would mean buffering
/// days of a healthy tunnel's output for a string headed to a status line, so
/// each line is truncated to [`MAX_REASON`] and only one is kept.
///
/// Lines that are not valid UTF-8 are skipped rather than ending the read: a
/// barn's MOTD can contain anything, and giving up on the first bad byte would
/// throw away the verdict that came after it.
fn last_complaint(stderr: ChildStderr) -> Option<String> {
    let mut last: Option<String> = None;
    for line in BufReader::new(stderr).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        last = Some(line.chars().take(MAX_REASON).collect());
    }
    last
}

/// What to report for a tunnel that ended.
///
/// An empty message reads in a UI as a silent success, which is the one thing a
/// dead tunnel must never look like — so a child that said nothing still gets a
/// sentence, and the sentence names the house so it is actionable.
fn reason(house: &str, said: Option<String>) -> String {
    said.unwrap_or_else(|| format!("the tunnel to '{}' ended with nothing on stderr", house))
}

/// Sleep out the backoff. `false` means stop instead.
///
/// Sliced rather than slept in one go: the wait reaches
/// [`crate::remote_grid::RETRY_MAX`], and a quit that had to sit out a minute of
/// it would be a quit that looks hung.
fn nap(wait: Duration, stopping: &AtomicBool) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if stopping.load(Ordering::SeqCst) {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        thread::sleep(STOP_CHECK.min(left));
    }
}

/// The real spawner: the argv [`plan`] built, handed to `ssh`.
///
/// No shell anywhere on the local side, so nothing in the house's host, user or
/// key path can be injected.
fn spawn_ssh(hold: &Hold) -> Result<Command> {
    let mut cmd = Command::new("ssh");
    cmd.args(&hold.args);
    Ok(cmd)
}

// ===========================================================================

/// Test-only.
///
/// `pub(crate)` for the same reason [`crate::remote_grid::tests`] is: `app`'s own
/// quit-path tests drive a **real** supervisor over a local child, and they have
/// to build and inspect it the way this module does rather than hand-rolling a
/// second spawn.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::remote_grid::tests::{local_child, poll_until, process_state, GroupGuard};
    use crate::remote_grid::{retry_delay, RETRY_MAX};
    use crate::ssh;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The pid of the child the supervisor is holding, if any.
    ///
    /// A helper here rather than a method on `Tunnel`: the only caller is this
    /// module's own teardown assertions, and a test-only accessor on the type
    /// would be production surface that exists for the suite. `remote_grid`'s
    /// tests reach into `stream.child` the same way, for the same reason.
    pub(crate) fn child_pid(t: &Tunnel) -> Option<i32> {
        t.child.lock().expect("the tunnel's child lock").as_ref().map(|c| c.id() as i32)
    }

    /// Nothing in this module may open an ssh connection, and the fixtures are
    /// built so that nothing *could*. The house's address is under `.invalid`,
    /// the one TLD RFC 2606 guarantees never resolves, and every supervisor test
    /// injects its own spawner — the real `spawn_ssh` is never reached. See
    /// `the_house_in_these_fixtures_is_not_a_machine_anybody_owns` for the
    /// control.
    const NOWHERE: &str = "house.invalid";

    fn a_house() -> Barn {
        Barn {
            name: "imac".into(),
            host: Some(NOWHERE.into()),
            user: Some("cam".into()),
            is_ranch_house: Some(true),
            ..Default::default()
        }
    }

    /// This machine: joined, given a tunnel port, and with nothing to dial
    /// itself with — which is what every machine's own record looks like.
    fn me() -> Barn {
        Barn {
            name: "pi".into(),
            host: None,
            user: Some("cam".into()),
            tunnel_port: Some(23007),
            ..Default::default()
        }
    }

    /// THE CONTROL. If a fixture ever named a machine the user actually owns, a
    /// bug in the injection seam would put a real `ssh -R` on their ranch. The
    /// house is only ever reachable through `dial_host`, so pinning that is
    /// pinning the blast radius.
    #[test]
    fn the_house_in_these_fixtures_is_not_a_machine_anybody_owns() {
        assert_eq!(ssh::dial_host(&a_house()), Some(NOWHERE));
        assert!(NOWHERE.ends_with(".invalid"), "{} must be unresolvable", NOWHERE);
        // And this machine's own record has nothing to dial at all, so even a
        // route built from it cannot reach anywhere.
        assert!(ssh::dial_host(&me()).is_none(), "the fixture must advertise nothing");
    }

    // === the plan ==========================================================
    //
    // Four ways to be holding no tunnel, and every one of them is a normal
    // state a healthy machine sits in. None is an error, so none sets
    // `last_error`.

    #[test]
    fn a_joined_machine_holds_a_tunnel_to_the_house() {
        let house = a_house();
        match plan(Some(&me()), &[house.clone(), me()]) {
            Plan::Hold(hold) => {
                assert_eq!(hold.port, 23007);
                assert_eq!(hold.house, "imac");
                assert_eq!(
                    hold.args,
                    ssh::tunnel_args(&house, 23007).expect("the house is dialable"),
                    "the plan carries the argv it will be run with, built once"
                );
            }
            other => panic!("expected a hold, got {:?}", other),
        }
    }

    /// The house is the thing everyone reaches. It holds no tunnel of its own,
    /// and this is checked before the port so that a house which somehow has one
    /// still says the right reason.
    #[test]
    fn the_ranch_house_holds_no_tunnel_of_its_own() {
        let mut house = a_house();
        house.tunnel_port = Some(23000);
        assert_eq!(plan(Some(&house), &[house.clone()]), Plan::Idle(Idle::IsRanchHouse));
    }

    /// No port means the house has not given this machine one — it has not
    /// joined, or joined an older build. Nothing to bind, so nothing to hold.
    #[test]
    fn a_machine_with_no_tunnel_port_holds_nothing() {
        let mut me = me();
        me.tunnel_port = None;
        let roster = [a_house(), me.clone()];
        assert_eq!(plan(Some(&me), &roster), Plan::Idle(Idle::NoTunnelPort));
    }

    /// A ranch with no house has no switchboard to dial out to.
    #[test]
    fn a_ranch_with_no_house_holds_nothing() {
        assert_eq!(plan(Some(&me()), &[me()]), Plan::Idle(Idle::NoHouse));
    }

    /// And a house with no address of its own is the same case: there is nothing
    /// to point `ssh -R` at. Grouped with `NoHouse` deliberately — from this
    /// machine's side "the ranch has no reachable house" is one situation.
    #[test]
    fn a_house_with_no_address_of_its_own_is_not_a_tunnel_target() {
        let mut house = a_house();
        house.host = None;
        assert!(house.addresses.is_empty(), "the fixture must advertise nothing");
        assert!(ssh::dial_host(&house).is_none());
        assert_eq!(plan(Some(&me()), &[house, me()]), Plan::Idle(Idle::NoHouse));
    }

    /// A machine that has never run `ranch init` or `ranch join` has no record
    /// of its own. Not an error either: it is every machine before enrollment.
    #[test]
    fn a_machine_that_has_never_joined_holds_nothing() {
        assert_eq!(plan(None, &[a_house()]), Plan::Idle(Idle::NotAdopted));
    }

    /// The second guard, mirroring `ssh::route`'s: this machine's *own* copy of
    /// its record may not carry `is_ranch_house` yet while the roster's copy
    /// does. Matched by name, or the house would dial a tunnel to itself.
    #[test]
    fn the_house_is_never_its_own_tunnel_target_by_name() {
        let mut mine = a_house();
        mine.is_ranch_house = None;
        mine.tunnel_port = Some(23000);
        assert_eq!(plan(Some(&mine), &[a_house()]), Plan::Idle(Idle::NoHouse));
    }

    // === the argv ==========================================================
    //
    // Read, never run. Every one of these options was chosen against a specific
    // failure; the test names say which.

    fn args() -> Vec<String> {
        ssh::tunnel_args(&a_house(), 23007).expect("the house is dialable")
    }

    fn opt(args: &[String], key: &str) -> Option<String> {
        args.iter().find(|a| a.starts_with(&format!("{}=", key))).cloned()
    }

    /// THE MANDATORY ONE. Without it, ssh connects happily when something else
    /// already holds the port on the house, the forward silently does not exist,
    /// and the supervisor sits there reporting a healthy tunnel that routes
    /// nothing. A tunnel that looks up and is not is the worst state available.
    #[test]
    fn the_tunnel_exits_rather_than_pretending_a_bound_port_is_a_forward() {
        assert_eq!(opt(&args(), "ExitOnForwardFailure").as_deref(), Some("ExitOnForwardFailure=yes"));
    }

    /// The forward itself: `<tunnel_port>` on the house's loopback, back to this
    /// machine's own sshd. `localhost` is resolved on the house, and with no
    /// `GatewayPorts` there the listener is loopback-only — which is why nothing
    /// is published by any of this.
    #[test]
    fn the_forward_binds_the_tunnel_port_on_the_houses_loopback() {
        let args = args();
        let r = args.iter().position(|a| a == "-R").expect("-R present");
        assert_eq!(args[r + 1], "23007:localhost:22", "{:?}", args);
    }

    /// `-N`: no remote command, no shell, no pty. The connection exists for the
    /// forward and nothing else.
    #[test]
    fn the_tunnel_runs_no_remote_command() {
        let args = args();
        assert!(args.contains(&"-N".to_string()), "{:?}", args);
        assert!(!args.contains(&"-t".to_string()), "no tty is wanted: {:?}", args);
    }

    /// Keepalives, and the *same* ones every other connection uses. Without them
    /// a tunnel whose network went away hangs rather than failing, and a hung
    /// tunnel is indistinguishable from a working one — so nothing reconnects it
    /// and the barn is quietly unreachable for as long as yeehaw runs.
    #[test]
    fn the_tunnel_carries_the_same_keepalives_every_other_connection_does() {
        let args = args();
        assert_eq!(
            opt(&args, "ServerAliveInterval"),
            Some(format!("ServerAliveInterval={}", ssh::SERVER_ALIVE_INTERVAL_SECS))
        );
        assert_eq!(
            opt(&args, "ServerAliveCountMax"),
            Some(format!("ServerAliveCountMax={}", ssh::SERVER_ALIVE_COUNT_MAX))
        );

        // And they are not a second opinion: `ssh_args` sets the same numbers.
        let _ranch = crate::testing::temp_ranch();
        let probe = ssh::ssh_args(&a_house(), ssh::Opts { batch: true, ..Default::default() })
            .expect("the house is dialable");
        assert_eq!(opt(&args, "ServerAliveInterval"), opt(&probe, "ServerAliveInterval"));
        assert_eq!(opt(&args, "ServerAliveCountMax"), opt(&probe, "ServerAliveCountMax"));
    }

    /// A tunnel is spawned from a background thread behind a full-screen TUI.
    /// A password prompt there is a prompt nobody can see and nobody will
    /// answer — so it must fail instead.
    #[test]
    fn the_tunnel_fails_rather_than_waiting_on_a_prompt_nobody_can_see() {
        assert_eq!(opt(&args(), "BatchMode").as_deref(), Some("BatchMode=yes"));
    }

    /// Never multiplexed. `ssh_args` shares a `ControlMaster` for the short
    /// connections, and the tunnel must be in neither half of that: as a master
    /// it would take every probe down with it when it reconnects, and as a
    /// client it would die whenever the master it borrowed went away. It is a
    /// connection of its own for the life of the process.
    #[test]
    fn the_tunnel_is_never_multiplexed_with_anything_else() {
        let args = args();
        assert_eq!(opt(&args, "ControlMaster").as_deref(), Some("ControlMaster=no"));
        assert_eq!(opt(&args, "ControlPath").as_deref(), Some("ControlPath=none"));
        assert!(
            !args.iter().any(|a| a.starts_with("ControlPersist")),
            "nothing to persist: {:?}",
            args
        );
    }

    /// The outbound hop is an ordinary ssh to the house, so it takes the house's
    /// own user, port, key and host-key policy.
    #[test]
    fn the_tunnel_dials_the_houses_own_user_port_and_key() {
        let mut house = a_house();
        house.port = Some(2022);
        house.identity_file = Some("~/.ssh/id_ranch".into());
        let args = ssh::tunnel_args(&house, 23007).unwrap();

        assert_eq!(args.last().map(String::as_str), Some("cam@house.invalid"), "{:?}", args);
        let p = args.iter().position(|a| a == "-p").expect("-p present");
        assert_eq!(args[p + 1], "2022", "{:?}", args);
        let i = args.iter().position(|a| a == "-i").expect("-i present");
        assert_eq!(args[i + 1], "~/.ssh/id_ranch", "{:?}", args);
        assert_eq!(
            opt(&args, "StrictHostKeyChecking").as_deref(),
            Some("StrictHostKeyChecking=accept-new")
        );
        assert!(args.iter().any(|a| a.starts_with("ConnectTimeout=")), "{:?}", args);
    }

    #[test]
    fn a_house_with_no_user_is_dialled_as_whoever_ssh_defaults_to() {
        let mut house = a_house();
        house.user = None;
        let args = ssh::tunnel_args(&house, 23007).unwrap();
        assert_eq!(args.last().map(String::as_str), Some("house.invalid"), "{:?}", args);
        assert_eq!(args.iter().filter(|a| a.contains('@')).count(), 0, "{:?}", args);
    }

    #[test]
    fn a_house_with_nothing_to_dial_has_no_argv() {
        let mut house = a_house();
        house.host = None;
        assert_eq!(ssh::tunnel_args(&house, 23007), None);
    }

    // === the ledger ========================================================
    //
    // The bookkeeping, driven directly. This is where "a flapping tunnel must
    // fail loudly rather than reconnect-loop silently" is actually decided, so
    // it is pinned without a process anywhere near it.

    #[test]
    fn the_backoff_is_the_shape_the_session_grid_already_uses() {
        let mut h = Health::waiting(23007);
        for n in 1..=8u32 {
            assert_eq!(
                h.down(23007, Duration::ZERO, "gone".into()),
                retry_delay(n),
                "the {}th consecutive failure",
                n
            );
        }
        for _ in 0..40 {
            h.down(23007, Duration::ZERO, "gone".into());
        }
        assert_eq!(h.down(23007, Duration::ZERO, "gone".into()), RETRY_MAX, "the cap is the cap");
    }

    /// THE ONE THAT MATTERS. A tunnel that connects, dies two seconds later,
    /// connects again and dies again is the failure this whole supervisor is
    /// most likely to meet — a house whose port is already held, a flaky link, a
    /// key that authenticates and then gets refused. Coming *up* must not clear
    /// the count, or the backoff resets on every cycle and the machine spends
    /// the rest of the day opening ssh connections a second apart with nobody
    /// told.
    #[test]
    fn coming_up_briefly_does_not_reset_the_backoff() {
        let mut h = Health::waiting(23007);
        let mut waits = Vec::new();
        for _ in 0..4 {
            h.up(23007);
            assert_eq!(h.status, Status::Up { port: 23007 });
            waits.push(h.down(23007, Duration::from_secs(2), "forwarding failed".into()));
        }
        assert_eq!(
            waits,
            vec![retry_delay(1), retry_delay(2), retry_delay(3), retry_delay(4)],
            "a flapping tunnel has to escalate: {:?}",
            waits
        );
        assert_eq!(h.attempts, 4, "the flap count is what makes this legible at all");
    }

    /// The other half of the same rule. A tunnel that genuinely worked for a
    /// while and then dropped — a lid closing, a reboot — is not flapping, and
    /// must come back as promptly as the first attempt did rather than inheriting
    /// an hour-old backoff.
    #[test]
    fn a_tunnel_that_held_for_long_enough_retries_promptly_when_it_finally_drops() {
        let mut h = Health::waiting(23007);
        for _ in 0..6 {
            h.up(23007);
            h.down(23007, Duration::ZERO, "gone".into());
        }
        assert!(h.attempts > 1, "control: the backoff has grown");

        h.up(23007);
        assert_eq!(h.down(23007, HEALTHY, "gone".into()), retry_delay(1));
        assert_eq!(h.attempts, 1);
    }

    /// The error is what a UI has to be able to show, and a flapping tunnel is
    /// `Up` about half the time — so an error that vanished the moment the next
    /// attempt connected would be invisible exactly when it is needed.
    #[test]
    fn the_last_error_survives_a_reconnect() {
        let mut h = Health::waiting(23007);
        assert_eq!(h.last_error, None, "a tunnel that has not failed has nothing to report");

        h.down(23007, Duration::ZERO, "remote port forwarding failed".into());
        assert_eq!(h.last_error.as_deref(), Some("remote port forwarding failed"));

        h.up(23007);
        assert_eq!(
            h.last_error.as_deref(),
            Some("remote port forwarding failed"),
            "the reconnect swallowed the reason it had to reconnect"
        );
    }

    // === the supervisor ====================================================
    //
    // Threads and children, never ssh. The spawner is injected in every one of
    // these, exactly as `RemoteStreams::reconcile_with` injects its own.

    /// A spawner that records what it was asked for and hands back a local
    /// child instead.
    fn recording(log: &Arc<Mutex<Vec<Vec<String>>>>, script: &'static str)
        -> impl Fn(&Hold) -> Result<Command> + Send + 'static
    {
        let log = Arc::clone(log);
        move |hold: &Hold| {
            log.lock().unwrap().push(hold.args.clone());
            Ok(local_child(script))
        }
    }

    fn calls(log: &Arc<Mutex<Vec<Vec<String>>>>) -> usize {
        log.lock().unwrap().len()
    }

    /// Every idle state is a normal one, so nothing is spawned and nothing is
    /// reported as wrong. The spawner panics: "did not spawn" and "spawned and
    /// threw it away" are an ssh connection apart.
    #[test]
    fn an_idle_plan_never_spawns_anything() {
        for why in [Idle::NotAdopted, Idle::IsRanchHouse, Idle::NoTunnelPort, Idle::NoHouse] {
            let tunnel = Tunnel::start_with(Plan::Idle(why), |_| -> Result<Command> {
                panic!("an idle plan tried to open a tunnel")
            });
            let health = tunnel.health();
            assert_eq!(health.status, Status::Idle(why));
            assert_eq!(health.last_error, None, "{:?} is a normal state, not an error", why);
            assert_eq!(health.attempts, 0);
        }
    }

    /// An `App` that has been built but not run holds no tunnel. This is what
    /// keeps every test in this crate that constructs an `App` from opening an
    /// ssh connection to the developer's own Ranch House.
    #[test]
    fn a_dormant_tunnel_is_idle_and_holds_nothing() {
        let tunnel = Tunnel::dormant();
        assert_eq!(tunnel.health().status, Status::Idle(Idle::NotStarted));
        assert_eq!(tunnel.last_error(), None);
    }

    /// A running supervisor whose child is a local `sleep`, not ssh.
    ///
    /// `pub(crate)` for `app`'s quit-path tests. The caller must put the returned
    /// pid under a [`GroupGuard`] and tear the tunnel down, or the `sleep`
    /// outlives the test.
    pub(crate) fn a_local_tunnel() -> Tunnel {
        let log = Arc::new(Mutex::new(Vec::new()));
        let tunnel = Tunnel::start_with(a_hold(), recording(&log, "exec sleep 300"));
        assert!(
            poll_until(Duration::from_secs(5), || child_pid(&tunnel).is_some()),
            "the fixture's child never started"
        );
        tunnel
    }

    fn a_hold() -> Plan {
        Plan::Hold(Hold {
            port: 23007,
            house: "imac".into(),
            args: ssh::tunnel_args(&a_house(), 23007).unwrap(),
        })
    }

    #[test]
    fn a_tunnel_that_holds_reports_up_and_is_spawned_exactly_once() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let tunnel = Tunnel::start_with(a_hold(), recording(&log, "exec sleep 300"));

        assert!(
            poll_until(Duration::from_secs(5), || tunnel.health().status
                == Status::Up { port: 23007 }),
            "the tunnel never came up: {:?}",
            tunnel.health()
        );
        let pid = child_pid(&tunnel).expect("a child is held while up");
        let _group = GroupGuard(pid);

        assert_eq!(calls(&log), 1, "a live tunnel must not be respawned");
        assert_eq!(
            log.lock().unwrap()[0],
            ssh::tunnel_args(&a_house(), 23007).unwrap(),
            "the argv the plan carried is the argv that ran"
        );
        assert_eq!(tunnel.last_error(), None);
    }

    /// The flap, end to end and with no ssh in it: a child that writes to stderr
    /// and exits is exactly the shape `ExitOnForwardFailure` gives a house whose
    /// port is already held.
    #[test]
    fn a_tunnel_that_dies_is_recorded_with_what_it_said() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let tunnel = Tunnel::start_with(
            a_hold(),
            recording(
                &log,
                "echo 'Error: remote port forwarding failed for listen port 23007' >&2; exit 255",
            ),
        );

        assert!(
            poll_until(Duration::from_secs(5), || tunnel.last_error().is_some()),
            "the failure was never recorded: {:?}",
            tunnel.health()
        );
        let health = tunnel.health();
        assert_eq!(health.status, Status::Down { port: 23007 });
        assert!(
            health.last_error.as_deref().is_some_and(|e| e.contains("forwarding failed")),
            "the reason ssh gave has to survive to the UI: {:?}",
            health.last_error
        );
        assert_eq!(health.attempts, 1, "one failure so far");
    }

    /// A tunnel that cannot even be started is a failure like any other, and it
    /// has to land in the same place — otherwise it is the one failure that
    /// loops in silence.
    #[test]
    fn a_tunnel_that_cannot_even_be_started_is_recorded_rather_than_looping_silently() {
        let tries = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&tries);
        let tunnel = Tunnel::start_with(a_hold(), move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!("no such file or directory: ssh"))
        });

        assert!(
            poll_until(Duration::from_secs(5), || tunnel.last_error().is_some()),
            "a spawn that failed was never recorded"
        );
        assert!(
            tunnel.last_error().as_deref().is_some_and(|e| e.contains("ssh")),
            "{:?}",
            tunnel.last_error()
        );
        assert_eq!(tunnel.health().attempts, 1);
        assert_eq!(tries.load(Ordering::SeqCst), 1, "the retry must wait, not spin");
    }

    /// A tunnel that exits with nothing on stderr must still say something. An
    /// empty message reads in the UI as a silent success, which is the one thing
    /// a dead tunnel must never look like.
    #[test]
    fn a_failure_is_never_an_empty_message() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let tunnel = Tunnel::start_with(a_hold(), recording(&log, "exit 1"));

        assert!(
            poll_until(Duration::from_secs(5), || tunnel.last_error().is_some()),
            "{:?}",
            tunnel.health()
        );
        let said = tunnel.last_error().unwrap();
        assert!(!said.trim().is_empty(), "an empty reason is not a reason");
        assert!(said.contains("imac"), "and it names the house it could not hold: {}", said);
    }

    /// Kill **and** reap. `kill` alone leaves a `<defunct>` ssh behind, and this
    /// supervisor reconnects — so a missing `wait` is not one zombie, it is one
    /// per reconnect for the life of the process.
    #[test]
    fn dropping_the_tunnel_kills_the_child_and_leaves_no_zombie() {
        let tunnel = a_local_tunnel();
        let pid = child_pid(&tunnel).expect("the fixture holds a child");
        let _group = GroupGuard(pid);
        assert!(process_state(pid).is_some(), "control: the child is alive before the drop");

        drop(tunnel);

        let state = process_state(pid);
        assert!(
            state.is_none(),
            "the child survived the drop as {state:?} — 'Z' means it was killed but never reaped"
        );
    }

    /// The teardown is the shutdown, and it must be sayable twice: the quit path
    /// calls it explicitly because `tmux kill-session` takes this process out
    /// before any `Drop` could run, and then `Drop` runs anyway on the paths
    /// where it can.
    #[test]
    fn shutting_down_twice_is_not_an_error() {
        let mut tunnel = a_local_tunnel();
        let pid = child_pid(&tunnel).expect("the fixture holds a child");
        let _group = GroupGuard(pid);

        tunnel.shutdown();
        tunnel.shutdown();
        assert_eq!(process_state(pid), None);
        assert!(child_pid(&tunnel).is_none(), "the shutdown left a child behind");
        drop(tunnel);
    }

    /// A shutdown is not a failure. Reporting one would put "the tunnel died" on
    /// screen every time the user quit.
    #[test]
    fn a_shutdown_is_not_reported_as_a_failure() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut tunnel = Tunnel::start_with(a_hold(), recording(&log, "exec sleep 300"));
        assert!(poll_until(Duration::from_secs(5), || child_pid(&tunnel).is_some()));
        let _group = GroupGuard(child_pid(&tunnel).unwrap());

        tunnel.shutdown();
        // Long enough for a worker that was going to report the kill as a death
        // to have done it.
        assert!(
            !poll_until(Duration::from_millis(600), || tunnel.last_error().is_some()),
            "the teardown was reported as a tunnel failure: {:?}",
            tunnel.health()
        );
        assert_eq!(calls(&log), 1, "the shutdown was followed by a reconnect");
    }
}

