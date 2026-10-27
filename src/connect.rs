use std::io::{self, Write};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::config;
use crate::ssh::{self, Opts, Probe};
use crate::types::Barn;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

/// Inner width of the connect box, in columns.
const BOX_WIDTH: usize = 60;

/// Backoff between automatic retries, indexed by consecutive failures. A
/// dropped connection comes back fast; a barn that stays down is asked less and
/// less often, settling at the last step.
const RETRY_STEPS_SECS: &[u64] = &[3, 5, 10, 20, 30];

/// An attach that ends sooner than this counts as a failed attempt, not a
/// session. Without it a remote yeehaw that crashes on launch would be
/// relaunched every few seconds forever at the shortest backoff.
const MIN_HEALTHY_SESSION: Duration = Duration::from_secs(10);

/// The installer the barn runs when the user picks `i`.
const INSTALL_CMD: &str = "curl -LsSf https://yeehaw.cool/install.sh | sh";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Blocker {
    NoTmux,
    NoYeehaw,
}

/// One-line summary of what the barn reported, for the connect screen.
pub fn status_line(p: &Probe) -> String {
    let mut parts = vec!["ssh ok".to_string()];
    parts.push(format!("tmux {}", if p.has_tmux { "ok" } else { "missing" }));
    parts.push(format!("yeehaw {}", if p.has_yeehaw { "ok" } else { "missing" }));
    if p.session_live {
        parts.push("session live".to_string());
    }
    parts.join(" · ")
}

/// The first thing that must be fixed before an attach can succeed.
/// A missing *session* is not a blocker: remote yeehaw creates one on launch.
pub fn blocker(p: &Probe) -> Option<Blocker> {
    if !p.has_tmux {
        return Some(Blocker::NoTmux);
    }
    if !p.has_yeehaw {
        return Some(Blocker::NoYeehaw);
    }
    None
}

fn describe(b: Blocker) -> String {
    match b {
        Blocker::NoTmux => "tmux is not installed on this barn".into(),
        Blocker::NoYeehaw => "yeehaw is not installed on this barn".into(),
    }
}

/// Why `barn` cannot be connected to, or `None` when it can.
///
/// One copy of the wording, shared with the TUI's `c` binding
/// (`app::connect_barn`): the same barn must not produce two different messages
/// depending on which side noticed.
///
/// `barn_is_this_machine`, not `is_local_barn`. Connecting means opening an ssh
/// session to a remote yeehaw, and after `migrate::adopt_this_machine` the barn
/// that is *this* machine is a real record under the machine's own name.
/// Answering only about the synthetic `local` let the user try to ssh to the
/// machine they were already sitting at.
pub fn refusal(barn: &Barn) -> Option<String> {
    if config::barn_is_this_machine(barn) {
        return Some(format!(
            "'{}' is the local barn — not connectable, just run yeehaw",
            barn.name
        ));
    }
    if barn.connectable == Some(false) {
        return Some(format!("barn '{}' is not connectable over SSH", barn.name));
    }
    None
}

/// Entry point for `yeehaw connect <barn>`. Runs inside its own tmux session,
/// never inside the TUI — a 10s ConnectTimeout here would otherwise freeze the
/// 250ms app loop.
pub fn run(barn_name: &str) -> Result<()> {
    let found = config::load_barns().into_iter().find(|b| b.name == barn_name);
    let barn = match found {
        Some(barn) => barn,
        // `local` is a spelling of this machine, and it stops appearing in
        // `load_barns()` the moment the machine is adopted — its real record
        // has taken the row. Standing the synthetic one back up here keeps
        // `yeehaw connect local` explaining *why* it is not connectable rather
        // than answering "no barn named 'local'".
        None if config::barn_name_is_this_machine(barn_name) => config::local_barn(),
        None => return Err(anyhow!("no barn named '{}'", barn_name)),
    };

    if let Some(why) = refusal(&barn) {
        return Err(anyhow!(why));
    }

    // Consecutive attempts that got nowhere. Drives the retry backoff, and is
    // reset by any session that lasted long enough to count as one.
    let mut failures: u32 = 0;

    loop {
        render_connecting(&barn.name);

        match ssh::probe(&barn) {
            Err(e) => {
                failures += 1;
                let auto = Some(retry_delay(failures));
                if !render_error(&barn, &format!("unreachable — {}", e), None, auto)? {
                    return Ok(());
                }
            }
            Ok(p) => match blocker(&p) {
                // Missing tmux or yeehaw needs the user to act; retrying on a
                // timer would only redraw the same complaint.
                Some(b) => {
                    if !render_error(&barn, &describe(b), Some((b, &p)), None)? {
                        return Ok(());
                    }
                }
                None => {
                    let started = Instant::now();
                    let status = attach(&barn)?;
                    reset_terminal_modes();

                    if started.elapsed() >= MIN_HEALTHY_SESSION {
                        failures = 0;
                    }
                    let auto = if status.success() {
                        // The remote yeehaw was quit on purpose. Bringing it
                        // straight back would fight the user.
                        None
                    } else {
                        failures += 1;
                        Some(retry_delay(failures))
                    };
                    if !render_dropped(&barn, auto)? {
                        return Ok(());
                    }
                }
            },
        }
    }
}

/// Wait before automatic retry number `failures` (1-based).
fn retry_delay(failures: u32) -> Duration {
    let i = (failures.max(1) as usize - 1).min(RETRY_STEPS_SECS.len() - 1);
    Duration::from_secs(RETRY_STEPS_SECS[i])
}

/// Hand the terminal to the remote yeehaw. `bash -lc` is required: a
/// non-interactive SSH session does not source the login profile, so `yeehaw`
/// would not otherwise be on PATH.
///
/// `dedicated`: see [`Opts::dedicated`] — as a mux client, a stale master could
/// keep reading this pane's tty after we get it back, and no key would reach
/// the reconnect screen.
///
/// The exit status is ssh's: 0 when the remote yeehaw quit normally, 255 when
/// the connection itself failed (including a keepalive timeout).
fn attach(barn: &Barn) -> Result<ExitStatus> {
    // Never hand a raw terminal to ssh — the remote TUI sets up its own modes
    // and we would have no way to restore ours afterwards.
    let _ = disable_raw_mode();
    let opts = Opts { tty: true, dedicated: true, ..Default::default() };
    let mut cmd = ssh::command(barn, "bash -lc yeehaw", opts)?;
    Ok(cmd.status()?)
}

/// Undo whatever terminal modes the remote side switched on. A connection that
/// dies mid-session never sends the remote tmux's teardown, so mouse tracking,
/// focus reporting, bracketed paste, extended keys and the alternate screen are
/// all still on in this pane — and the reconnect screen would be drawn into the
/// alternate screen with the pane reporting mouse motion at it.
fn reset_terminal_modes() {
    print!(
        "\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\
         \x1b[?1004l\x1b[?2004l\x1b[>4m\x1b[<u\x1b>\x1b[?25h\x1b[0m"
    );
    let _ = io::stdout().flush();
}

// ---------------------------------------------------------------------------
// Terminal input
// ---------------------------------------------------------------------------

/// Raw mode, scoped. `Drop` runs on every exit path from the scope — normal
/// return, `?` propagation, and panics — so the terminal can never be left raw.
struct RawGuard;

impl RawGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()
            .map_err(|e| anyhow!("connect needs an interactive terminal ({})", e))?;
        Ok(RawGuard)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Block until the user presses one of `allowed` (an empty slice means any
/// key). Ctrl-C, Ctrl-D and Esc always mean quit.
fn read_key(allowed: &[char]) -> Result<char> {
    loop {
        if let Some(c) = read_key_within(allowed, Duration::from_secs(3600))? {
            return Ok(c);
        }
    }
}

/// [`read_key`], giving up after `timeout` with `None`.
pub(crate) fn read_key_within(allowed: &[char], timeout: Duration) -> Result<Option<char>> {
    let _guard = RawGuard::new()?;
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || !event::poll(left)? {
            return Ok(None);
        }
        if let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event::read()? {
            if let Some(c) = interpret_key(code, modifiers, kind, allowed) {
                return Ok(Some(c));
            }
        }
    }
}

fn interpret_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    kind: KeyEventKind,
    allowed: &[char],
) -> Option<char> {
    if kind != KeyEventKind::Press {
        return None;
    }
    if modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('c') | KeyCode::Char('d'))
    {
        return Some('q');
    }
    match code {
        KeyCode::Esc => Some('q'),
        KeyCode::Char(c) => {
            let c = c.to_ascii_lowercase();
            (allowed.is_empty() || allowed.contains(&c)).then_some(c)
        }
        _ if allowed.is_empty() => Some(' '),
        _ => None,
    }
}

/// Draw a screen and wait for one of `keys`. With `auto`, the screen counts
/// down and answers `'r'` by itself when it reaches zero; `'r'` must be in
/// `keys`. The hint line is appended last, so the countdown sits beside the
/// keys that cut it short.
fn prompt(rows: &[(String, &str)], hint: &str, keys: &[char], auto: Option<Duration>) -> Result<char> {
    let draw = |hint_line: String| {
        let mut all = rows.to_vec();
        all.push((hint_line, DIM));
        clear();
        render_box("yeehaw · connect", &all);
    };

    let Some(wait) = auto else {
        draw(hint.to_string());
        return read_key(keys);
    };

    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok('r');
        }
        let secs = left.as_secs() + u64::from(left.subsec_nanos() > 0);
        draw(format!("retrying in {}s   {}", secs, hint));
        // Wake when the displayed second runs out, so the countdown ticks.
        let tick = left - Duration::from_secs(secs - 1).min(left);
        if let Some(c) = read_key_within(keys, tick)? {
            return Ok(c);
        }
    }
}

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------

fn clear() {
    print!("\x1b[2J\x1b[3J\x1b[H");
    let _ = io::stdout().flush();
}

/// One box row. Padding is computed from the plain text, so colors never
/// misalign the right border.
fn row(text: &str, color: &str) {
    let mut text: String = text.chars().take(BOX_WIDTH - 2).collect();
    let pad = BOX_WIDTH.saturating_sub(text.chars().count() + 1);
    text.push_str(&" ".repeat(pad));
    println!("  {DIM}│{RESET} {color}{text}{RESET}{DIM}│{RESET}");
}

/// Split text to fit inside the box. ssh failure messages are frequently longer
/// than the box is wide, and a truncated one hides the actual reason.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.chars().count() <= width {
        return vec![text.to_string()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();

    for word in text.split_whitespace() {
        // Hard-split anything that cannot fit on a line of its own.
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            lines.push(word.chars().take(width).collect());
            word = word.chars().skip(width).collect();
        }

        if cur.is_empty() {
            cur = word;
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(&word);
        } else {
            lines.push(std::mem::replace(&mut cur, word));
        }
    }

    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

fn render_box(title: &str, rows: &[(String, &str)]) {
    let bar = "─".repeat(BOX_WIDTH);
    println!();
    println!("  {DIM}╭{bar}╮{RESET}");
    row(title, BOLD);
    println!("  {DIM}├{bar}┤{RESET}");
    for (text, color) in rows {
        for line in wrap(text, BOX_WIDTH - 2) {
            row(&line, color);
        }
    }
    println!("  {DIM}╰{bar}╯{RESET}");
    println!();
    let _ = io::stdout().flush();
}

fn render_connecting(name: &str) {
    clear();
    render_box(
        "yeehaw · connect",
        &[
            (format!("barn   {}", name), CYAN),
            (String::new(), RESET),
            ("connecting…".to_string(), DIM),
        ],
    );
}

/// The barn cannot be attached to yet. Returns `true` to retry the outer loop,
/// `false` to exit the process.
///
/// With `auto`, retries by itself after that long unless a key is pressed.
fn render_error(
    barn: &Barn,
    message: &str,
    ctx: Option<(Blocker, &Probe)>,
    auto: Option<Duration>,
) -> Result<bool> {
    let can_install = matches!(ctx, Some((Blocker::NoYeehaw, _)));

    let mut rows: Vec<(String, &str)> = vec![
        (format!("barn   {}", barn.name), CYAN),
        (String::new(), RESET),
        (format!("✗ {}", message), RED),
    ];

    if let Some((b, p)) = ctx {
        rows.push((status_line(p), DIM));
        rows.push((String::new(), RESET));
        match b {
            Blocker::NoTmux => {
                rows.push(("install tmux on the barn, e.g.".to_string(), YELLOW));
                rows.push(("  apt install tmux".to_string(), DIM));
            }
            Blocker::NoYeehaw => {
                rows.push(("press i to install yeehaw on the barn".to_string(), YELLOW));
                rows.push((format!("  {}", INSTALL_CMD), DIM));
            }
        }
    }

    rows.push((String::new(), RESET));
    let hint = match (can_install, auto.is_some()) {
        (true, _) => "[r] retry   [i] install   [q] quit",
        (false, true) => "[r] now   [q] quit",
        (false, false) => "[r] retry   [q] quit",
    };

    let keys: &[char] = if can_install { &['r', 'i', 'q'] } else { &['r', 'q'] };
    match prompt(&rows, hint, keys, auto)? {
        'r' => Ok(true),
        'i' => {
            install(barn)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The remote session ended. Returns `true` to reconnect, `false` to exit.
///
/// `auto` is set when the connection was lost rather than quit, and reconnects
/// by itself after that long.
fn render_dropped(barn: &Barn, auto: Option<Duration>) -> Result<bool> {
    let (what, hint) = if auto.is_some() {
        ("connection lost", "[r] now   [q] quit")
    } else {
        ("session ended", "[r] reconnect   [q] quit")
    };
    let rows = [
        (format!("barn   {}", barn.name), CYAN),
        (String::new(), RESET),
        (what.to_string(), YELLOW),
        (String::new(), RESET),
    ];

    Ok(prompt(&rows, hint, &['r', 'q'], auto)? == 'r')
}

/// Run the yeehaw installer on the barn. Errors are shown on the screen rather
/// than returned — the caller loops back and re-probes either way.
fn install(barn: &Barn) -> Result<()> {
    clear();
    render_box(
        "yeehaw · connect",
        &[
            (format!("barn   {}", barn.name), CYAN),
            (String::new(), RESET),
            ("installing yeehaw — this can take a minute…".to_string(), DIM),
        ],
    );

    let result = ssh::run(barn, INSTALL_CMD, Opts { batch: true, ..Default::default() });

    let rows: Vec<(String, &str)> = match &result {
        Ok(_) => vec![
            (format!("barn   {}", barn.name), CYAN),
            (String::new(), RESET),
            ("✓ yeehaw installed".to_string(), GREEN),
            (String::new(), RESET),
            ("press any key to continue".to_string(), DIM),
        ],
        Err(e) => vec![
            (format!("barn   {}", barn.name), CYAN),
            (String::new(), RESET),
            ("✗ install failed".to_string(), RED),
            (e.to_string(), DIM),
            (String::new(), RESET),
            ("press any key to continue".to_string(), DIM),
        ],
    };

    clear();
    render_box("yeehaw · connect", &rows);
    read_key(&[])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::Probe;

    #[test]
    fn retry_backoff_grows_then_settles() {
        let secs: Vec<u64> = (1..=7).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(secs, vec![3, 5, 10, 20, 30, 30, 30]);
        // 0 is not a count the loop produces, but must not panic.
        assert_eq!(retry_delay(0).as_secs(), 3);
    }

    #[test]
    fn quit_keys_always_quit_and_others_are_filtered() {
        let press = KeyEventKind::Press;
        let none = KeyModifiers::NONE;
        let r = |c| interpret_key(KeyCode::Char(c), none, press, &['r', 'q']);
        assert_eq!(r('r'), Some('r'));
        assert_eq!(r('R'), Some('r'));
        assert_eq!(r('x'), None);
        assert_eq!(interpret_key(KeyCode::Esc, none, press, &['r']), Some('q'));
        assert_eq!(
            interpret_key(KeyCode::Char('c'), KeyModifiers::CONTROL, press, &['r']),
            Some('q')
        );
        assert_eq!(interpret_key(KeyCode::Char('r'), none, KeyEventKind::Release, &['r']), None);
        assert_eq!(interpret_key(KeyCode::Enter, none, press, &[]), Some(' '));
    }

    #[test]
    fn summarizes_a_ready_barn() {
        let p = Probe { has_tmux: true, has_yeehaw: true, session_live: true };
        assert_eq!(status_line(&p), "ssh ok · tmux ok · yeehaw ok · session live");
    }

    #[test]
    fn summarizes_missing_yeehaw() {
        let p = Probe { has_tmux: true, has_yeehaw: false, session_live: false };
        assert_eq!(status_line(&p), "ssh ok · tmux ok · yeehaw missing");
    }

    #[test]
    fn summarizes_missing_tmux() {
        let p = Probe { has_tmux: false, has_yeehaw: true, session_live: false };
        assert_eq!(status_line(&p), "ssh ok · tmux missing · yeehaw ok");
    }

    #[test]
    fn blocker_names_the_first_thing_to_fix() {
        assert_eq!(
            blocker(&Probe { has_tmux: false, has_yeehaw: false, session_live: false }),
            Some(Blocker::NoTmux)
        );
        assert_eq!(
            blocker(&Probe { has_tmux: true, has_yeehaw: false, session_live: false }),
            Some(Blocker::NoYeehaw)
        );
        // Not running is not a blocker — remote yeehaw creates its own session.
        assert_eq!(
            blocker(&Probe { has_tmux: true, has_yeehaw: true, session_live: false }),
            None
        );
    }

    /// Connecting opens an ssh session to a remote yeehaw. After adoption the
    /// barn record for *this* machine carries this machine's real name, and the
    /// refusal has to name that for what it is — "you are already here" — not
    /// report it as some remote barn that happens to be unreachable.
    #[test]
    fn the_adopted_self_barn_is_refused_as_this_machine() {
        let _ranch = crate::testing::temp_ranch();
        crate::migrate::adopt_this_machine("imac").unwrap();

        let imac = config::load_barns()
            .into_iter()
            .find(|b| b.name == "imac")
            .expect("adoption mints the record");
        assert!(!config::is_local_barn(&imac), "control: not the synthetic placeholder");

        let why = refusal(&imac).expect("the machine you are sitting at is never connectable");
        assert!(
            why.contains("just run yeehaw"),
            "refused as an unreachable remote instead of as this machine: {:?}",
            why
        );
    }

    /// The synthetic row is still this machine on a ranch that was never
    /// adopted, which is the only state it survives into.
    #[test]
    fn the_synthetic_local_barn_is_still_refused() {
        let _ranch = crate::testing::temp_ranch();

        let why = refusal(&config::local_barn()).expect("`local` is never connectable");
        assert!(why.contains("just run yeehaw"), "{:?}", why);
    }

    /// And a real remote barn is still offered, or `yeehaw connect` would have
    /// nothing left to do.
    #[test]
    fn a_reachable_remote_barn_is_not_refused() {
        let _ranch = crate::testing::temp_ranch();
        crate::migrate::adopt_this_machine("imac").unwrap();

        let pi = Barn {
            name: "pi".into(),
            host: Some("pi.local".into()),
            user: Some("cam".into()),
            ..Default::default()
        };
        assert_eq!(refusal(&pi), None);

        let mut managed = pi.clone();
        managed.name = "terraform-managed".into();
        managed.connectable = Some(false);
        assert!(
            refusal(&managed).is_some_and(|why| why.contains("not connectable over SSH")),
            "a barn that declared itself unreachable still says so"
        );
    }
}
