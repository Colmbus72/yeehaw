//! Worms and trails run as windows in the `yeehaw` tmux session.
//!
//! Every way a run starts — cron, the git poller, MCP, a key in the dashboard —
//! goes through [`launch_worm`] / [`launch_trail`], which open a background
//! window running `yeehaw worm run …` / `yeehaw trail run …`. That process *is*
//! the run: it streams output to the window as it happens, writes the log and
//! the run record itself, takes Ctrl-C as cancel, and when it is done counts
//! down and closes the window.
//!
//! The dashboard only reads what the runner writes. It does not have to be
//! alive for a run to start, finish, or be recorded, and leaving a view no
//! longer has anything to do with whether a run keeps going.
//!
//! Nothing here ever selects a window: a run appearing must not move the user
//! off whatever they are doing.

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use crate::config;
use crate::trails::provider::{StepStatus, TrailProvider};
use crate::trails::{TrailRun, TrailStepRun};
use crate::tmux;
use crate::types::{Barn, Livestock, WormRun};

/// How long a finished run's window stays up before closing itself.
const CLOSE_AFTER: Duration = Duration::from_secs(15);

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

#[derive(Debug, PartialEq)]
pub enum Launch {
    /// A window was opened for the run.
    Started,
    /// There is no `yeehaw` tmux session to open it in. Nothing was done.
    NoSession,
    /// The trail already has a live run on this livestock. Nothing was done.
    AlreadyRunning,
}

// ---------------------------------------------------------------------------
// Launching
// ---------------------------------------------------------------------------

/// Open a window that runs worm `name`.
pub fn launch_worm(name: &str, trigger: &str) -> Result<Launch> {
    let worm = config::load_worms()
        .into_iter()
        .find(|w| w.name == name)
        .ok_or_else(|| anyhow!("no worm named '{}'", name))?;

    let args = ["worm", "run", name, "--trigger", trigger].map(String::from);
    open_window("worm", &format!("worm-{}", name), &args, worm.project.as_deref(), None)
}

/// Open a window that runs `trail` on `livestock`.
///
/// `barn` names the barn to run on; `None` resolves it from the livestock, the
/// same way the runner would.
pub fn launch_trail(
    livestock: &str,
    trail: &str,
    project: Option<&str>,
    barn: Option<&str>,
    trigger: &str,
) -> Result<Launch> {
    if !tmux::yeehaw_session_exists() {
        return Ok(Launch::NoSession);
    }
    if trail_is_running(livestock, trail) {
        return Ok(Launch::AlreadyRunning);
    }

    let mut args: Vec<String> =
        ["trail", "run", livestock, trail, "--trigger", trigger].map(String::from).to_vec();
    if let Some(p) = project {
        args.extend(["--project".into(), p.into()]);
    }
    if let Some(b) = barn {
        args.extend(["--barn".into(), b.into()]);
    }
    // Tag the window with the barn it will actually run on, so the grid's barn
    // filter finds it whether or not the caller named one.
    let barn_tag = match barn {
        Some(b) => Some(b.to_string()),
        None => find_livestock(livestock, project)
            .and_then(|(l, _)| config::resolve_livestock_barn(&l).map(String::from)),
    };
    open_window("trail", &format!("trail-{}", trail), &args, project, barn_tag.as_deref())
}

fn open_window(
    kind: &str,
    window_name: &str,
    args: &[String],
    project: Option<&str>,
    barn: Option<&str>,
) -> Result<Launch> {
    if !tmux::yeehaw_session_exists() {
        return Ok(Launch::NoSession);
    }
    let command = std::iter::once(tmux::current_exe_path())
        .chain(args.iter().cloned())
        .map(|a| tmux::shell_escape(&a))
        .collect::<Vec<_>>()
        .join(" ");
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    let idx = tmux::create_task_window(kind, window_name, &command, &home)?;
    tmux::set_window_scope(idx, project.unwrap_or(""), barn);
    Ok(Launch::Started)
}

/// Cron runs with `PATH=/usr/bin:/bin`, which has no `tmux` on a Homebrew
/// machine — `has-session` would fail to spawn and every cron worm would look
/// like "no session". Commands inside the window get the tmux server's own
/// environment and do not need this; only our process does.
pub fn ensure_path() {
    let mut parts: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
    for extra in [
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        format!("{}/.local/bin", home),
        format!("{}/.cargo/bin", home),
    ] {
        if !parts.contains(&extra) {
            parts.push(extra);
        }
    }
    std::env::set_var("PATH", parts.join(":"));
}

// ---------------------------------------------------------------------------
// Liveness
// ---------------------------------------------------------------------------

fn pid_alive(pid: u32) -> bool {
    // Signal 0 checks existence without delivering anything. EPERM means it
    // exists but belongs to someone else — still alive.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether `run` is still executing. A record that says `running` with no
/// process behind it — the runner was killed, the machine rebooted, or it came
/// from a build that ran trails inside the dashboard — is not.
pub fn run_is_live(run: &TrailRun) -> bool {
    run.status == "running" && run.pid.is_some_and(pid_alive)
}

pub fn trail_is_running(livestock: &str, trail: &str) -> bool {
    config::load_trail_runs(livestock, trail).iter().any(run_is_live)
}

/// Cancel a live run the way a user in its window would: Ctrl-C into its
/// pane. Falls back to SIGINT on the runner when the pane is unknown or gone.
pub fn cancel_trail_run(run: &TrailRun) -> bool {
    if let Some(pane) = run.pane.as_deref() {
        if tmux::send_interrupt(pane) {
            return true;
        }
    }
    match run.pid {
        Some(pid) => unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) == 0 },
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Ctrl-C
// ---------------------------------------------------------------------------

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Ctrl-C in the window signals the whole foreground process group: the job's
/// own children die of it, and the runner — which catches it instead — lives
/// on to record the run as cancelled. A handler, not `SIG_IGN`: an ignored
/// signal stays ignored across `exec`, and the job's children would then be
/// immune to Ctrl-C too.
fn catch_sigint() {
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Running a worm
// ---------------------------------------------------------------------------

/// Execute worm `name` in this process, streaming to stdout. `interactive`
/// adds the closing countdown; it is off for headless runs (poll worms).
/// Returns the job's exit code.
pub fn run_worm(name: &str, trigger: &str, interactive: bool) -> Result<i32> {
    ensure_path();
    let worm = config::load_worms()
        .into_iter()
        .find(|w| w.name == name)
        .ok_or_else(|| anyhow!("no worm named '{}'", name))?;
    catch_sigint();

    let now = chrono::Utc::now();
    let mut run = WormRun {
        worm: worm.name.clone(),
        started_at: now.to_rfc3339(),
        finished_at: None,
        exit_code: None,
        log_file: format!("{}.log", now.format("%Y-%m-%dT%H-%M-%S")),
        trigger: trigger.to_string(),
        status: Some("running".to_string()),
        skip_reason: None,
    };
    config::save_worm_run(&worm.name, &run)?;
    let log_path = config::worm_runs_for(&worm.name).join(&run.log_file);

    let cwd = expand_home(worm.working_dir.as_deref().unwrap_or("~"));
    let script = match worm.worm_type.as_str() {
        "claude" => format!(
            "{} -p {}",
            tmux::claude_base_command(),
            tmux::shell_escape(&worm.command)
        ),
        _ => worm.command.clone(),
    };

    if interactive {
        println!("{BOLD}{CYAN}worm{RESET} {BOLD}{}{RESET}  {DIM}{} · {}{RESET}", worm.name, trigger, cwd.display());
        println!("{DIM}$ {}{RESET}\n", worm.command);
    }

    let started = Instant::now();
    let code = match run_teed(&script, &cwd, &log_path) {
        Ok(code) => code,
        Err(e) => {
            println!("{RED}{e}{RESET}");
            append_log(&log_path, &e.to_string());
            -1
        }
    };

    let status = if interrupted() {
        "cancelled"
    } else if code == 0 {
        "success"
    } else {
        "failed"
    };
    run.finished_at = Some(chrono::Utc::now().to_rfc3339());
    run.exit_code = Some(code);
    run.status = Some(status.to_string());
    config::save_worm_run(&worm.name, &run)?;

    if interactive {
        finish(status, &format!("worm {} · exit {} · {}", worm.name, code, elapsed(started)));
    }
    Ok(code)
}

/// Run `script` under `sh` in `cwd`, copying its combined output to both
/// stdout and `log_path` as it arrives. Stdin stays the terminal, so a job
/// that asks a question can be answered from the window.
fn run_teed(script: &str, cwd: &Path, log_path: &Path) -> Result<i32> {
    let mut log = File::create(log_path).ok();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("( {}\n) 2>&1", script))
        .current_dir(cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start in {}", cwd.display()))?;

    if let Some(mut out) = child.stdout.take() {
        let mut buf = [0u8; 8192];
        let mut stdout = std::io::stdout();
        loop {
            match out.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = stdout.write_all(&buf[..n]);
                    let _ = stdout.flush();
                    if let Some(f) = log.as_mut() {
                        let _ = f.write_all(&buf[..n]);
                    }
                }
                // A Ctrl-C lands on this process too and interrupts the read.
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    }

    let status = child.wait()?;
    Ok(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

fn append_log(path: &Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", line);
    }
}

// ---------------------------------------------------------------------------
// Running a trail
// ---------------------------------------------------------------------------

/// Find `livestock_name` (optionally within `project`) and its project name.
fn find_livestock(livestock_name: &str, project: Option<&str>) -> Option<(Livestock, String)> {
    config::load_projects()
        .into_iter()
        .filter(|p| project.map_or(true, |name| p.name == name))
        .find_map(|p| {
            let name = p.name.clone();
            p.livestock.into_iter().find(|l| l.name == livestock_name).map(|l| (l, name))
        })
}

/// The barn a trail runs on: the one named, else the livestock's own, else
/// this machine.
fn resolve_barn(barn: Option<&str>, livestock: &Livestock) -> Barn {
    let barns = config::load_barns();
    let named = barn.or_else(|| config::resolve_livestock_barn(livestock));
    match named {
        Some(name) if !config::barn_name_is_this_machine(name) => barns
            .into_iter()
            .find(|b| b.name == name)
            .unwrap_or_else(config::local_barn),
        _ => config::local_barn(),
    }
}

/// Execute a trail in this process, streaming each step to stdout and keeping
/// `run.json` current at every step transition. Returns 0 on success.
pub fn run_trail(
    livestock_name: &str,
    trail_name: &str,
    project: Option<&str>,
    barn: Option<&str>,
    interactive: bool,
) -> Result<i32> {
    ensure_path();
    let (livestock, project_name) = find_livestock(livestock_name, project)
        .ok_or_else(|| anyhow!("no livestock named '{}'", livestock_name))?;
    let trail = config::load_trail(trail_name)
        .ok_or_else(|| anyhow!("no trail named '{}'", trail_name))?;
    let barn = resolve_barn(barn, &livestock);
    catch_sigint();

    let (run_dir, mut rx, provider) =
        crate::trails::runner::start_trail(&trail, &livestock, &barn, Some(&project_name))?;
    let mut run = read_run(&run_dir).ok_or_else(|| anyhow!("run record missing in {}", run_dir.display()))?;
    let total = run.steps.len();

    if interactive {
        println!(
            "{BOLD}{CYAN}trail{RESET} {BOLD}{}{RESET} → {}  {DIM}{} · {}{RESET}\n",
            trail.name, livestock.name, project_name, barn.name
        );
    }

    let started = Instant::now();
    let mut step_started: Option<Instant> = None;
    let mut cancel_sent = false;

    loop {
        if interrupted() && !cancel_sent {
            let _ = provider.cancel();
            cancel_sent = true;
            println!("\n{YELLOW}cancelling…{RESET}");
        }
        let update = match rx.try_recv() {
            Ok(u) => u,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        };
        let i = update.step_index;
        let name = run.steps.get(i).map(|s| s.name.clone()).unwrap_or_default();

        match (&update.status, update.output_line) {
            (StepStatus::Running, Some(line)) => println!("{}", line),
            (StepStatus::Running, None) => {
                println!("{BOLD}▶ [{}/{}] {}{RESET}", i + 1, total, name);
                step_started = Some(Instant::now());
                set_step(&mut run, i, "running", None, None);
                let _ = config::save_trail_run(&run, &run_dir);
            }
            (status, line) => {
                if let Some(line) = line {
                    println!("{}", line);
                }
                let ms = step_started.map(|t| t.elapsed().as_millis() as u64);
                let took = step_started.map(|t| fmt_duration(t.elapsed())).unwrap_or_default();
                match status {
                    StepStatus::Success => {
                        println!("{GREEN}✓ {}{RESET} {DIM}{}{RESET}\n", name, took);
                        set_step(&mut run, i, "success", Some(0), ms);
                    }
                    StepStatus::Failed { exit_code } => {
                        println!("{RED}✗ {} · exit {}{RESET} {DIM}{}{RESET}\n", name, exit_code, took);
                        set_step(&mut run, i, "failed", Some(*exit_code), ms);
                    }
                    StepStatus::Skipped => set_step(&mut run, i, "skipped", None, None),
                    _ => {}
                }
                let _ = config::save_trail_run(&run, &run_dir);
            }
        }
    }

    let status = if interrupted() {
        "cancelled"
    } else if run.steps.iter().all(|s| s.status == "success" || s.status == "skipped") {
        "success"
    } else {
        "failed"
    };
    run.status = status.to_string();
    run.finished_at = Some(chrono::Utc::now().to_rfc3339());
    config::save_trail_run(&run, &run_dir)?;

    if interactive {
        finish(status, &format!("trail {} → {} · {}", trail.name, livestock.name, elapsed(started)));
    }
    Ok(if status == "success" { 0 } else { 1 })
}

fn read_run(run_dir: &Path) -> Option<TrailRun> {
    let json = std::fs::read_to_string(run_dir.join("run.json")).ok()?;
    serde_json::from_str(&json).ok()
}

fn set_step(run: &mut TrailRun, i: usize, status: &str, exit_code: Option<i32>, ms: Option<u64>) {
    if let Some(step) = run.steps.get_mut(i) {
        let step: &mut TrailStepRun = step;
        if status == "running" {
            step.started_at = Some(chrono::Utc::now().to_rfc3339());
        }
        step.status = status.to_string();
        if exit_code.is_some() {
            step.exit_code = exit_code;
        }
        if ms.is_some() {
            step.duration_ms = ms;
        }
    }
}

// ---------------------------------------------------------------------------
// Closing
// ---------------------------------------------------------------------------

/// A run that could not even start. Say why and hold the window open long
/// enough to read it, instead of the window vanishing the instant it opened.
pub fn fail_and_close(what: &str, err: &anyhow::Error) {
    println!("{RED}{BOLD}✗ {what} could not start{RESET}\n{RED}{err:#}{RESET}");
    countdown();
}

/// Print the outcome, then count down and return so the window closes. Any key
/// — Ctrl-C included — closes it at once.
fn finish(status: &str, summary: &str) {
    let (color, mark) = match status {
        "success" => (GREEN, "✓"),
        "cancelled" => (YELLOW, "■"),
        _ => (RED, "✗"),
    };
    println!("\n{color}{BOLD}{mark} {status}{RESET}  {DIM}{summary}{RESET}");
    countdown();
}

fn countdown() {
    let deadline = Instant::now() + CLOSE_AFTER;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let secs = left.as_secs() + u64::from(left.subsec_nanos() > 0);
        print!("\r{DIM}closing in {:>2}s — any key closes now{RESET} ", secs);
        let _ = std::io::stdout().flush();
        let tick = left - Duration::from_secs(secs - 1).min(left);
        match crate::connect::read_key_within(&[], tick) {
            Ok(None) => {}
            // A key, or no terminal to read one from: close now.
            Ok(Some(_)) | Err(_) => break,
        }
    }
    println!();
}

fn elapsed(started: Instant) -> String {
    fmt_duration(started.elapsed())
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

fn expand_home(path: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    if path == "~" {
        home
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: &str, pid: Option<u32>) -> TrailRun {
        TrailRun {
            livestock: "web".into(),
            trail: "deploy".into(),
            started_at: "2026-10-27T10:00:00+00:00".into(),
            finished_at: None,
            status: status.into(),
            steps: vec![],
            pid,
            pane: None,
        }
    }

    #[test]
    fn a_running_record_is_live_only_while_its_process_is() {
        assert!(run_is_live(&run("running", Some(std::process::id()))));
        // Records written before runs carried a pid, by a dashboard that has
        // since exited: interrupted, not running forever.
        assert!(!run_is_live(&run("running", None)));
        // pid 0x7fff_fff0 is far above any real pid.
        assert!(!run_is_live(&run("running", Some(0x7fff_fff0))));
        assert!(!run_is_live(&run("success", Some(std::process::id()))));
    }

    #[test]
    fn teed_output_reaches_the_log_and_the_exit_code_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("out.log");
        let code = run_teed("echo out; echo err >&2; exit 3", dir.path(), &log).unwrap();
        assert_eq!(code, 3);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("out") && text.contains("err"), "{:?}", text);
    }

    #[test]
    fn a_missing_working_dir_is_an_error_not_a_hang() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("out.log");
        assert!(run_teed("true", &dir.path().join("nope"), &log).is_err());
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(fmt_duration(Duration::from_secs(5)), "5s");
        assert_eq!(fmt_duration(Duration::from_secs(125)), "2m05s");
        assert_eq!(fmt_duration(Duration::from_secs(3720)), "1h02m");
    }
}
