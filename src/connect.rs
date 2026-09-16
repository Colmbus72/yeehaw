use std::io::{self, Write};

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

    loop {
        render_connecting(&barn.name);

        match ssh::probe(&barn) {
            Err(e) => {
                if !render_error(&barn, &format!("unreachable — {}", e), None)? {
                    return Ok(());
                }
            }
            Ok(p) => match blocker(&p) {
                Some(b) => {
                    if !render_error(&barn, &describe(b), Some((b, &p)))? {
                        return Ok(());
                    }
                }
                None => {
                    attach(&barn)?;
                    // attach only returns if the connection dropped.
                    if !render_dropped(&barn)? {
                        return Ok(());
                    }
                }
            },
        }
    }
}

/// Hand the terminal to the remote yeehaw. `bash -lc` is required: a
/// non-interactive SSH session does not source the login profile, so `yeehaw`
/// would not otherwise be on PATH.
fn attach(barn: &Barn) -> Result<()> {
    // Never hand a raw terminal to ssh — the remote TUI sets up its own modes
    // and we would have no way to restore ours afterwards.
    let _ = disable_raw_mode();
    let mut cmd = ssh::command(barn, "bash -lc yeehaw", Opts { tty: true, ..Default::default() })?;
    let _ = cmd.status()?;
    Ok(())
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
    let _guard = RawGuard::new()?;
    loop {
        if let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event::read()? {
            if kind != KeyEventKind::Press {
                continue;
            }
            if modifiers.contains(KeyModifiers::CONTROL)
                && matches!(code, KeyCode::Char('c') | KeyCode::Char('d'))
            {
                return Ok('q');
            }
            match code {
                KeyCode::Esc => return Ok('q'),
                KeyCode::Char(c) => {
                    let c = c.to_ascii_lowercase();
                    if allowed.is_empty() || allowed.contains(&c) {
                        return Ok(c);
                    }
                }
                _ if allowed.is_empty() => return Ok(' '),
                _ => {}
            }
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
fn render_error(barn: &Barn, message: &str, ctx: Option<(Blocker, &Probe)>) -> Result<bool> {
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
    rows.push((
        if can_install {
            "[r] retry   [i] install   [q] quit".to_string()
        } else {
            "[r] retry   [q] quit".to_string()
        },
        DIM,
    ));

    clear();
    render_box("yeehaw · connect", &rows);

    let keys: &[char] = if can_install { &['r', 'i', 'q'] } else { &['r', 'q'] };
    match read_key(keys)? {
        'r' => Ok(true),
        'i' => {
            install(barn)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The remote session ended. Returns `true` to reconnect, `false` to exit.
fn render_dropped(barn: &Barn) -> Result<bool> {
    clear();
    render_box(
        "yeehaw · connect",
        &[
            (format!("barn   {}", barn.name), CYAN),
            (String::new(), RESET),
            ("connection closed".to_string(), YELLOW),
            (String::new(), RESET),
            ("[r] reconnect   [q] quit".to_string(), DIM),
        ],
    );

    Ok(read_key(&['r', 'q'])? == 'r')
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
