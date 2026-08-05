//! Streaming a barn's session grid over one ssh channel.
//!
//! Two halves. The wire format — the loop that runs on the barn
//! ([`FRAME_SCRIPT`]) and the parser that turns its output back into windows,
//! screens, and signals ([`parse_frame`]) — and the transport that carries it,
//! [`RemoteStream`]: one long-lived child per barn, a reader off its stdout,
//! and a teardown that kills *and* reaps.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::signals::{self, SessionSignal};
use crate::ssh::{self, Opts};
use crate::tmux::{self, TmuxWindow};
use crate::types::Barn;

/// Section boundary inside one frame: windows │ captures │ signals.
pub(crate) const SPLIT_SENTINEL: &str = "\u{1}\u{2}YHSPLIT\u{2}\u{1}";
/// Pane boundary inside the captures section.
pub(crate) const SEP_SENTINEL: &str = "\u{1}\u{2}YHSEP\u{2}\u{1}";
/// End of frame. The reader accumulates until it sees this.
pub(crate) const FRAME_SENTINEL: &str = "\u{1}\u{2}YHFRAME\u{2}\u{1}";

/// The loop that runs on the barn. Emits one frame per second, forever.
///
/// ```text
/// <list-windows lines, WINDOW_LIST_FORMAT, tab separated>
/// \x01\x02YHSPLIT\x02\x01
/// <capture>\x01\x02YHSEP\x02\x01<capture>\x01\x02YHSEP\x02\x01...
/// \x01\x02YHSPLIT\x02\x01
/// <barn epoch seconds>
/// <sanitized pane id>\t<signal json, one line>
/// \x01\x02YHFRAME\x02\x01
/// ```
///
/// Four properties are load-bearing.
///
/// **1. The sentinels are built remotely by `printf`, never embedded as literal
/// bytes.** The whole script travels as a shell string through
/// [`crate::tmux::single_quote`], ssh's transport, and the remote shell's
/// parser. A raw `\x01` surviving all three is not something to rely on when
/// `printf "\001..."` constructs the byte on the far side and keeps the wire
/// pure ASCII. `sentinels_never_appear_as_literal_bytes_in_the_script` fails if
/// anyone "simplifies" this by inlining the control characters.
///
/// **2. The loop must never stop writing.** There is no `-t`, so no remote pty
/// and no SIGHUP. The *only* thing that kills this loop when the connection dies
/// is SIGPIPE on its next write. An "only emit a frame when something changed"
/// optimisation would leave a loop running `capture-pane` forever on the user's
/// production box every time a barn sat idle. Verified by hand against a live
/// barn: after killing the local ssh, `pgrep -f YHFRAME` → 0. If cadence ever
/// becomes adaptive, keep an unconditional heartbeat.
///
/// **3. It runs under `bash -lc`,** matching [`crate::ssh`]'s probe and
/// [`crate::connect`]'s attach. sshd hands a remote command to a non-login,
/// non-interactive shell whose PATH has neither Homebrew nor `~/.local/bin`, so
/// `tmux` is simply absent and every frame comes back empty. That was bug B of
/// the barn-connect work. See [`frame_command`].
///
/// **4. The parser must skip exactly what this script skips.** Captures are
/// positional — the Nth chunk belongs to the Nth emitted window — so a window
/// the script passes over without printing a `YHSEP` shifts every capture after
/// it into the wrong cell. Both sides skip window 0 and any line with no pane
/// id, and nothing else.
///
/// `date +%s` rides at the head of the signals section because signal freshness
/// must be judged against the barn's own clock. [`signals::read_signal`] drops
/// anything older than five minutes; measured against the *local* clock, a barn
/// running behind would have every status silently discarded and its cells would
/// render blank with nothing to explain why. See [`RemoteFrame::fresh_signal`].
///
/// Two details that are less obvious than they look:
///
/// - **`IFS= read -r` plus parameter expansion, not `IFS=<tab> read -r idx nm
///   act pane rest`.** Tab is IFS *whitespace*, so `read` collapses runs of it
///   and strips leading ones: one empty field shifts every later field left.
///   tmux allows an empty window name (`rename-window ""` is accepted), which
///   makes `$pane` the pane *title* and captures the wrong thing — silently,
///   forever, for that window.
/// - **Captures go through `tail -n 40`.** Cells render ~20 lines and the
///   renderer already keeps only the tail, so this roughly halves the frame with
///   nothing visible lost.
///
/// The script holds no single quote, so [`frame_command`] nests it in `bash -lc
/// '...'` with no escaping beyond the wrapper.
const FRAME_SCRIPT: &str = "\
    FMT=\"#{window_index}\t#{window_name}\t#{window_active}\t#{pane_id}\t#{pane_title}\t#{pane_current_command}\t#{window_activity}\t#{@yeehaw_type}\t#{@yeehaw_project}\t#{@yeehaw_barn}\"; \
    T=$(printf \"\\t\"); \
    while :; do \
    W=$(tmux list-windows -t \"=yeehaw\" -F \"$FMT\" 2>/dev/null); \
    printf \"%s\\n\" \"$W\"; \
    printf \"\\001\\002YHSPLIT\\002\\001\\n\"; \
    printf \"%s\\n\" \"$W\" | while IFS= read -r L; do \
    [ -n \"$L\" ] || continue; \
    [ \"${L%%\"$T\"*}\" = \"0\" ] && continue; \
    R=${L#*\"$T\"}; R=${R#*\"$T\"}; R=${R#*\"$T\"}; P=${R%%\"$T\"*}; \
    [ -n \"$P\" ] || continue; \
    tmux capture-pane -p -e -t \"$P\" 2>/dev/null | tail -n 40; \
    printf \"\\001\\002YHSEP\\002\\001\\n\"; \
    done; \
    printf \"\\001\\002YHSPLIT\\002\\001\\n\"; \
    date +%s; \
    for f in \"$HOME\"/.yeehaw/session-signals/*.json; do \
    [ -e \"$f\" ] || continue; \
    b=${f##*/}; \
    printf \"%s\\t\" \"${b%.json}\"; \
    tr -d \"\\n\" < \"$f\"; \
    printf \"\\n\"; \
    done; \
    printf \"\\001\\002YHFRAME\\002\\001\\n\"; \
    sleep 1; \
    done";

/// [`FRAME_SCRIPT`] as the single argv element to hand `ssh`.
///
/// `bash -lc` for the reason in property 3 above. The remote command reaches the
/// barn as one string that sshd runs through the login shell, so the script has
/// to survive one more round of shell parsing — hence the single quoting.
pub fn frame_command() -> String {
    format!("bash -lc {}", tmux::single_quote(FRAME_SCRIPT))
}

/// How a jump's one-shot exec talks to a barn.
///
/// `batch`, for the reason [`stream_command`] has it: this runs behind a
/// full-screen TUI, where an ssh password or passphrase prompt is invisible and
/// unanswerable, so a jump that can prompt is a jump that can hang yeehaw.
///
/// No `tty`. `select-window` renders nothing and exits.
const SELECT_OPTS: Opts = Opts { batch: true, tty: false, allow_failure: false };

/// The tmux target for window `window_index` of the yeehaw session **on a
/// barn**.
///
/// The `=` is the whole function. A bare `yeehaw:3` is a *prefix pattern* to
/// tmux, and it is the wrong answer that succeeds. Measured on tmux 3.6a
/// against a server holding one session named `yeehaw-decoy` and none named
/// `yeehaw`:
///
/// ```text
/// $ tmux select-window -t 'yeehaw:0'    # exit 0 — selected yeehaw-decoy:0
/// $ tmux select-window -t '=yeehaw:0'   # exit 1 — can't find session: yeehaw
/// ```
///
/// So an unanchored target cannot be caught downstream by an exit status: the
/// user simply lands in some unrelated session of their own on the barn.
///
/// **No trailing colon**, unlike `set-option -t` and `send-keys -t`, whose `-t`
/// is a target-*pane* and which both fail on a bare session target.
/// `select-window` takes a target-*window*, and `=yeehaw:2` is already one —
/// verified on tmux 3.6a, where it exits 0 and moves the session's active
/// window to 2. Do not generalise from the siblings without checking.
pub(crate) fn select_window_target(window_index: u32) -> String {
    format!("{}:{}", tmux::exact_target(tmux::YEEHAW_SESSION), window_index)
}

/// The remote command a jump sends, as the single argv element ssh takes.
///
/// `bash -lc`, matching the probe, the attach and the frame stream. sshd runs a
/// non-login shell whose PATH holds neither Homebrew nor `~/.local/bin`, so a
/// bare `tmux select-window` is "command not found" on exactly the barns whose
/// cells are already streaming happily — bug B of the barn-connect work.
pub(crate) fn select_window_command(window_index: u32) -> String {
    format!(
        "bash -lc {}",
        tmux::single_quote(&format!(
            "tmux select-window -t {}",
            select_window_target(window_index)
        ))
    )
}

/// Select a window in the yeehaw session on `barn`, so a jump lands there.
///
/// **Blocking**, ~70–180 ms over a warm `ControlMaster`. Affordable here and
/// nowhere else: this is a keypress. Nothing on the 250 ms idle tick may call
/// it — that is what the streaming thread exists to avoid.
///
/// The warm figure is the *good* case, and it is the only one the design
/// measured. Against a barn whose master has gone — which is exactly the barn
/// whose cells are about to be marked stale — this blocks for ssh's
/// `ConnectTimeout`, 10 s, with the TUI frozen behind it. That is the upper
/// bound to design a stale-cell jump around, not 180 ms.
pub fn select_window(barn: &Barn, window_index: u32) -> Result<()> {
    ssh::run(barn, &select_window_command(window_index), SELECT_OPTS)?;
    Ok(())
}

/// One frame from one barn.
#[derive(Debug, Clone)]
pub struct RemoteFrame {
    pub barn: String,
    /// Windows worth showing: window 0 is already gone, matching the script.
    pub windows: Vec<TmuxWindow>,
    /// pane id → rendered screen. One entry per window in [`Self::windows`],
    /// empty for a window whose capture did not arrive. Pane ids collide across
    /// hosts (`%1` exists everywhere), so this map is never merged with another
    /// barn's or with the local one.
    pub captures: HashMap<String, Vec<String>>,
    /// **Sanitized** pane id → signal, exactly as the files are named on the
    /// barn. Not filtered for freshness — see [`Self::fresh_signal`].
    pub signals: HashMap<String, SessionSignal>,
    /// The barn's own `date +%s` at the moment the frame was built.
    pub barn_now: u64,
}

impl RemoteFrame {
    /// The signal for a pane, judged against the **barn's** clock.
    ///
    /// [`signals::read_signal`] drops anything older than
    /// [`signals::SIGNAL_MAX_AGE_SECS`] against the local clock. Applying that
    /// rule to a remote signal compares the barn's `updated` with *our* `now`,
    /// so a barn running more than five minutes behind has every status
    /// discarded and renders a wall of blank cells with no error anywhere.
    ///
    /// Takes a raw pane id and sanitizes it, so callers use the same `%33` they
    /// hold for local lookups. Sanitizing is idempotent, so an already-sanitized
    /// id works too.
    pub fn fresh_signal(&self, pane_id: &str) -> Option<&SessionSignal> {
        let sig = self.signals.get(&signals::sanitize_pane_id(pane_id))?;
        if self.barn_now.saturating_sub(sig.updated) > signals::SIGNAL_MAX_AGE_SECS {
            return None;
        }
        Some(sig)
    }
}

/// Split accumulated stdout into complete frame bodies plus the unconsumed tail.
///
/// The tail is whatever came after the last frame sentinel — a partial frame
/// still being written. It is never a frame and must be kept, not parsed.
pub(crate) fn split_frames(buf: &str) -> (Vec<&str>, &str) {
    let mut frames = Vec::new();
    let mut rest = buf;
    while let Some(i) = rest.find(FRAME_SENTINEL) {
        frames.push(&rest[..i]);
        rest = &rest[i + FRAME_SENTINEL.len()..];
    }
    (frames, rest)
}

/// Parse one frame body — the bytes between two frame sentinels.
///
/// Returns `None` for anything that is not a whole frame. A truncated frame is
/// rejected outright rather than half parsed: half a frame renders as a barn
/// that suddenly lost most of its sessions, which is indistinguishable from the
/// real thing.
pub fn parse_frame(barn: &str, body: &str) -> Option<RemoteFrame> {
    // Every write the script makes ends in a newline, so a body that does not
    // was cut mid line. Without this a frame truncated inside `date +%s` parses
    // with a `barn_now` of, say, 1 — structurally fine, and every signal on the
    // barn then looks decades old and gets dropped.
    if !body.ends_with('\n') {
        return None;
    }

    let mut sections = body.splitn(3, SPLIT_SENTINEL);
    let (win_sec, cap_sec, sig_sec) = (sections.next()?, sections.next()?, sections.next()?);
    if body.matches(SPLIT_SENTINEL).count() != 2 {
        return None;
    }

    // Whole-line discipline, the same guard `ssh::parse_probe` uses against
    // MOTDs: `bash -lc` sources the profile, and an rc file that echoes prepends
    // its output to the first frame. Junk has no tabs, so it fails to parse and
    // is dropped instead of becoming a phantom window.
    let windows: Vec<TmuxWindow> = win_sec
        .lines()
        .filter_map(tmux::parse_window_line)
        // Property 4: skip exactly what the script skips, or every capture after
        // a skipped window lands in the wrong cell.
        .filter(|w| w.index != 0 && !w.pane_id.is_empty())
        .collect();

    let mut chunks: Vec<Vec<String>> = cap_sec
        .split(SEP_SENTINEL)
        .map(|c| c.trim_matches('\n').lines().map(|l| l.to_string()).collect())
        .collect();
    // A window whose capture never arrived still owes us a slot. The extra
    // chunk the trailing separator leaves behind needs no handling — `zip` stops
    // at the shorter side.
    chunks.resize(windows.len(), Vec::new());

    let captures: HashMap<String, Vec<String>> = windows
        .iter()
        .map(|w| w.pane_id.clone())
        .zip(chunks)
        .collect();

    let mut lines = sig_sec.lines().filter(|l| !l.trim().is_empty());
    // No clock means no way to judge freshness, so the frame is unusable.
    let barn_now: u64 = lines.next()?.trim().parse().ok()?;

    let mut signals = HashMap::new();
    for line in lines {
        if let Some((id, json)) = line.split_once('\t') {
            if let Some(sig) = signals::parse_signal(json) {
                signals.insert(id.to_string(), sig);
            }
        }
    }

    Some(RemoteFrame {
        barn: barn.to_string(),
        windows,
        captures,
        signals,
        barn_now,
    })
}

// ===========================================================================
// The transport
// ===========================================================================

/// What a stream tells its owner. [`RemoteStreams::drain`] reads every field.
#[derive(Debug)]
pub enum RemoteEvent {
    /// One complete frame. The barn is alive and this is what it looks like.
    Frame(RemoteFrame),
    /// The stream is over and will send nothing further. The owner keeps the
    /// last good frame and renders the barn stale rather than dropping its
    /// cells, which would renumber everything after them.
    Failed { barn: String, error: String },
}

/// How long a stream may go without completing a frame before it is declared
/// dead.
///
/// **A stalled barn hangs a naive reader rather than failing it.** The frames
/// are one second apart, but the failure this guards is not a slow barn — it is
/// a wedged remote tmux with the ssh channel still wide open and nothing
/// arriving. `read` blocks on that forever, so without a deadline `Failed` is
/// reachable only from EOF and read errors and the grid shows a frozen barn as
/// a healthy one indefinitely. RG-2 hung its own test suite for three minutes
/// demonstrating it.
///
/// Ten seconds, not two: the first frame comes after ssh has connected and
/// `bash -l` has sourced a profile, and a barn is not dead for being slow to
/// log in.
pub(crate) const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Bytes pulled off the child per `read`. One frame is ~8 KB after `tail -n 40`.
const READ_CHUNK: usize = 16 * 1024;

/// One barn's live frame stream: a child process and the thread reading it.
///
/// Dropping it is the shutdown; there is no other. See [`Drop`].
pub struct RemoteStream {
    // No barn name. RG-3 kept one here for "the registry that owns the
    // stream", but the registry keys its map by barn name and the reader thread
    // carries its own copy for the events it emits, so a third would only be a
    // second source of truth to keep in step.
    child: Child,
    /// Set by [`Drop`] before the kill, so the reader can tell "we shut this
    /// down" from "the barn went away". Without it, leaving the grid reports
    /// every connected barn as failed on the way out.
    stopping: Arc<AtomicBool>,
}

/// The ssh invocation behind a stream.
///
/// **No `tty: true`.** A remote pty would give the loop a controlling terminal,
/// and the SIGPIPE it takes on its next write is the only thing that ends it
/// when the connection goes — see the design's teardown note. `batch` because a
/// stream that stops at a passphrase prompt stops behind a full-screen TUI,
/// where nobody can see it or answer it.
fn stream_command(barn: &Barn) -> Result<Command> {
    ssh::command(
        barn,
        &frame_command(),
        Opts { batch: true, ..Default::default() },
    )
}

impl RemoteStream {
    /// Open a stream to a barn. One ssh channel, live for as long as the grid.
    pub fn spawn(barn: &Barn, tx: Sender<RemoteEvent>) -> Result<Self> {
        Self::from_command(&barn.name, stream_command(barn)?, tx, STALL_TIMEOUT)
    }

    /// Start a stream over an already-built command.
    ///
    /// This is the seam. [`Self::spawn`] does nothing but build the ssh
    /// `Command`, and nothing past this point knows or cares whether the child
    /// is ssh — which is what makes the reader and the teardown testable
    /// against a locally spawned `bash` running [`FRAME_SCRIPT`], with no barn
    /// and no network anywhere in the test.
    ///
    /// The caller owns everything about the command except its stdio, which is
    /// fixed here because getting it wrong is a bug in every caller equally.
    pub(crate) fn from_command(
        barn: &str,
        mut cmd: Command,
        tx: Sender<RemoteEvent>,
        stall: Duration,
    ) -> Result<Self> {
        // stdin: ssh forwards its own stdin to the remote command, so a child
        // that inherits ours sits reading the terminal and races crossterm for
        // every keystroke the user aims at the grid.
        // stderr: ssh reports warnings and "Connection closed" there, and
        // inherited they paint straight over the rendered cells.
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("failed to start the stream for '{barn}': {e}"))?;
        let stdout = child.stdout.take().expect("stdout was just set to a pipe");

        let stopping = Arc::new(AtomicBool::new(false));

        // Two threads, not one. A blocking `read` cannot be given a deadline
        // from the standard library alone, and the deadline is the whole point
        // (see STALL_TIMEOUT). So the pump does the blocking read and the
        // assembler does the timing, with a channel between them.
        //
        // Neither handle is kept, and `Drop` deliberately does not join.
        // Joining the pump would deadlock in exactly the case that matters: the
        // child's forked subshells inherit the write end of the pipe, so `read`
        // does not return 0 the moment the child dies. Killing and reaping the
        // child is what ends both threads — the pump on the EOF that follows,
        // the assembler on the disconnect after it.
        let (raw_tx, raw_rx) = mpsc::channel::<Result<Vec<u8>, String>>();
        thread::spawn(move || pump(stdout, raw_tx));

        let name = barn.to_string();
        let flag = Arc::clone(&stopping);
        thread::spawn(move || assemble(name, raw_rx, tx, flag, stall));

        Ok(RemoteStream { child, stopping })
    }
}

impl Drop for RemoteStream {
    /// Kill **and** reap.
    ///
    /// `kill` alone leaves a `<defunct>` ssh per barn per grid open, for the
    /// rest of the TUI's life. `wait` is not a courtesy; it is the other half of
    /// the teardown.
    fn drop(&mut self) {
        // Before the kill, so the reader sees the flag set by the time the pipe
        // closes. A shutdown that reported `Failed` would mark every barn stale
        // every time the user left the grid.
        self.stopping.store(true, Ordering::SeqCst);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Blocking reads off the child's stdout, forwarded verbatim.
///
/// Bytes, never text. A read boundary lands wherever the kernel put it, and
/// decoding each chunk on its own replaces any multi-byte glyph straddling that
/// boundary with `U+FFFD`. Panes are full of box drawing.
fn pump(mut out: ChildStdout, tx: Sender<Result<Vec<u8>, String>>) {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match out.read(&mut buf) {
            // EOF. The disconnect is the message; nothing to send.
            Ok(0) => break,
            Ok(n) => {
                if tx.send(Ok(buf[..n].to_vec())).is_err() {
                    break; // the assembler is gone
                }
            }
            // A signal landing mid-read is not a dead barn. `read` on a pipe is
            // not restarted for us the way `read_to_end` is, and the TUI runs
            // under a runtime that handles signals, so treating EINTR as a
            // failure would drop a perfectly healthy stream on a window resize.
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                break;
            }
        }
    }
}

/// One past the end of the last frame sentinel in `buf`, if there is one.
///
/// Searched over bytes rather than decoded text so the accumulator never has to
/// decode a partial frame. The sentinel is ASCII, so the index it returns is
/// always a character boundary.
fn last_sentinel_end(buf: &[u8]) -> Option<usize> {
    let pat = FRAME_SENTINEL.as_bytes();
    if buf.len() < pat.len() {
        return None;
    }
    buf.windows(pat.len())
        .rposition(|w| w == pat)
        .map(|i| i + pat.len())
}

/// Accumulate the pump's output into whole frames, on a deadline.
///
/// Ends in exactly one way — by reporting a reason — so a stream is never
/// silently over.
fn assemble(
    barn: String,
    rx: Receiver<Result<Vec<u8>, String>>,
    tx: Sender<RemoteEvent>,
    stopping: Arc<AtomicBool>,
    stall: Duration,
) {
    let mut buf: Vec<u8> = Vec::new();
    let mut deadline = Instant::now() + stall;

    let reason = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let got = if left.is_zero() {
            Err(RecvTimeoutError::Timeout)
        } else {
            rx.recv_timeout(left)
        };

        let chunk = match got {
            Ok(Ok(chunk)) => chunk,
            Ok(Err(e)) => break format!("reading from '{barn}' failed: {e}"),
            Err(RecvTimeoutError::Timeout) => {
                break format!("no frame from '{barn}' in {}s", stall.as_secs())
            }
            Err(RecvTimeoutError::Disconnected) => break format!("the stream to '{barn}' ended"),
        };
        buf.extend_from_slice(&chunk);

        // Everything up to and including the LAST sentinel is whole frames.
        // Whatever follows is a frame still being written: it stays in the
        // buffer and is never parsed. At EOF it is simply dropped, which is the
        // whole of `partial_output_at_eof_is_discarded_not_emitted_as_a_frame`.
        let Some(end) = last_sentinel_end(&buf) else {
            continue;
        };
        let whole = String::from_utf8_lossy(&buf[..end]).into_owned();
        buf.drain(..end);

        // A sentinel arrived, so the barn is still completing frames — even if
        // one of them turns out to be unparseable.
        deadline = Instant::now() + stall;

        let (bodies, tail) = split_frames(&whole);
        debug_assert!(tail.is_empty(), "cut at a sentinel, so nothing can be left over");

        for body in bodies {
            // Login noise before the first frame lands in `bodies[0]`;
            // `parse_frame` drops it line by line. A body that fails outright is
            // one frame we cannot trust, not a dead barn — skip it and wait for
            // the next.
            if let Some(frame) = parse_frame(&barn, body) {
                if tx.send(RemoteEvent::Frame(frame)).is_err() {
                    return; // the owner is gone; nothing left to report to
                }
            }
        }
    };

    if !stopping.load(Ordering::SeqCst) {
        let _ = tx.send(RemoteEvent::Failed { barn, error: reason });
    }
}

// ===========================================================================
// The registry
// ===========================================================================

/// First wait after a stream dies before another ssh channel is opened to that
/// barn.
///
/// Short, because most stream deaths are not deaths: a lid closing, a wifi
/// handover, a barn rebooting. The design's target is that "a barn that drops
/// for ten seconds should come back on its own", and 2s doubling clears that
/// inside three attempts.
pub(crate) const RETRY_BASE: Duration = Duration::from_secs(2);

/// Ceiling on the doubling. A barn that is genuinely gone costs one handshake a
/// minute instead of the four a second an unguarded reconcile would spend —
/// each of which blocks a child for ssh's full `ConnectTimeout`.
pub(crate) const RETRY_MAX: Duration = Duration::from_secs(60);

/// How long to wait before the `attempts`-th retry. 1-based: the first failure
/// waits [`RETRY_BASE`].
///
/// Doubling rather than a fixed interval because the two failures this has to
/// serve are opposites. A barn that blipped wants to be retried almost at once;
/// a barn that has been unreachable for an hour wants to be left alone. A fixed
/// interval can only be wrong for one of them.
pub(crate) fn retry_delay(attempts: u32) -> Duration {
    let shift = attempts.saturating_sub(1).min(31);
    RETRY_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(RETRY_MAX)
        .min(RETRY_MAX)
}

/// Why a barn has no stream, and when it may have one again.
#[derive(Debug)]
struct Failure {
    /// The reason the stream ended.
    ///
    /// **Read by tests only, and deliberately.** The stream child's stderr is
    /// nulled — correctly, ssh's chatter would paint straight over the rendered
    /// cells — so a network failure arrives here as "the stream to 'X' ended"
    /// whatever actually happened. A cell can say STALE; it cannot say why, and
    /// putting that sentence on screen would spend the header's last columns
    /// telling the user something the badge already told them.
    ///
    /// It stays because it is the only thing that tells the three failures
    /// apart — the stall deadline, a read error, and a closed pipe — plus the
    /// spawn-time class ("barn 'ghost' has no host configured") that never
    /// reaches the channel at all. `a_failed_barn_keeps_its_last_frame_for_
    /// stale_rendering` and `a_barn_that_cannot_be_reached_is_recorded_rather_
    /// than_spawning_a_child` both read it. Whoever re-plumbs stderr, or gives
    /// yeehaw a debug log, takes this `allow` off.
    #[allow(dead_code)]
    error: String,
    /// Consecutive failures with no frame in between. The exponent of the
    /// backoff.
    attempts: u32,
    /// Earliest instant [`RemoteStreams::reconcile`] may open a new channel.
    next: Instant,
}

/// One stream per connected barn, and the last thing each barn had to say.
///
/// Owned by the app for the lifetime of the process, but only ever *populated*
/// while the session grid is open: [`Self::reconcile`] opens the ssh channels
/// and [`Self::shutdown`] closes every one of them, so a closed grid costs
/// nothing.
///
/// The three maps are all keyed by **barn name** and they are deliberately not
/// one map:
///
/// - `streams` is what is running now.
/// - `frames` is the last good frame per barn, kept past a failure so a dead
///   barn renders dim and STALE in place rather than having its cells vanish
///   and renumber everything after them.
/// - `failed` is why a barn has no stream and when it may have one again. It is
///   what STALE renders from ([`Self::stale`]) *and* the backoff that stops
///   [`Self::reconcile`] reopening a hopeless ssh channel four times a second.
///   It is the one thing here that outlives [`Self::shutdown`], because
///   `reconcile` runs on grid open as well as on the tick and a forgotten
///   backoff is a free handshake per `v`.
///
/// RG-5 owns one of these as `App::remote_grid`, so the struct-wide and
/// impl-wide `allow(dead_code)` that used to sit here are gone. Two members
/// still carry a narrow one each; both say who takes it off.
pub struct RemoteStreams {
    streams: HashMap<String, RemoteStream>,
    frames: HashMap<String, RemoteFrame>,
    failed: HashMap<String, Failure>,
    tx: Sender<RemoteEvent>,
    rx: Receiver<RemoteEvent>,
}

impl RemoteStreams {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        RemoteStreams {
            streams: HashMap::new(),
            frames: HashMap::new(),
            failed: HashMap::new(),
            tx,
            rx,
        }
    }

    /// Bring the running streams in line with the barns that are connected
    /// *right now*: spawn for barns that gained a session, drop for barns that
    /// lost one.
    ///
    /// Called on **every 250ms tick**, not only when the grid opens — so that
    /// connecting to a barn from another window brings its sessions in without
    /// reopening the grid. Which makes idempotence load-bearing: a reconcile
    /// that respawned a live stream would open four ssh channels a second per
    /// barn.
    ///
    /// `connected` holds tmux **session** names, straight out of
    /// `list-sessions`. Barn names are not session names — barn `camera pi`
    /// lives in session `yh-barn-camera-pi-<hash>` — so the mapping runs
    /// forwards through [`tmux::barn_session_name`], the direction that is not
    /// lossy. Never [`tmux::barn_session_target`]: its `=` prefix exists for
    /// `-t` arguments and appears in nothing `list-sessions` prints. Both
    /// mistakes fail the same way, which is the reason this is spelled out —
    /// nothing matches, no stream is ever spawned, no error is raised anywhere,
    /// and the grid simply stays empty.
    // Not on the app's hot path: `app::tick_remote_streams` goes through
    // `reconcile_with` so that the "no stream exists off the grid" guard and
    // the spawn sit together in one function a test can drive with a recording
    // spawner. This stays as the un-injected form — the one the registry's own
    // `a_barn_that_cannot_be_reached_is_recorded_rather_than_spawning_a_child`
    // uses to exercise the real `RemoteStream::spawn`.
    #[allow(dead_code)]
    pub fn reconcile(&mut self, barns: &[Barn], connected: &HashSet<String>) {
        self.reconcile_with(barns, connected, RemoteStream::spawn)
    }

    /// [`Self::reconcile`] with the spawn injected.
    ///
    /// The same seam as [`RemoteStream::from_command`], one level up.
    /// [`Self::reconcile`]'s entire job is deciding *which* barns should have a
    /// stream; it never touches ssh itself. Handing it a spawner lets every one
    /// of those decisions be tested against real streams over local children,
    /// with no barn and no network in the suite.
    pub(crate) fn reconcile_with<F>(
        &mut self,
        barns: &[Barn],
        connected: &HashSet<String>,
        spawn: F,
    ) where
        F: Fn(&Barn, Sender<RemoteEvent>) -> Result<RemoteStream>,
    {
        self.reconcile_at(Instant::now(), barns, connected, spawn)
    }

    /// [`Self::reconcile_with`] with the clock injected too.
    ///
    /// The backoff is the only thing here that has an opinion about time, and a
    /// test that had to *sleep* through it would be a test that either takes
    /// [`RETRY_BASE`] seconds or proves nothing. Handing the instant in lets the
    /// retry schedule be driven exactly — one tick before it is due, one tick
    /// after — with no sleeping and no flake.
    pub(crate) fn reconcile_at<F>(
        &mut self,
        now: Instant,
        barns: &[Barn],
        connected: &HashSet<String>,
        spawn: F,
    ) where
        F: Fn(&Barn, Sender<RemoteEvent>) -> Result<RemoteStream>,
    {
        let live: Vec<&Barn> = barns
            .iter()
            .filter(|b| connected.contains(&tmux::barn_session_name(&b.name)))
            .collect();
        let wanted: HashSet<&str> = live.iter().map(|b| b.name.as_str()).collect();

        // Disconnecting a barn takes its cells off the grid, so everything the
        // registry knows about it goes at once — the stream (dropped, which
        // kills and reaps), the last frame, and any failure. A frame left
        // behind would keep painting a barn the user just closed.
        //
        // `RemoteStream::drop` sets `stopping` before the kill, so none of this
        // reports the barn as failed. That is the one thing this path must not
        // lose: without it, every deliberate teardown looks like a dead barn.
        self.streams.retain(|barn, _| wanted.contains(barn.as_str()));
        self.frames.retain(|barn, _| wanted.contains(barn.as_str()));
        self.failed.retain(|barn, _| wanted.contains(barn.as_str()));

        for barn in live {
            // Already streaming.
            if self.streams.contains_key(&barn.name) {
                continue;
            }
            // Or failed, and not yet due for another try. The wait is the whole
            // guard: without one, this opens four ssh handshakes a second
            // against a host that is not answering, each blocking a child for
            // ConnectTimeout.
            //
            // That is not a hypothetical. A `yh-barn-*` session exists from the
            // moment `tmux::connect_to_barn` creates it, and the `yeehaw
            // connect` inside it renders "unreachable" and offers a retry
            // *without exiting* — so a barn sitting on that error screen is in
            // `connected` exactly like a healthy one. Being connected means a
            // session exists, never that ssh works.
            if self.failed.get(&barn.name).is_some_and(|f| now < f.next) {
                continue;
            }
            // Past the wait, the barn is retried — but it stays marked stale
            // until a frame actually arrives. A spawn is not an answer: against
            // a dead host the child sits in ssh's ConnectTimeout for ten
            // seconds, and clearing the marking here would un-dim the cells for
            // all of it on the strength of nothing.
            match spawn(barn, self.tx.clone()) {
                Ok(stream) => {
                    self.streams.insert(barn.name.clone(), stream);
                }
                // A barn that cannot even produce an ssh argv fails here rather
                // than through the channel, but it is the same kind of failure
                // and the grid shows it the same way.
                Err(e) => {
                    self.record_failure(barn.name.clone(), e.to_string(), now);
                }
            }
        }
    }

    /// A barn has no stream, and this is why and when it may have one again.
    ///
    /// One place, because the two ways a stream can fail — the channel reporting
    /// it and `spawn` refusing to even build an argv — must not disagree about
    /// the backoff. `attempts` counts consecutive failures with no frame in
    /// between, so a barn that flaps every few minutes keeps being retried
    /// quickly while one that has never answered is left alone.
    fn record_failure(&mut self, barn: String, error: String, now: Instant) {
        let attempts = self.failed.get(&barn).map_or(0, |f| f.attempts) + 1;
        self.failed.insert(
            barn,
            Failure { error, attempts, next: now + retry_delay(attempts) },
        );
    }

    /// Take everything the streams have sent since the last tick.
    ///
    /// **Non-blocking, always.** This runs on the thread that paints, so a
    /// drain that waited on a frame would freeze the TUI between them — and
    /// most ticks have nothing pending, since frames are a second apart and
    /// ticks are 250ms.
    ///
    /// Events for a barn that is not currently streaming are dropped. That is
    /// what keeps a `Failed` racing a deliberate teardown — decided by the
    /// reader an instant before `stopping` was set — from marking a barn that
    /// the user simply disconnected.
    pub fn drain(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                RemoteEvent::Frame(frame) => {
                    if !self.streams.contains_key(&frame.barn) {
                        continue;
                    }
                    // A frame is the only proof a barn is back: the stale
                    // marking goes, and with it the backoff, so the next drop
                    // is retried as promptly as the first was. RG-4 left this
                    // out deliberately — nothing there could produce a frame
                    // after a failure, and it declined to ship untested code
                    // for it. The retry above is what produces it.
                    self.failed.remove(&frame.barn);
                    self.frames.insert(frame.barn.clone(), frame);
                }
                RemoteEvent::Failed { barn, error } => {
                    // Removing the stream drops it, which kills and reaps the
                    // child. Keeping a failed stream in the map instead would
                    // leave a defunct ssh per dead barn for the rest of the
                    // TUI's life — and a *stalled* barn's child is not even
                    // dead, just wedged with the channel still open.
                    if self.streams.remove(&barn).is_none() {
                        continue;
                    }
                    // The frame is not touched: the cells stay, dimmed, so a
                    // barn dying does not renumber the ones after it.
                    self.record_failure(barn, error, Instant::now());
                }
            }
        }
    }

    /// Close every stream. The grid is no longer open.
    ///
    /// Dropping the streams is the teardown — `stopping`, kill, reap — and the
    /// frames go with them so the next grid open starts from what the barns say
    /// then, not from what they said minutes ago.
    ///
    /// **`failed` deliberately stays.** RG-4 cleared it here, on the same
    /// reasoning as the frames, and that was right while it was only a rendering
    /// input. It is now also the backoff's only memory, and `reconcile` runs on
    /// every grid *open* as well as on the tick — so clearing it would give a
    /// barn parked on its own "unreachable" screen a fresh handshake per `v`,
    /// with the wait starting over each time. What survives is a claim about
    /// right now ("this barn has no stream, and here is when it may have one"),
    /// not stale news: it names the barn in the header, and it is dropped the
    /// moment a frame arrives or the barn is disconnected.
    ///
    /// The channel is replaced rather than reused. A stream that had genuinely
    /// failed a moment before the shutdown may have left a `Failed` in it with
    /// nobody reading, and the next grid open spawns a *new* stream for that
    /// same barn — which the stale event would land on, rendering a barn STALE
    /// for a death one grid ago. Dropping the receiver strands anything still
    /// in flight where it can do no harm.
    pub fn shutdown(&mut self) {
        self.streams.clear();
        self.frames.clear();

        let (tx, rx) = mpsc::channel();
        self.tx = tx;
        self.rx = rx;
    }

    /// Every barn's last frame, live or stale, keyed by barn name. A barn that
    /// has not completed one yet — a stream mid-login, most often — is simply
    /// absent.
    ///
    /// The whole map rather than one barn at a time, because the grid's job is
    /// to merge *all* of them with the local windows into a single sorted list
    /// of cells; a per-barn accessor would only be called in a loop over this.
    /// (RG-5 shipped that per-barn `frame()` for RG-6 to un-dead-code. It has no
    /// caller here now that the merge exists, so it went rather than carrying an
    /// `allow(dead_code)` no later task would ever remove.)
    pub fn frames(&self) -> &HashMap<String, RemoteFrame> {
        &self.frames
    }

    /// Barns with no live stream: their cells are the last frame rather than a
    /// current one, and the grid renders them STALE.
    ///
    /// Names only. The reason a stream died is kept ([`Failure::error`]) but
    /// never shown, because with the child's stderr nulled it is always "the
    /// stream to 'X' ended" — a sentence that tells the user nothing the badge
    /// has not already said.
    ///
    /// A barn can be in here with **no frame at all**: one that has never
    /// answered has no cells to mark, and naming it in the header is then the
    /// only thing on screen that explains where its sessions went.
    pub fn stale(&self) -> HashSet<&str> {
        self.failed.keys().map(String::as_str).collect()
    }
}

/// Test-only.
///
/// Some of these helpers are `pub(crate)` because the lifecycle tests in
/// `app.rs` drive **real** streams over local children, and they have to tear
/// them down the same way this module does: [`tests::local_child`] puts every
/// child in a process group of its own and [`tests::GroupGuard`] kills the
/// whole group, because a child's forked subshells hold the same pipe and
/// outlive a kill aimed at the child alone. Hand-rolling a spawn over there is
/// how RG-2 left `capture-pane` loops running on this machine, twice.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::signals::SessionStatus;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    const SPLIT: &str = SPLIT_SENTINEL;
    const SEP: &str = SEP_SENTINEL;
    const FRAME: &str = FRAME_SENTINEL;

    /// A `list-windows` line in `WINDOW_LIST_FORMAT`. Ten tab separated fields.
    fn win(index: &str, name: &str, active: &str, pane: &str) -> String {
        format!("{index}\t{name}\t{active}\t{pane}\ttitle\tclaude\t100\tclaude\tproj\t")
    }

    fn frame_body(windows: &[String], captures: &[&str], now: u64, sigs: &[(&str, &str)]) -> String {
        let mut s = String::new();
        for w in windows {
            s.push_str(w);
            s.push('\n');
        }
        s.push_str(SPLIT);
        s.push('\n');
        for c in captures {
            s.push_str(c);
            s.push('\n');
            s.push_str(SEP);
            s.push('\n');
        }
        s.push_str(SPLIT);
        s.push('\n');
        s.push_str(&format!("{now}\n"));
        for (id, json) in sigs {
            s.push_str(&format!("{id}\t{json}\n"));
        }
        s
    }

    // === the parser ========================================================

    #[test]
    fn parses_a_full_frame_into_windows_captures_and_signals() {
        let body = frame_body(
            &[win("1", "api", "0", "%7"), win("2", "web", "1", "%9")],
            &["api line one\napi line two", "web only line"],
            1_700_000_000,
            &[("_7", r#"{"status":"waiting","updated":1699999990}"#)],
        );

        let f = parse_frame("guided", &body).expect("a whole frame parses");
        assert_eq!(f.barn, "guided");
        assert_eq!(f.barn_now, 1_700_000_000);

        let names: Vec<&str> = f.windows.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["api", "web"]);
        assert!(f.windows[1].active, "window_active must survive the trip");

        assert_eq!(
            f.captures.get("%7").map(Vec::as_slice),
            Some(&["api line one".to_string(), "api line two".to_string()][..])
        );
        assert_eq!(
            f.captures.get("%9").map(Vec::as_slice),
            Some(&["web only line".to_string()][..])
        );

        let sig = f.signals.get("_7").expect("signal for the sanitized pane id");
        assert_eq!(sig.status, SessionStatus::Waiting);
        assert_eq!(sig.updated, 1_699_999_990);
    }

    #[test]
    fn a_remote_signal_is_looked_up_by_the_raw_pane_id() {
        // Remote filenames arrive already sanitized (`%7` -> `_7`); callers hold
        // the raw id. If the lookup did not sanitize, every remote cell would
        // miss its status while the map sat there full.
        let body = frame_body(
            &[win("1", "api", "0", "%7")],
            &["x"],
            1_700_000_000,
            &[("_7", r#"{"status":"working","updated":1699999999}"#)],
        );
        let f = parse_frame("guided", &body).unwrap();
        assert_eq!(
            f.fresh_signal("%7").map(|s| s.status.clone()),
            Some(SessionStatus::Working)
        );
    }

    #[test]
    fn a_remote_signal_is_judged_against_the_barns_clock_not_ours() {
        // A barn an hour behind us. Against the local clock every one of its
        // signals is older than SIGNAL_MAX_AGE_SECS and gets dropped, so the
        // whole barn renders statusless with nothing to explain why.
        let barn_now = 1_700_000_000u64;
        let body = frame_body(
            &[win("1", "api", "0", "%7")],
            &["x"],
            barn_now,
            &[
                ("_7", &format!(r#"{{"status":"waiting","updated":{}}}"#, barn_now - 10)),
                ("_9", &format!(r#"{{"status":"working","updated":{}}}"#, barn_now - 9999)),
            ],
        );
        let f = parse_frame("guided", &body).unwrap();

        assert!(
            f.fresh_signal("%7").is_some(),
            "10s old on the barn's clock is fresh however far off our clock is"
        );
        assert!(
            f.fresh_signal("%9").is_none(),
            "genuinely old on the barn's own clock must still be dropped"
        );
    }

    #[test]
    fn discards_login_noise_before_the_first_frame() {
        // `bash -lc` sources the profile; an rc file that echoes prepends junk to
        // stdout, and it lands in front of the first frame's window list. Same
        // class as the MOTD bug `parse_probe` guards with whole-line matching.
        let mut body = String::from(
            "Welcome to Ubuntu 22.04.3 LTS\n\
             *** System restart required ***\n\
             You have mail.\n",
        );
        body.push_str(&frame_body(
            &[win("4", "api", "0", "%7")],
            &["real capture"],
            1_700_000_000,
            &[],
        ));

        let f = parse_frame("guided", &body).expect("noise must not break the frame");
        assert_eq!(f.windows.len(), 1, "three banner lines became windows: {:?}", f.windows);
        assert_eq!(f.windows[0].name, "api");
        assert_eq!(
            f.captures.get("%7").map(Vec::as_slice),
            Some(&["real capture".to_string()][..]),
            "the banner shifted the captures off by three"
        );
    }

    #[test]
    fn a_window_with_no_capture_still_gets_a_slot() {
        // A window that appeared between the list and the capture loop, or whose
        // pane died mid frame, sends no chunk. It must still be a cell — three
        // windows and one capture, so the shortfall is real and not covered by
        // the empty chunk the trailing separator leaves behind.
        let body = frame_body(
            &[
                win("1", "api", "0", "%7"),
                win("2", "web", "0", "%9"),
                win("3", "job", "0", "%11"),
            ],
            &["api output"],
            1_700_000_000,
            &[],
        );
        let f = parse_frame("guided", &body).unwrap();
        assert_eq!(f.windows.len(), 3);
        assert_eq!(
            f.captures.get("%7").map(Vec::as_slice),
            Some(&["api output".to_string()][..])
        );
        for missing in ["%9", "%11"] {
            assert_eq!(
                f.captures.get(missing).map(Vec::as_slice),
                Some(&[][..]),
                "{missing} must own an empty screen, not another window's and not nothing"
            );
        }
    }

    #[test]
    fn window_zero_is_absent_because_the_script_skips_it() {
        // Window 0 is the dashboard itself and the script never captures it.
        // Keeping it in `windows` would shift every capture down by one, so the
        // parser has to drop it too.
        let body = frame_body(
            &[
                win("0", "yeehaw", "0", "%1"),
                win("1", "api", "0", "%7"),
                win("2", "web", "0", "%9"),
            ],
            &["api output", "web output"],
            1_700_000_000,
            &[],
        );
        let f = parse_frame("guided", &body).unwrap();

        assert!(
            !f.windows.iter().any(|w| w.index == 0),
            "window 0 is not a cell: {:?}",
            f.windows
        );
        assert_eq!(
            f.captures.get("%7").map(Vec::as_slice),
            Some(&["api output".to_string()][..]),
            "keeping window 0 shifted every capture by one"
        );
        assert_eq!(
            f.captures.get("%9").map(Vec::as_slice),
            Some(&["web output".to_string()][..])
        );
        assert!(!f.captures.contains_key("%1"));
    }

    #[test]
    fn window_ten_is_not_mistaken_for_window_zero() {
        let body = frame_body(&[win("10", "api", "0", "%7")], &["ten"], 1_700_000_000, &[]);
        let f = parse_frame("guided", &body).unwrap();
        assert_eq!(f.windows.len(), 1, "window 10 is a real window");
        assert_eq!(f.windows[0].index, 10);
    }

    #[test]
    fn a_truncated_frame_is_rejected_rather_than_half_parsed() {
        let whole = frame_body(
            &[win("1", "api", "0", "%7")],
            &["api output"],
            1_700_000_000,
            &[],
        );
        assert!(parse_frame("guided", &whole).is_some(), "control: the whole frame parses");

        // Cut off at every byte boundary: nothing shorter than the whole frame
        // may come back as a frame.
        for cut in 0..whole.len() {
            let partial = &whole[..cut];
            if !partial.is_char_boundary(cut) {
                continue;
            }
            assert!(
                parse_frame("guided", partial).is_none(),
                "a frame truncated at {cut} parsed anyway: {partial:?}"
            );
        }
    }

    #[test]
    fn a_frame_with_no_clock_is_rejected() {
        // Without `date +%s` there is no way to judge signal freshness, and a
        // frame whose signals section starts with a signal line means the clock
        // never made it.
        let body = frame_body(&[win("1", "api", "0", "%7")], &["x"], 1_700_000_000, &[])
            .replace("1700000000\n", "");
        assert!(parse_frame("guided", &body).is_none());
    }

    #[test]
    fn split_frames_keeps_a_partial_frame_out_of_the_stream() {
        let a = frame_body(&[win("1", "a", "0", "%1")], &["a"], 1, &[]);
        let b = frame_body(&[win("1", "b", "0", "%2")], &["b"], 2, &[]);
        let stream = format!("{a}{FRAME}\n{b}{FRAME}\nhalf a frame with no end");

        let (frames, rest) = split_frames(&stream);
        assert_eq!(frames.len(), 2);
        assert_eq!(rest, "\nhalf a frame with no end");
        assert_eq!(parse_frame("g", frames[0]).unwrap().barn_now, 1);
        assert_eq!(parse_frame("g", frames[1]).unwrap().barn_now, 2);
    }

    // === the script, read as text ==========================================

    #[test]
    fn sentinels_never_appear_as_literal_bytes_in_the_script() {
        // The whole point of building them with printf on the far side. If
        // someone "simplifies" the script by inlining the control characters,
        // they have to survive our quoting, ssh, and the remote shell as raw
        // bytes, and this fails.
        assert!(!FRAME_SCRIPT.contains('\u{1}'), "literal \\x01 in the script");
        assert!(!FRAME_SCRIPT.contains('\u{2}'), "literal \\x02 in the script");
        assert!(FRAME_SCRIPT.is_ascii(), "the wire must stay pure ASCII");
        for name in ["YHSPLIT", "YHSEP", "YHFRAME"] {
            assert!(
                FRAME_SCRIPT.contains(&format!("\\001\\002{name}\\002\\001\\n")),
                "{name} is not built by printf"
            );
        }
    }

    #[test]
    fn the_remote_window_format_is_byte_identical_to_the_local_one() {
        // `parse_window_line` is shared with the local grid. One extra field in
        // WINDOW_LIST_FORMAT and every remote field shifts, silently.
        assert!(
            FRAME_SCRIPT.contains(tmux::WINDOW_LIST_FORMAT),
            "the script's FMT has drifted from WINDOW_LIST_FORMAT"
        );
    }

    #[test]
    fn the_script_holds_no_single_quote_so_the_bash_wrapper_stays_readable() {
        assert!(!FRAME_SCRIPT.contains('\''));
        assert_eq!(frame_command(), format!("bash -lc '{FRAME_SCRIPT}'"));
    }

    // === the script through a real shell ===================================
    //
    // FRAME_SCRIPT is a shell program we never parse ourselves — sshd hands it
    // to the barn's shell. Its sibling PROBE_CMD shipped two bugs and both were
    // in how a real shell ran it, not in how it read. So these tests run the
    // actual constant through an actual shell rather than comparing it to a
    // hand-written expectation that could be wrong in the same way the constant
    // is. Same approach as `ssh::tests` and `tmux::tests::assert_sh_roundtrip`.

    /// Install `script` as an executable named `name` in `dir`.
    fn stub_bin(dir: &std::path::Path, name: &str, script: &str) {
        let bin = dir.join(name);
        std::fs::write(&bin, script).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }

    /// Seconds the script gets before the watchdog shoots it. Frames are one
    /// second apart and no test asks for more than three.
    const WATCHDOG_SECS: u32 = 15;

    /// `sh` that runs `$YH_FRAME_CMD` under a watchdog and exits when either the
    /// command or the timer finishes.
    ///
    /// This covers a child that goes completely silent, where a blocking read
    /// would never return. It is *not* enough on its own — `$!` is the pid of
    /// the subshell wrapping the `eval`, and killing it does not always reach
    /// the `bash` underneath, which then keeps the pipe open. The deadline in
    /// [`read_frames`] is what actually bounds the common case, and the process
    /// group kill there is what actually ends the loop.
    ///
    /// The timer redirects its own stdout away so it does not hold the pipe open
    /// after the command dies, and the command comes in on the environment so
    /// nothing has to be quoted into this wrapper twice. `eval` parses it with
    /// the same shell parser `sh -c` would.
    const WATCHDOG: &str = "eval \"$YH_FRAME_CMD\" & p=$!; \
                            { sleep $YH_WATCHDOG_SECS; kill $p; } >/dev/null 2>&1 & \
                            wait $p";

    /// Run `cmd` through a real `/bin/sh`, the way sshd runs a remote command,
    /// and read until `want` frame sentinels have arrived, the child stops
    /// writing, or the deadline passes. Returns everything read.
    ///
    /// **The deadline is not optional.** The property under test is "this loop
    /// never stops writing", and the failure mode is a loop that keeps talking
    /// but never completes a frame — against which an unbounded read is not a
    /// failing test, it is a hung one. Found the hard way: putting the frame
    /// `printf` behind a condition on purpose hung the suite for three minutes
    /// instead of turning it red.
    ///
    /// `/bin/sh` is spelled absolutely so a replaced PATH cannot change which
    /// shell runs.
    ///
    /// Teardown kills the child's whole **process group**, not just the child.
    /// The watchdog `sh` is not the loop — it forked a subshell that exec'd
    /// `bash` — so killing the child alone leaves the loop running until its
    /// next write hits the closed pipe. Measured: a full second of
    /// `pgrep -f YHFRAME` still returning a pid *after the test binary exited*.
    /// The design says it in as many words: SIGPIPE is the backstop, never the
    /// primary teardown.
    fn read_frames(cmd: &str, env: &[(&str, &str)], want: usize) -> String {
        let mut child = Command::new("/bin/sh")
            .args(["-c", WATCHDOG])
            .env("YH_FRAME_CMD", cmd)
            .env("YH_WATCHDOG_SECS", WATCHDOG_SECS.to_string())
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("sh should be runnable");
        let group = GroupGuard(child.id() as i32);

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(WATCHDOG_SECS as u64);
        let mut out: Vec<u8> = Vec::new();
        {
            let mut r = BufReader::new(child.stdout.take().expect("piped stdout"));
            let mut seen = 0usize;
            loop {
                let mut line = Vec::new();
                match r.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if String::from_utf8_lossy(&line).contains(FRAME) {
                    seen += 1;
                }
                out.extend_from_slice(&line);
                if seen >= want || std::time::Instant::now() > deadline {
                    break;
                }
            }
        }
        let pgid = child.id() as i32;
        let _ = child.kill();
        let _ = child.wait();
        drop(group); // the loop itself, which the child never was

        // Asserted rather than assumed, in every test that runs the script.
        //
        // The window is deliberately far shorter than the loop's `sleep 1`:
        // SIGPIPE cannot fire until the loop wakes for its next write, and we
        // stopped reading immediately after a frame, so almost a full second of
        // that sleep is still ahead. A kill that reaches the whole group clears
        // this in milliseconds; SIGPIPE on its own cannot, which is what makes
        // the assertion mean something.
        assert!(
            poll_until(Duration::from_millis(500), || loop_pids(pgid).is_empty()),
            "read_frames left the frame loop running: {:?}",
            loop_pids(pgid)
        );

        String::from_utf8_lossy(&out).to_string()
    }

    /// A barn in a box: a temp `$HOME` whose login profile prepends a stub
    /// `tmux` to PATH, plus whatever signal files the caller asked for.
    ///
    /// The profile is where the stub PATH has to be set, not the environment:
    /// `bash -l` sources `/etc/profile`, which on macOS runs `path_helper` and
    /// *replaces* PATH outright. Anything set before the login shell starts is
    /// gone by the time the script runs — which is the same reason property 3
    /// exists.
    fn barn_in_a_box(
        dir: &std::path::Path,
        window_lines: &str,
        signals: &[(&str, &str)],
        profile_noise: &str,
    ) {
        std::fs::write(dir.join("windows.txt"), window_lines).expect("windows");
        stub_bin(
            dir,
            "tmux",
            &format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                 list-windows) cat {d}/windows.txt ;;\n\
                 capture-pane) echo \"CAPTURED $5\" ;;\n\
                 *) exit 1 ;;\n\
                 esac\n",
                d = dir.display()
            ),
        );
        std::fs::write(
            dir.join(".bash_profile"),
            format!("PATH={d}:$PATH\n{profile_noise}", d = dir.display()),
        )
        .expect("profile");

        let sigdir = dir.join(".yeehaw").join("session-signals");
        std::fs::create_dir_all(&sigdir).expect("signals dir");
        for (name, json) in signals {
            std::fs::write(sigdir.join(format!("{name}.json")), json).expect("signal");
        }
    }

    fn box_env(dir: &std::path::Path) -> Vec<(String, String)> {
        vec![
            ("HOME".to_string(), dir.display().to_string()),
            ("PATH".to_string(), "/usr/bin:/bin:/usr/sbin:/sbin".to_string()),
        ]
    }

    /// Run the real `frame_command()` against a stubbed barn and parse whatever
    /// whole frames came back.
    fn frames_from_box(
        dir: &std::path::Path,
        window_lines: &str,
        signals: &[(&str, &str)],
        profile_noise: &str,
        want: usize,
    ) -> Vec<RemoteFrame> {
        barn_in_a_box(dir, window_lines, signals, profile_noise);
        let env = box_env(dir);
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let raw = read_frames(&frame_command(), &env, want);
        let (bodies, _) = split_frames(&raw);
        assert_eq!(
            bodies.len(),
            want,
            "wanted {want} frames from a real shell, got {}. raw output: {raw:?}",
            bodies.len()
        );
        bodies
            .iter()
            .map(|b| {
                parse_frame("boxed", b)
                    .unwrap_or_else(|| panic!("a real shell produced an unparseable frame: {b:?}"))
            })
            .collect()
    }

    #[test]
    fn the_script_skips_window_zero_and_captures_everything_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lines = format!(
            "{}\n{}\n{}\n",
            win("0", "yeehaw", "0", "%1"),
            win("1", "api", "0", "%7"),
            win("10", "web", "1", "%9"),
        );
        let frames = frames_from_box(dir.path(), &lines, &[], "", 1);
        assert_eq!(frames.len(), 1, "one whole frame");
        let f = &frames[0];

        assert_eq!(f.windows.len(), 2, "window 0 is not a cell: {:?}", f.windows);
        // The stub tmux echoes the target it was handed, so this is proof of
        // which pane each capture actually came from.
        assert_eq!(f.captures.get("%7").map(Vec::as_slice), Some(&["CAPTURED %7".to_string()][..]));
        assert_eq!(f.captures.get("%9").map(Vec::as_slice), Some(&["CAPTURED %9".to_string()][..]));
        assert!(!f.captures.contains_key("%1"), "window 0 was captured");
    }

    #[test]
    fn an_empty_window_name_does_not_shift_the_capture_target() {
        // tmux accepts `rename-window ""`, and tab is IFS *whitespace*: with
        // `IFS=<tab> read -r idx nm act pane rest` an empty field collapses and
        // every later field shifts left, so `$pane` becomes the pane *title* and
        // that window captures the wrong thing for as long as it exists. The
        // stub tmux echoes its target, so this catches the shift directly.
        let dir = tempfile::tempdir().expect("tempdir");
        let lines = format!("{}\n{}\n", win("1", "", "0", "%7"), win("2", "web", "0", "%9"));
        let frames = frames_from_box(dir.path(), &lines, &[], "", 1);
        let f = &frames[0];

        assert_eq!(f.windows.len(), 2, "an unnamed window is still a window");
        assert_eq!(
            f.captures.get("%7").map(Vec::as_slice),
            Some(&["CAPTURED %7".to_string()][..]),
            "the empty name shifted the pane id"
        );
        assert_eq!(f.captures.get("%9").map(Vec::as_slice), Some(&["CAPTURED %9".to_string()][..]));
    }

    #[test]
    fn login_noise_from_a_real_profile_never_becomes_a_window() {
        // Hazard 5 end to end: a `.bash_profile` that echoes, sourced by the
        // `-l` the script needs for its PATH, prints straight into the first
        // frame's window section.
        let dir = tempfile::tempdir().expect("tempdir");
        let lines = format!("{}\n", win("3", "api", "0", "%7"));
        let frames = frames_from_box(
            dir.path(),
            &lines,
            &[],
            "echo 'Welcome to Ubuntu 22.04'\necho '*** System restart required ***'\n",
            1,
        );
        let f = &frames[0];
        assert_eq!(f.windows.len(), 1, "the banner became windows: {:?}", f.windows);
        assert_eq!(f.windows[0].name, "api");
        assert_eq!(f.captures.get("%7").map(Vec::as_slice), Some(&["CAPTURED %7".to_string()][..]));
    }

    #[test]
    fn the_script_reads_signals_and_the_barn_clock_through_a_real_shell() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lines = format!("{}\n", win("1", "api", "0", "%7"));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let frames = frames_from_box(
            dir.path(),
            &lines,
            &[
                // Pretty printed on purpose: `tr -d "\n"` has to flatten it or
                // the frame gains lines that are not signals.
                ("_7", &format!("{{\n  \"status\": \"waiting\",\n  \"updated\": {now}\n}}\n")),
                ("_9", &format!("{{\"status\":\"error\",\"updated\":{now}}}")),
            ],
            "",
            1,
        );
        let f = &frames[0];

        assert!(
            f.barn_now.abs_diff(now) < 60,
            "the barn clock did not survive: {} vs {now}",
            f.barn_now
        );
        assert_eq!(
            f.fresh_signal("%7").map(|s| s.status.clone()),
            Some(SessionStatus::Waiting),
            "signals: {:?}",
            f.signals.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            f.fresh_signal("%9").map(|s| s.status.clone()),
            Some(SessionStatus::Error)
        );
    }

    #[test]
    fn the_script_keeps_an_unconditional_write_every_iteration() {
        // Property 2. The barn with the least possible to say: no windows, no
        // captures, no signals. It must still emit a whole frame every second,
        // because SIGPIPE on the next write is the only thing that will ever
        // kill this loop when the connection dies. A loop that goes quiet when
        // nothing changed is a loop running `capture-pane` forever on someone's
        // production box.
        let dir = tempfile::tempdir().expect("tempdir");
        let frames = frames_from_box(dir.path(), "", &[], "", 3);
        assert_eq!(frames.len(), 3, "an idle barn stopped emitting frames");
        for f in &frames {
            assert!(f.windows.is_empty());
            assert!(f.captures.is_empty());
            assert!(f.barn_now > 0, "every frame carries the clock");
        }
        assert!(
            frames[2].barn_now > frames[0].barn_now,
            "three frames arrived without the clock moving: the loop is not sleeping"
        );
    }

    #[test]
    fn the_script_reaches_bash_as_a_single_login_shell_argument() {
        // Property 3, and the quoting that carries it. A `bash` that prints its
        // argv proves both halves at once: the `-l` really reaches the shell,
        // and the whole script arrives as ONE argument instead of being split by
        // the shell sshd uses to run the remote command.
        let dir = tempfile::tempdir().expect("tempdir");
        stub_bin(
            dir.path(),
            "bash",
            "#!/bin/sh\nfor a in \"$@\"; do printf 'ARG[%s]\\n' \"$a\"; done\n",
        );
        let path = dir.path().display().to_string();

        let out = Command::new("/bin/sh")
            .args(["-c", &frame_command()])
            .env("PATH", &path)
            .output()
            .expect("sh should be runnable");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();

        let args: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("ARG[")?.strip_suffix(']'))
            .collect();

        assert_eq!(args.len(), 2, "expected `bash -lc <script>`, got {args:?}");
        assert_eq!(
            args[0], "-lc",
            "the stream must use a login shell, the same as the probe and the attach"
        );
        for fragment in ["list-windows", "capture-pane", "tail -n 40", "date +%s", "YHFRAME", "sleep 1"] {
            assert!(
                args[1].contains(fragment),
                "the script reached bash mangled — {fragment:?} missing"
            );
        }
    }

    #[test]
    fn the_real_frame_script_produces_parseable_frames_on_this_machine() {
        // No stubs: the exact constant, the exact command, a real bash, and this
        // machine's real tmux server. Skips rather than fails on a box with no
        // yeehaw session so CI stays green.
        let live = Command::new("tmux")
            .args(["has-session", "-t", "=yeehaw"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !live {
            eprintln!("skipping: no live `yeehaw` tmux session on this machine");
            return;
        }

        let raw = read_frames(&frame_command(), &[], 2);
        let (bodies, _) = split_frames(&raw);
        assert_eq!(bodies.len(), 2, "expected two frames, got {}", bodies.len());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        for (i, body) in bodies.iter().enumerate() {
            let f = parse_frame("this-machine", body)
                .unwrap_or_else(|| panic!("frame {i} did not parse: {body:?}"));

            assert!(!f.windows.is_empty(), "a live yeehaw session has windows");
            assert!(!f.windows.iter().any(|w| w.index == 0), "window 0 leaked in");
            assert_eq!(
                f.captures.len(),
                f.windows.len(),
                "every window owes a capture slot"
            );
            for w in &f.windows {
                assert!(
                    f.captures.contains_key(&w.pane_id),
                    "window {} ({}) has no capture slot",
                    w.index,
                    w.name
                );
            }
            assert!(
                f.captures.values().any(|c| !c.is_empty()),
                "not one pane on this machine rendered anything"
            );
            assert!(
                f.barn_now.abs_diff(now) < 60,
                "frame {i} clock {} is nowhere near {now}",
                f.barn_now
            );
        }

        assert!(
            bodies[1].len() > 0 && parse_frame("x", bodies[1]).unwrap().barn_now
                >= parse_frame("x", bodies[0]).unwrap().barn_now,
            "the second frame is older than the first"
        );
    }

    // === the remote jump ===================================================
    //
    // `select_window` is one blocking ssh exec on a keypress. Nothing here runs
    // it — the argv it builds is the whole behaviour worth guarding, and the one
    // character in it that matters fails *silently* when it is wrong.

    #[test]
    fn the_remote_select_target_is_exact_not_a_prefix_pattern() {
        // A bare `yeehaw:3` is a *prefix pattern* to tmux, and the failure is
        // the silent kind. Measured on tmux 3.6a against a scratch server
        // holding one session named `yeehaw-decoy` and none named `yeehaw`:
        //
        //   select-window -t 'yeehaw:0'   -> exit 0, selected yeehaw-decoy:0
        //   select-window -t '=yeehaw:0'  -> exit 1, "can't find session: yeehaw"
        //
        // The wrong one is the one that succeeds, so no exit status downstream
        // can catch it: the user simply lands somewhere else on their own box.
        let cmd = select_window_command(3);

        assert!(
            cmd.contains("=yeehaw:3"),
            "the target lost its `=` and is now a prefix pattern: {cmd:?}"
        );
        assert!(
            !cmd.contains("-t yeehaw:3"),
            "the target reached tmux unanchored: {cmd:?}"
        );
        assert_eq!(select_window_target(3), "=yeehaw:3");
    }

    #[test]
    fn the_select_target_needs_no_trailing_colon_unlike_its_option_setting_siblings() {
        // `set-option -t` and `send-keys -t` both take a target-*pane* and fail
        // on a bare session target; `select-window` takes a target-*window* and
        // does not. Verified on tmux 3.6a — `select-window -t '=yeehaw:2'`
        // succeeds and moves the active window to 2. Asserted so nobody
        // "fixes" this by generalising from the siblings.
        assert!(!select_window_target(2).ends_with(':'));
    }

    #[test]
    fn the_select_runs_under_a_login_shell_like_every_other_remote_command() {
        // Bug B of the barn-connect work: sshd's non-login shell has a PATH with
        // neither Homebrew nor `~/.local/bin`, so `tmux` is simply absent. The
        // probe, the attach and the frame stream all use `bash -lc` for this;
        // a jump that did not would fail on exactly the barns the grid is
        // already streaming happily.
        let cmd = select_window_command(4);
        assert!(cmd.starts_with("bash -lc "), "{cmd:?}");
        assert_eq!(cmd, format!("bash -lc 'tmux select-window -t =yeehaw:4'"));
    }

    #[test]
    fn the_select_never_stops_at_a_password_prompt() {
        // Batch mode, for a reason the stream has too: this runs behind a
        // full-screen TUI, where an ssh prompt is invisible and unanswerable.
        // Without it a jump to a barn with a passphrase-locked key hangs
        // yeehaw outright.
        //
        // `SELECT_OPTS` through the real `ssh_args` rather than the struct's
        // fields: what ssh is told is the claim, not what we wrote down.
        let args = ssh::ssh_args(&stream_barn(), SELECT_OPTS).expect("a configured barn");
        assert!(
            args.iter().any(|a| a == "BatchMode=yes"),
            "a jump can prompt for a password behind the TUI: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "-t"),
            "a jump asked for a remote tty it has no use for: {args:?}"
        );
    }

    #[test]
    fn the_select_command_reaches_tmux_as_one_exact_target_through_a_real_shell() {
        // The constant travels through our quoting, sshd's shell, and `bash -l`
        // before tmux ever sees it. Both of `PROBE_CMD`'s bugs were in how a
        // real shell ran it, so run this one too: a stub `tmux` that prints its
        // argv is proof of the bytes that arrive, not of the bytes we meant.
        let dir = tempfile::tempdir().expect("tempdir");
        stub_bin(
            dir.path(),
            "tmux",
            "#!/bin/sh\nfor a in \"$@\"; do printf 'ARG[%s]\\n' \"$a\"; done\n",
        );
        // `bash -l` sources `/etc/profile`, which on macOS runs `path_helper`
        // and replaces PATH outright — so the stub goes on PATH from the
        // profile in a temp $HOME, not from the environment.
        std::fs::write(
            dir.path().join(".bash_profile"),
            format!("PATH={d}:$PATH\n", d = dir.path().display()),
        )
        .expect("profile");

        let out = Command::new("/bin/sh")
            .args(["-c", &select_window_command(3)])
            .env("HOME", dir.path())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .output()
            .expect("sh should be runnable");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();

        let args: Vec<&str> = stdout
            .lines()
            .filter_map(|l| l.strip_prefix("ARG[")?.strip_suffix(']'))
            .collect();
        assert_eq!(
            args,
            ["select-window", "-t", "=yeehaw:3"],
            "tmux was handed {args:?} (raw: {stdout:?})"
        );
    }

    // === RemoteStream ======================================================
    //
    // A test cannot ssh anywhere, so none of these do. `from_command` is the
    // seam: `spawn` only builds the ssh `Command` and hands it over, and
    // nothing past that point knows whether the child is ssh. Everything below
    // therefore runs the real reader and the real teardown against a locally
    // spawned child — including, in the orphan test, the real `frame_command()`
    // against this machine's own tmux.
    //
    // **Every local child gets its own process group, and the group is killed
    // at the end.** Killing the child alone is not enough: FRAME_SCRIPT's
    // forked subshells inherit the write end of the pipe and outlive their
    // parent, which is how RG-2 left two `capture-pane` loops running here.

    use std::os::unix::process::CommandExt;
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    /// A local stand-in for the ssh child, in a process group of its own so a
    /// test can reach the whole tree and not just the shell at the top of it.
    pub(crate) fn local_child(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script]);
        c.process_group(0);
        c
    }

    /// A child that writes `out` verbatim and then runs `then`.
    ///
    /// Where `then` sleeps it must `exec`: a *forked* `sleep` inherits the pipe,
    /// survives the kill aimed at its parent, and holds the reader's `read`
    /// open for as long as it runs.
    fn emitting_child(out: &str, then: &str) -> Command {
        let mut c = local_child(&format!("printf '%s' \"$YH_OUT\"; {then}"));
        c.env("YH_OUT", out);
        c
    }

    /// Kills every process in the group led by `pgid` when it goes out of
    /// scope, however the test ended.
    pub(crate) struct GroupGuard(pub(crate) i32);

    impl Drop for GroupGuard {
        fn drop(&mut self) {
            let _ = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("kill -9 -{} 2>/dev/null", self.0))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    /// `ps` state letter for a pid: `None` once it is gone *and reaped*, `Z`
    /// while it is a zombie waiting for a `wait()` that never came.
    pub(crate) fn process_state(pid: i32) -> Option<String> {
        let out = Command::new("ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .expect("ps should be runnable");
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// Processes *in this test's own group* still running the frame loop.
    ///
    /// Scoped to the group on purpose: `cargo test` runs these in parallel with
    /// each other and with RG-2's script tests, so a bare `pgrep -f YHFRAME`
    /// would keep finding another test's healthy child and never go quiet.
    fn loop_pids(pgid: i32) -> Vec<String> {
        let out = Command::new("pgrep")
            .args(["-g", &pgid.to_string(), "-f", "YHFRAME"])
            .output()
            .expect("pgrep should be runnable");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    fn poll_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if done() {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// One whole frame with its trailing sentinel, exactly as a barn writes it.
    fn wire_frame(index: &str, pane: &str, screen: &str, now: u64) -> String {
        format!(
            "{}{FRAME}\n",
            frame_body(&[win(index, "api", "0", pane)], &[screen], now, &[])
        )
    }

    fn next_event(rx: &Receiver<RemoteEvent>, within: Duration) -> Option<RemoteEvent> {
        rx.recv_timeout(within).ok()
    }

    fn expect_frame(rx: &Receiver<RemoteEvent>, within: Duration) -> RemoteFrame {
        match next_event(rx, within) {
            Some(RemoteEvent::Frame(f)) => f,
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    fn expect_failed(rx: &Receiver<RemoteEvent>, within: Duration) -> String {
        match next_event(rx, within) {
            Some(RemoteEvent::Failed { error, .. }) => error,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    fn stream_barn() -> Barn {
        Barn {
            name: "guided".into(),
            host: Some("172.233.141.59".into()),
            user: Some("forge".into()),
            port: Some(2222),
            identity_file: None,
            critters: vec![],
            source: None,
            connection_type: None,
            connection_config: None,
            connectable: None,
        }
    }

    #[test]
    fn the_stream_asks_for_no_remote_tty_and_batches_the_frame_command() {
        // No `-t`: a remote pty gives the loop a controlling terminal, and the
        // SIGPIPE on its next write is the only thing that ends it when the
        // connection goes. BatchMode because a stream must fail rather than sit
        // at a passphrase prompt no one can see behind the TUI.
        let cmd = stream_command(&stream_barn()).expect("a configured barn");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert_eq!(cmd.get_program(), "ssh");
        assert!(
            !args.iter().any(|a| a == "-t"),
            "the stream asked for a remote pty: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "BatchMode=yes"),
            "the stream can block on a prompt: {args:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some(frame_command().as_str()),
            "the remote command must be the frame loop, as one argv element"
        );
    }

    #[test]
    fn the_child_gets_no_stdin_so_a_stream_cannot_eat_the_tuis_keystrokes() {
        // ssh forwards its own stdin to the remote command. A stream child that
        // inherits ours sits there reading the terminal, racing crossterm for
        // every key the user presses at the grid — and the grid is exactly
        // where the number keys live.
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("what-stdin-was");
        let script = format!(
            "if [ /dev/stdin -ef /dev/null ]; then echo NULL; else echo OTHER; fi > {}",
            marker.display()
        );

        // Vacuity check: with stdin inherited the same script has to say OTHER,
        // or this harness already has /dev/null on fd 0 and the test below
        // would pass no matter what the stream does.
        let ctl = Command::new("/bin/sh")
            .args(["-c", &script])
            .status()
            .expect("sh should be runnable");
        assert!(ctl.success());
        if std::fs::read_to_string(&marker).unwrap().trim() == "NULL" {
            eprintln!("skipping: this harness already runs with /dev/null on stdin");
            return;
        }

        let (tx, rx) = mpsc::channel();
        let stream = RemoteStream::from_command("guided", local_child(&script), tx, STALL_TIMEOUT)
            .expect("a local shell spawns");
        let _group = GroupGuard(stream.child.id() as i32);

        // The child exits as soon as it has written the marker; the stream
        // noticing is our signal that it is done.
        expect_failed(&rx, Duration::from_secs(10));
        drop(stream);

        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().trim(),
            "NULL",
            "the stream child inherited our stdin"
        );
    }

    #[test]
    fn dropping_a_stream_kills_the_child_and_leaves_no_zombie() {
        // kill() alone leaves a <defunct> ssh per barn per grid open, for the
        // whole life of the TUI. Reaping is the other half.
        let (tx, _rx) = mpsc::channel();
        let stream =
            RemoteStream::from_command("guided", local_child("exec sleep 300"), tx, STALL_TIMEOUT)
                .expect("a local shell spawns");
        let pid = stream.child.id() as i32;
        let _group = GroupGuard(pid);

        assert!(
            process_state(pid).is_some(),
            "control: the child should be running before the drop"
        );

        drop(stream);

        let state = process_state(pid);
        assert!(
            state.is_none(),
            "the child survived the drop as {state:?} — 'Z' means it was killed but never reaped"
        );
    }

    #[test]
    fn killing_the_local_child_terminates_the_remote_loop() {
        // THE orphan test. A loop left behind runs `capture-pane` once a second
        // forever on someone's production box with nothing pointing at it. This
        // runs the real `frame_command()` against this machine's own tmux, then
        // asserts the whole group goes quiet after teardown — not just the
        // process we killed, because the subshells it forked hold the same pipe.
        let (tx, _rx) = mpsc::channel();
        let stream = RemoteStream::from_command(
            "this-machine",
            local_child(&format!("exec {}", frame_command())),
            tx,
            STALL_TIMEOUT,
        )
        .expect("a local shell spawns");
        let pgid = stream.child.id() as i32;
        let _group = GroupGuard(pgid);

        assert!(
            poll_until(Duration::from_secs(15), || !loop_pids(pgid).is_empty()),
            "the frame loop never started, so there was nothing to orphan"
        );

        drop(stream);

        assert!(
            poll_until(Duration::from_secs(15), || loop_pids(pgid).is_empty()),
            "the loop outlived its stream: {:?}",
            loop_pids(pgid)
        );
    }

    #[test]
    fn a_stream_whose_child_dies_reports_failed_rather_than_going_silent() {
        // A barn that disappears must say so. Silence is indistinguishable from
        // a healthy barn with nothing to report, and the grid would keep
        // painting a frame that stopped being true minutes ago.
        let (tx, rx) = mpsc::channel();
        let out = wire_frame("1", "%7", "only frame", 1_700_000_000);
        let stream = RemoteStream::from_command(
            "guided",
            emitting_child(&out, "exit 0"),
            tx,
            STALL_TIMEOUT,
        )
        .expect("a local shell spawns");
        let _group = GroupGuard(stream.child.id() as i32);

        let f = expect_frame(&rx, Duration::from_secs(10));
        assert_eq!(f.barn, "guided", "every event has to name its barn");

        // Well inside STALL_TIMEOUT, so this is the EOF path and not the stall
        // deadline arriving late and looking like a pass.
        let err = expect_failed(&rx, Duration::from_secs(3));
        assert!(!err.trim().is_empty(), "a failure with nothing to say");
    }

    #[test]
    fn partial_output_at_eof_is_discarded_not_emitted_as_a_frame() {
        // The tail after the last sentinel is a frame still being written. It
        // is crafted here to be otherwise perfectly parseable, so anything that
        // hands the leftovers to `parse_frame` on the way out produces a second,
        // fictional frame instead of failing.
        let (tx, rx) = mpsc::channel();
        let whole = wire_frame("1", "%7", "real", 1_700_000_000);
        let partial = frame_body(
            &[win("2", "web", "0", "%9")],
            &["never sent"],
            1_700_000_001,
            &[],
        );
        assert!(
            parse_frame("guided", &partial).is_some(),
            "the leftover has to be parseable, or this test proves nothing"
        );

        let stream = RemoteStream::from_command(
            "guided",
            emitting_child(&format!("{whole}{partial}"), "exit 0"),
            tx,
            STALL_TIMEOUT,
        )
        .expect("a local shell spawns");
        let _group = GroupGuard(stream.child.id() as i32);

        let f = expect_frame(&rx, Duration::from_secs(10));
        assert_eq!(f.barn_now, 1_700_000_000);

        match next_event(&rx, Duration::from_secs(3)) {
            Some(RemoteEvent::Failed { .. }) => {}
            other => panic!("the half-written frame was emitted: {other:?}"),
        }
    }

    #[test]
    fn a_barn_that_goes_quiet_reports_failed_rather_than_hanging_the_reader() {
        // The state this feature exists to survive: the remote tmux wedges, the
        // ssh channel stays wide open, and the loop simply stops completing
        // frames. An accumulate-until-sentinel reader with no deadline blocks
        // on that forever — RG-2 hung its own suite for three minutes proving
        // it. Failed has to be reachable from a stall, not only from EOF.
        let (tx, rx) = mpsc::channel();
        let out = format!(
            "{}{}",
            wire_frame("1", "%7", "first", 1_700_000_000),
            wire_frame("2", "%9", "second", 1_700_000_001),
        );
        let stream = RemoteStream::from_command(
            "guided",
            emitting_child(&out, "exec sleep 300"),
            tx,
            // Short enough to keep the suite quick, long enough that a loaded
            // machine taking a moment to schedule the reader is not a stall.
            Duration::from_secs(2),
        )
        .expect("a local shell spawns");
        let _group = GroupGuard(stream.child.id() as i32);

        // Both frames arrive while the child is still very much alive, which is
        // also the proof that frames stream out as they complete rather than
        // being handed over in a lump when the child finally exits.
        assert_eq!(expect_frame(&rx, Duration::from_secs(10)).barn_now, 1_700_000_000);
        assert_eq!(expect_frame(&rx, Duration::from_secs(10)).barn_now, 1_700_000_001);
        let quiet_since = Instant::now();

        let err = expect_failed(&rx, Duration::from_secs(10));
        assert!(err.contains("guided"), "a failure has to name its barn: {err:?}");
        // The lower bound is the point. Anything that failed instantly failed
        // for some other reason — the child is alive and holding the pipe, so
        // there is no EOF here to find.
        let waited = quiet_since.elapsed();
        assert!(
            waited >= Duration::from_millis(1_500),
            "reported a stall after only {waited:?}, so the deadline is not what fired"
        );
    }

    #[test]
    fn shutting_a_stream_down_does_not_report_the_barn_as_failed() {
        // Leaving the grid drops every stream. If a deliberate shutdown looked
        // like a dead barn, every barn would be marked STALE on the way out and
        // still be marked STALE the next time the grid opened.
        let (tx, rx) = mpsc::channel();
        let out = wire_frame("1", "%7", "alive", 1_700_000_000);
        let stream = RemoteStream::from_command(
            "guided",
            emitting_child(&out, "exec sleep 300"),
            tx,
            STALL_TIMEOUT,
        )
        .expect("a local shell spawns");
        let _group = GroupGuard(stream.child.id() as i32);

        expect_frame(&rx, Duration::from_secs(10));
        drop(stream);

        // The child held the pipe open until the kill, so EOF genuinely happens
        // here — this is the path that would report Failed if nothing told it
        // the shutdown was ours.
        while let Some(ev) = next_event(&rx, Duration::from_secs(2)) {
            if let RemoteEvent::Failed { barn, error } = ev {
                panic!("a deliberate shutdown reported '{barn}' as failed: {error}");
            }
        }
    }

    #[test]
    fn a_frame_split_across_reads_arrives_whole_and_uncorrupted() {
        // Read boundaries land wherever the kernel put them. Decoding each read
        // on its own turns any multi-byte glyph straddling one into replacement
        // characters, and panes are full of box drawing — Claude's own TUI draws
        // with it. The accumulator has to stay bytes until a frame is complete.
        let dir = tempfile::tempdir().expect("tempdir");
        let screen = "├─ box drawing ─┤";
        let payload = wire_frame("1", "%7", screen, 1_700_000_000);
        let path = dir.path().join("frame.bin");
        std::fs::write(&path, &payload).expect("write payload");

        let cut = payload.find('├').expect("the glyph is in the payload") + 1;
        assert!(
            !payload.is_char_boundary(cut),
            "the cut has to land inside a glyph or this test proves nothing"
        );

        let (tx, rx) = mpsc::channel();
        let script = format!(
            "head -c {cut} {p}; sleep 1; tail -c +{next} {p}; exec sleep 300",
            p = path.display(),
            next = cut + 1
        );
        let stream = RemoteStream::from_command("guided", local_child(&script), tx, STALL_TIMEOUT)
            .expect("a local shell spawns");
        let _group = GroupGuard(stream.child.id() as i32);

        let f = expect_frame(&rx, Duration::from_secs(10));
        assert_eq!(
            f.captures.get("%7").map(Vec::as_slice),
            Some(&[screen.to_string()][..]),
            "the glyph was cut in half by a read boundary"
        );
    }

    // === RemoteStreams =====================================================
    //
    // `reconcile` decides *which* barns get a stream; it does not care what a
    // stream is. So these drive it through the same seam RG-3 opened one level
    // down — `spawn` builds the ssh `Command` and `from_command` takes it from
    // there — with a spawner that starts a local child. Every decision the
    // registry makes is exercised against real streams, real reader threads and
    // real teardown, with no barn, no ssh and no network anywhere in the suite.
    //
    // Local children keep the same process-group discipline as everything above:
    // `local_child` puts each in a group of its own and every test guards the
    // group, because a child's forked subshells outlive a kill aimed at the
    // child alone.

    use std::cell::RefCell;
    use std::collections::HashSet;

    /// A barn that is configured enough for `ssh::command` to build an argv,
    /// pointed at TEST-NET-1 so a stray real spawn cannot reach anything.
    pub(crate) fn named_barn(name: &str) -> Barn {
        Barn { name: name.to_string(), host: Some("192.0.2.1".into()), ..stream_barn() }
    }

    /// What `App.connected_barns` holds: tmux **session** names, not barn names.
    pub(crate) fn sessions_for(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| tmux::barn_session_name(n)).collect()
    }

    /// A stand-in for [`RemoteStream::spawn`] that runs `cmd(barn)` locally and
    /// records every barn it was asked for, in order.
    ///
    /// The log is the point: "did not respawn" and "never spawned at all" are
    /// indistinguishable from the outside, and the difference between them is
    /// four ssh channels a second.
    pub(crate) fn recording_spawner<'a, C>(
        log: &'a RefCell<Vec<String>>,
        cmd: C,
    ) -> impl Fn(&Barn, Sender<RemoteEvent>) -> Result<RemoteStream> + 'a
    where
        C: Fn(&Barn) -> Command + 'a,
    {
        move |barn: &Barn, tx| {
            log.borrow_mut().push(barn.name.clone());
            RemoteStream::from_command(&barn.name, cmd(barn), tx, STALL_TIMEOUT)
        }
    }

    /// A child that connects, says nothing, and holds the pipe open — a barn
    /// mid-login, which is what most reconcile ticks find.
    pub(crate) fn silent(_: &Barn) -> Command {
        local_child("exec sleep 300")
    }

    /// Guard every child the registry currently holds, so a panic anywhere below
    /// still takes the process groups with it.
    pub(crate) fn guard_all(reg: &RemoteStreams) -> Vec<GroupGuard> {
        reg.streams.values().map(|s| GroupGuard(s.child.id() as i32)).collect()
    }

    pub(crate) fn child_pid(reg: &RemoteStreams, barn: &str) -> Option<i32> {
        reg.streams.get(barn).map(|s| s.child.id() as i32)
    }

    /// Barns the registry has given up on, sorted. The only evidence a
    /// reconcile ran at all when every barn it was handed was unreachable.
    pub(crate) fn failed_barns(reg: &RemoteStreams) -> Vec<String> {
        let mut names: Vec<String> = reg.failed.keys().cloned().collect();
        names.sort();
        names
    }

    /// Put a barn in the state a dead stream leaves it in, without the stream.
    ///
    /// Through `record_failure`, the same call `drain` makes, so a test that
    /// only cares *that* a barn is stale — `app.rs`'s jump, most of all — gets
    /// the real bookkeeping, backoff included, rather than a hand-built map
    /// entry that could drift from it.
    pub(crate) fn mark_failed(reg: &mut RemoteStreams, barn: &str, error: &str) {
        reg.record_failure(barn.to_string(), error.to_string(), Instant::now());
    }

    /// Tick the registry the way the app does — `drain` on a timer — until
    /// `done`, or give up.
    fn drain_until(
        reg: &mut RemoteStreams,
        limit: Duration,
        mut done: impl FnMut(&RemoteStreams) -> bool,
    ) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            reg.drain();
            if done(reg) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn reconcile_spawns_for_newly_connected_barns_only() {
        let barns = [named_barn("guided"), named_barn("smash-mac")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        reg.reconcile_with(&barns, &connected, recording_spawner(&log, silent));
        let _guards = guard_all(&reg);

        assert_eq!(*log.borrow(), ["guided"], "a barn nobody connected to got a stream");
        assert!(reg.streams.contains_key("guided"));
        assert!(
            !reg.streams.contains_key("smash-mac"),
            "streams exist only for connected barns: {:?}",
            reg.streams.keys().collect::<Vec<_>>()
        );

        reg.shutdown();
    }

    #[test]
    fn reconcile_drops_the_stream_for_a_disconnected_barn() {
        let barns = [named_barn("guided"), named_barn("smash-mac")];
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        reg.reconcile_with(&barns, &sessions_for(&["guided", "smash-mac"]), recording_spawner(&log, silent));
        let _guards = guard_all(&reg);
        let guided = child_pid(&reg, "guided").expect("guided is streaming");
        let gone = child_pid(&reg, "smash-mac").expect("smash-mac is streaming");

        reg.reconcile_with(&barns, &sessions_for(&["guided"]), recording_spawner(&log, silent));

        assert!(
            !reg.streams.contains_key("smash-mac"),
            "the disconnected barn kept its stream: {:?}",
            reg.streams.keys().collect::<Vec<_>>()
        );
        // Dropping the entry is the whole teardown, so the child has to be gone
        // *and reaped* — a `Z` here is an ssh zombie per disconnect for the rest
        // of the TUI's life.
        assert_eq!(
            process_state(gone),
            None,
            "the disconnected barn's child outlived its stream"
        );
        assert_eq!(
            child_pid(&reg, "guided"),
            Some(guided),
            "the connected barn's stream was disturbed by its neighbour leaving"
        );

        reg.shutdown();
    }

    #[test]
    fn reconcile_matches_barns_by_session_name_not_by_barn_name() {
        // `connected` comes from `tmux list-sessions`, so barn "camera pi" is in
        // it as "yh-barn-camera-pi-<hash>". Matching on the raw name finds
        // nothing, and finding nothing looks exactly like "no barns connected" —
        // an empty grid, forever, with no error anywhere to explain it.
        let barns = [named_barn("camera pi")];
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let raw: HashSet<String> = ["camera pi".to_string()].into_iter().collect();
        reg.reconcile_with(&barns, &raw, recording_spawner(&log, silent));
        assert!(log.borrow().is_empty(), "matched the raw barn name, which tmux never reports");

        // The `=` form is for `-t` arguments. It is never what list-sessions
        // prints, so matching on it would be the same silent nothing.
        let target: HashSet<String> =
            [tmux::barn_session_target("camera pi")].into_iter().collect();
        reg.reconcile_with(&barns, &target, recording_spawner(&log, silent));
        assert!(log.borrow().is_empty(), "matched the '='-prefixed target form");

        let real = sessions_for(&["camera pi"]);
        assert!(
            real.iter().all(|s| s.starts_with("yh-barn-camera-pi-")),
            "control: the session name is the slug plus a hash, not the barn name"
        );
        reg.reconcile_with(&barns, &real, recording_spawner(&log, silent));
        let _guards = guard_all(&reg);

        assert_eq!(*log.borrow(), ["camera pi"], "the real session name did not match");
        assert!(reg.streams.contains_key("camera pi"), "streams are keyed by barn name");

        reg.shutdown();
    }

    #[test]
    fn reconcile_is_idempotent_and_does_not_respawn_a_live_stream() {
        // It runs on every 250ms tick, not just on grid open. Respawning a live
        // stream opens four ssh channels a second per barn.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        reg.reconcile_with(&barns, &connected, recording_spawner(&log, silent));
        let _guards = guard_all(&reg);
        let first = child_pid(&reg, "guided").expect("guided is streaming");

        for tick in 0..8 {
            reg.reconcile_with(&barns, &connected, recording_spawner(&log, silent));
            assert_eq!(
                child_pid(&reg, "guided"),
                Some(first),
                "tick {tick} replaced a perfectly good stream"
            );
        }

        assert_eq!(
            *log.borrow(),
            ["guided"],
            "nine reconciles spawned {} streams",
            log.borrow().len()
        );
        assert_eq!(reg.streams.len(), 1);

        reg.shutdown();
    }

    #[test]
    fn drain_never_blocks_when_no_frames_are_pending() {
        // It is called from the idle tick, on the thread that paints. A drain
        // that waited for a frame would freeze the whole TUI between them —
        // and most ticks have nothing pending, because frames are a second
        // apart and ticks are 250ms.
        let mut reg = RemoteStreams::new();

        let empty = Instant::now();
        reg.drain();
        assert!(
            empty.elapsed() < Duration::from_millis(100),
            "drain blocked for {:?} with no streams at all",
            empty.elapsed()
        );

        // The state that actually recurs: a live stream, mid-login, with nothing
        // to say yet. `rx` has a sender in it, so a blocking read has no EOF to
        // rescue it.
        let barns = [named_barn("guided")];
        let log = RefCell::new(Vec::new());
        reg.reconcile_with(&barns, &sessions_for(&["guided"]), recording_spawner(&log, silent));
        let _guards = guard_all(&reg);

        for _ in 0..4 {
            let quiet = Instant::now();
            reg.drain();
            assert!(
                quiet.elapsed() < Duration::from_millis(100),
                "drain blocked for {:?} on a stream with nothing to send",
                quiet.elapsed()
            );
        }
        assert!(reg.frames().get("guided").is_none(), "a silent stream produced a frame");

        reg.shutdown();
    }

    #[test]
    fn a_failed_barn_keeps_its_last_frame_for_stale_rendering() {
        // Dropping a dead barn's cells would renumber every cell after them
        // under the user's fingers. The last frame stays so Task 9 can dim it
        // and badge it STALE in place.
        let barns = [named_barn("guided")];
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "last words", 1_700_000_000);
        reg.reconcile_with(
            &barns,
            &sessions_for(&["guided"]),
            recording_spawner(&log, |_: &Barn| emitting_child(&out, "exit 0")),
        );
        let _guards = guard_all(&reg);

        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "a barn whose stream died never reported it"
        );

        let frame = reg.frames().get("guided").expect("the last good frame is kept for STALE cells");
        assert_eq!(frame.barn_now, 1_700_000_000);
        assert_eq!(
            frame.captures.get("%7").map(Vec::as_slice),
            Some(&["last words".to_string()][..])
        );
        assert!(
            !reg.failed["guided"].error.trim().is_empty(),
            "a failure with nothing to say"
        );
        assert!(
            !reg.streams.contains_key("guided"),
            "the dead stream was kept, so its child is a zombie until the grid closes"
        );

        reg.shutdown();
    }

    #[test]
    fn a_failed_barn_is_not_respawned_on_every_tick() {
        // Reconcile runs four times a second and a genuinely gone barn stays
        // gone. Retrying on every tick is four ssh handshakes a second against
        // a host that is not answering — each one blocking for ConnectTimeout.
        // (RG-9 turns this into a retry with backoff; it must not become a
        // retry without one.)
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "last words", 1_700_000_000);
        let dies = |_: &Barn| emitting_child(&out, "exit 0");
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        let _guards = guard_all(&reg);

        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "the stream never failed, so there is nothing to not-retry"
        );

        for _ in 0..8 {
            reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        }
        assert_eq!(
            *log.borrow(),
            ["guided"],
            "a dead barn was retried {} times in eight ticks",
            log.borrow().len() - 1
        );

        reg.shutdown();
    }

    #[test]
    fn a_disconnected_barn_takes_its_last_frame_with_it() {
        // `C-d` on a connected barn has to remove its cells from the grid. A
        // frame left behind would keep painting a barn the user just closed.
        let barns = [named_barn("guided")];
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "still here", 1_700_000_000);
        reg.reconcile_with(
            &barns,
            &sessions_for(&["guided"]),
            recording_spawner(&log, |_: &Barn| emitting_child(&out, "exec sleep 300")),
        );
        let _guards = guard_all(&reg);

        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.frames().get("guided").is_some()),
            "no frame ever arrived, so there is nothing to drop"
        );

        reg.reconcile_with(&barns, &HashSet::new(), recording_spawner(&log, silent));
        assert!(
            reg.frames().get("guided").is_none(),
            "a disconnected barn kept its cells on the grid"
        );

        reg.shutdown();
    }

    #[test]
    fn frames_from_two_barns_land_in_their_own_slots() {
        // Pane ids collide across hosts — `%1` exists on every machine — so
        // frames are partitioned per barn and never merged into one map.
        let barns = [named_barn("guided"), named_barn("smash-mac")];
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        reg.reconcile_with(
            &barns,
            &sessions_for(&["guided", "smash-mac"]),
            recording_spawner(&log, |b: &Barn| {
                let out = wire_frame("1", "%1", &format!("{} output", b.name), 1_700_000_000);
                emitting_child(&out, "exec sleep 300")
            }),
        );
        let _guards = guard_all(&reg);

        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| {
                r.frames().get("guided").is_some() && r.frames().get("smash-mac").is_some()
            }),
            "both barns must land, got {:?}",
            reg.frames.keys().collect::<Vec<_>>()
        );

        for barn in ["guided", "smash-mac"] {
            let f = reg.frames().get(barn).expect("a frame per barn");
            assert_eq!(f.barn, barn);
            assert_eq!(
                f.captures.get("%1").map(Vec::as_slice),
                Some(&[format!("{barn} output")][..]),
                "%1 on one barn overwrote %1 on the other"
            );
        }

        reg.shutdown();
    }

    #[test]
    fn shutdown_clears_every_stream() {
        let barns = [named_barn("guided"), named_barn("smash-mac")];
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        reg.reconcile_with(&barns, &sessions_for(&["guided", "smash-mac"]), recording_spawner(&log, silent));
        let _guards = guard_all(&reg);
        let pids: Vec<i32> = reg.streams.values().map(|s| s.child.id() as i32).collect();
        assert_eq!(pids.len(), 2, "control: two streams to shut down");
        reg.frames.insert("guided".into(), parse_frame("guided", &wire_frame("1", "%7", "x", 1)[..]).unwrap_or_else(|| panic!("fixture")));
        mark_failed(&mut reg, "smash-mac", "went away");

        reg.shutdown();

        assert!(reg.streams.is_empty(), "streams outlived the grid");
        assert!(reg.frames.is_empty(), "a closed grid is still holding frames");
        // The failure deliberately outlives the grid, unlike the frame it came
        // with. RG-4 asserted the opposite here and was right at the time: a
        // failure with nothing to retry is just stale news. It is now the
        // backoff's whole memory, and `reconcile` runs on grid *open* as well as
        // on the tick — so clearing it would hand every reopen a free handshake
        // against a barn that is not answering, which is the one thing the
        // backoff exists to stop. See `the_backoff_survives_a_grid_reopen`.
        assert_eq!(failed_barns(&reg), ["smash-mac"], "the backoff forgot a dead barn");
        for pid in pids {
            // Killed *and* reaped: `Z` here is a defunct ssh per barn per grid
            // open, for the rest of the TUI's life.
            assert_eq!(process_state(pid), None, "pid {pid} survived the shutdown");
        }
    }

    // === the backoff =======================================================
    //
    // `reconcile_at` rather than `reconcile_with`: the clock is the subject, and
    // driving it by hand is what makes "one tick before it is due" and "one tick
    // after" two different assertions rather than two sleeps.

    #[test]
    fn retry_delay_doubles_and_then_stops_at_the_cap() {
        assert_eq!(retry_delay(1), RETRY_BASE, "the first retry is the base wait");
        assert_eq!(retry_delay(2), RETRY_BASE * 2);
        assert_eq!(retry_delay(3), RETRY_BASE * 4);
        assert!(
            retry_delay(5) > retry_delay(4),
            "the wait stopped growing before it reached the cap"
        );
        assert_eq!(retry_delay(60), RETRY_MAX, "the wait grew past the cap");
        // The exponent is shifted, so an unclamped `1 << attempts` is UB-adjacent
        // long before this and a `Duration` multiply overflows shortly after.
        assert_eq!(retry_delay(u32::MAX), RETRY_MAX, "the cap is not total");
    }

    #[test]
    fn a_failed_stream_is_retried_with_backoff_not_every_tick() {
        // Once per 250ms tick is four ssh handshakes a second against a dead
        // host, each blocking a child for ConnectTimeout. Never retrying at all
        // is a barn that never comes back from a ten-second blip.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "last words", 1_700_000_000);
        let dies = |_: &Barn| emitting_child(&out, "exit 0");
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        let mut guards = guard_all(&reg);

        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "the stream never failed, so there is nothing to retry"
        );
        // Taken *after* the failure landed, so it is later than the instant the
        // backoff was scheduled from and every offset below is a lower bound.
        let seen = Instant::now();

        // Four ticks a second for as long as the wait lasts, and not one of them
        // may open a channel.
        for tick in 0..(RETRY_BASE.as_millis() / 250) as u32 {
            reg.reconcile_at(
                seen + Duration::from_millis(250) * tick,
                &barns,
                &connected,
                recording_spawner(&log, dies),
            );
        }
        assert_eq!(
            *log.borrow(),
            ["guided"],
            "a dead barn was retried {} times inside the backoff",
            log.borrow().len() - 1
        );

        // And past it, it is retried — a barn that dropped for a moment has to
        // come back on its own.
        reg.reconcile_at(
            seen + RETRY_BASE + Duration::from_secs(1),
            &barns,
            &connected,
            recording_spawner(&log, dies),
        );
        guards.extend(guard_all(&reg));
        assert_eq!(
            *log.borrow(),
            ["guided", "guided"],
            "the backoff expired and nothing retried the barn"
        );

        reg.shutdown();
    }

    #[test]
    fn each_failure_waits_longer_than_the_one_before() {
        // A barn that has failed twice is likelier to be gone than one that has
        // failed once. Retrying both at the same interval spends the same
        // handshakes on a host that has never answered as on one that blipped.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        // Dies with nothing to say, both times. A child that got a frame out
        // first would have *recovered*, however briefly, and its counter would
        // rightly start over — see
        // `a_recovered_barn_starts_its_next_backoff_from_scratch`.
        let dies = |_: &Barn| local_child("exit 0");
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        let mut guards = guard_all(&reg);
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "the first stream never failed"
        );

        let first = Instant::now();
        reg.reconcile_at(first + RETRY_BASE + Duration::from_secs(1), &barns, &connected, recording_spawner(&log, dies));
        guards.extend(guard_all(&reg));
        assert_eq!(log.borrow().len(), 2, "the first retry never happened");

        // The retry dies too.
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| !r.streams.contains_key("guided")),
            "the retried stream never failed"
        );
        let second = Instant::now();

        // One base wait is no longer enough.
        reg.reconcile_at(second + RETRY_BASE + Duration::from_millis(500), &barns, &connected, recording_spawner(&log, dies));
        assert_eq!(
            log.borrow().len(),
            2,
            "the second failure was retried on the same schedule as the first"
        );

        reg.reconcile_at(second + retry_delay(2) + Duration::from_secs(1), &barns, &connected, recording_spawner(&log, dies));
        guards.extend(guard_all(&reg));
        assert_eq!(log.borrow().len(), 3, "the longer wait expired and nothing retried");

        reg.shutdown();
    }

    #[test]
    fn the_backoff_survives_a_grid_reopen() {
        // `shutdown` runs every time the user leaves the grid, and `reconcile`
        // runs on every grid *open* as well as on the tick. A backoff that lived
        // only as long as the grid would hand a barn parked on its own
        // "unreachable" screen one fresh handshake per `v`.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "last words", 1_700_000_000);
        let dies = |_: &Barn| emitting_child(&out, "exit 0");
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        let mut guards = guard_all(&reg);
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "the stream never failed"
        );
        let seen = Instant::now();

        // Leave the grid and come straight back, twice.
        for reopen in 0..2 {
            reg.shutdown();
            reg.reconcile_at(seen, &barns, &connected, recording_spawner(&log, dies));
            assert_eq!(
                *log.borrow(),
                ["guided"],
                "reopen {reopen} retried a barn still inside its backoff"
            );
        }

        // The wait still expires — a reopen must not be able to postpone it
        // either, or a barn that recovered stays dark for as long as the user
        // keeps looking at it.
        reg.reconcile_at(seen + RETRY_BASE + Duration::from_secs(1), &barns, &connected, recording_spawner(&log, dies));
        guards.extend(guard_all(&reg));
        assert_eq!(log.borrow().len(), 2, "the backoff outlasted its own deadline");

        reg.shutdown();
    }

    #[test]
    fn a_recovered_barn_clears_its_stale_marking() {
        // The event RG-4 had no way to produce: a retry that works. Until the
        // frame lands the cells stay STALE — a spawned child is not a barn that
        // answered — and the moment it does, the marking has to go or the grid
        // dims a barn that is streaming happily.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "last words", 1_700_000_000);
        let dies = |_: &Barn| emitting_child(&out, "exit 0");
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        let mut guards = guard_all(&reg);
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "the stream never failed, so there is nothing to recover from"
        );
        let seen = Instant::now();

        let back = wire_frame("2", "%9", "back on its feet", 1_700_000_001);
        let alive = |_: &Barn| emitting_child(&back, "exec sleep 300");
        reg.reconcile_at(
            seen + RETRY_BASE + Duration::from_secs(1),
            &barns,
            &connected,
            recording_spawner(&log, alive),
        );
        guards.extend(guard_all(&reg));
        assert_eq!(log.borrow().len(), 2, "control: the retry has to happen at all");
        assert!(
            reg.failed.contains_key("guided"),
            "a barn stopped being stale on a spawn rather than on a frame"
        );

        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| {
                r.frames().get("guided").is_some_and(|f| f.barn_now == 1_700_000_001)
            }),
            "the recovered stream never delivered a frame"
        );
        assert!(
            reg.failed.is_empty(),
            "a barn that came back is still marked stale: {:?}",
            failed_barns(&reg)
        );
        assert!(reg.stale().is_empty(), "the grid would still dim a live barn");

        reg.shutdown();
    }

    #[test]
    fn a_recovered_barn_starts_its_next_backoff_from_scratch() {
        // `attempts` counts *consecutive* failures. Left uncleared, a barn that
        // drops once an hour and recovers every time would eventually wait a
        // full minute to notice a blip.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "last words", 1_700_000_000);
        let dies = |_: &Barn| emitting_child(&out, "exit 0");
        let back = wire_frame("2", "%9", "back on its feet", 1_700_000_001);
        let alive = |_: &Barn| emitting_child(&back, "exec sleep 300");

        reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        let mut guards = guard_all(&reg);
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "the first stream never failed"
        );

        // Recover.
        reg.reconcile_at(Instant::now() + RETRY_BASE + Duration::from_secs(1), &barns, &connected, recording_spawner(&log, alive));
        guards.extend(guard_all(&reg));
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.is_empty()),
            "the barn never recovered"
        );

        // Then die again. The wait that follows is the *first* wait, not the
        // second.
        reg.streams.remove("guided");
        reg.record_failure("guided".into(), "and again".into(), Instant::now());
        let seen = Instant::now();
        reg.reconcile_at(seen + RETRY_BASE + Duration::from_millis(500), &barns, &connected, recording_spawner(&log, dies));
        guards.extend(guard_all(&reg));
        assert_eq!(
            log.borrow().len(),
            3,
            "a barn that had recovered was made to wait out the second-failure backoff"
        );

        reg.shutdown();
    }

    #[test]
    fn disconnecting_a_barn_forgets_its_backoff() {
        // `C-d` then reconnect is a deliberate act, and the user watching for
        // their sessions to come back should not be made to sit out a wait that
        // belonged to the last connection.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "last words", 1_700_000_000);
        let dies = |_: &Barn| emitting_child(&out, "exit 0");
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, dies));
        let mut guards = guard_all(&reg);
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.failed.contains_key("guided")),
            "the stream never failed"
        );
        let seen = Instant::now();

        // Disconnect: the barn leaves `connected` entirely.
        reg.reconcile_at(seen, &barns, &HashSet::new(), recording_spawner(&log, dies));
        assert!(reg.stale().is_empty(), "a disconnected barn is still marked stale");

        // Reconnect inside what would have been the backoff window.
        reg.reconcile_at(seen, &barns, &connected, recording_spawner(&log, dies));
        guards.extend(guard_all(&reg));
        assert_eq!(
            log.borrow().len(),
            2,
            "reconnecting a barn by hand still waited out the old backoff"
        );

        reg.shutdown();
    }

    #[test]
    fn a_deliberate_teardown_never_marks_a_barn_failed() {
        // Every shutdown closes a pipe and every closed pipe looks like a barn
        // that went away. If that landed in `failed`, disconnecting a barn — or
        // simply leaving the grid — would badge it STALE on the way out and
        // again the next time it came back.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        let out = wire_frame("1", "%7", "alive", 1_700_000_000);
        let alive = |_: &Barn| emitting_child(&out, "exec sleep 300");

        reg.reconcile_with(&barns, &connected, recording_spawner(&log, alive));
        let mut guards = guard_all(&reg);
        assert!(
            drain_until(&mut reg, Duration::from_secs(15), |r| r.frames().get("guided").is_some()),
            "control: the stream has to be genuinely alive first"
        );

        // The barn disconnects and immediately reconnects — `C-d` then `C-b`,
        // or a flapping session list. The second reconcile puts the barn back in
        // `streams`, so a `Failed` from the stream we deliberately dropped is no
        // longer covered by "this barn is not streaming" and lands squarely on
        // the new one.
        reg.reconcile_with(&barns, &HashSet::new(), recording_spawner(&log, alive));
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, alive));
        guards.extend(guard_all(&reg));

        drain_until(&mut reg, Duration::from_millis(750), |_| false);
        assert!(
            reg.failed.is_empty(),
            "tearing a stream down reported its barn as dead: {:?}",
            reg.failed
        );

        // And the same for leaving the grid entirely.
        reg.shutdown();
        reg.reconcile_with(&barns, &connected, recording_spawner(&log, alive));
        guards.extend(guard_all(&reg));
        drain_until(&mut reg, Duration::from_millis(750), |_| false);
        assert!(
            reg.failed.is_empty(),
            "leaving the grid reported every barn as dead: {:?}",
            reg.failed
        );

        reg.shutdown();
    }

    #[test]
    fn a_failure_in_flight_across_a_shutdown_does_not_stick_to_the_next_stream() {
        // A stream that failed for real just before the user left the grid has
        // its `Failed` sitting in the channel with nobody reading. Reopening the
        // grid spawns a fresh stream for that barn, and the stale event would
        // land on it: one dead barn ago, rendered STALE now.
        let barns = [named_barn("guided")];
        let connected = sessions_for(&["guided"]);
        let log = RefCell::new(Vec::new());
        let mut reg = RemoteStreams::new();

        // The reader thread of a stream that is already being torn down.
        let orphan = reg.tx.clone();
        reg.shutdown();

        let sent = orphan.send(RemoteEvent::Failed {
            barn: "guided".into(),
            error: "the previous stream, one grid ago".into(),
        });
        assert!(sent.is_err(), "the shutdown left the old channel connected");

        reg.reconcile_with(&barns, &connected, recording_spawner(&log, silent));
        let _guards = guard_all(&reg);
        drain_until(&mut reg, Duration::from_millis(250), |_| false);

        assert!(
            reg.failed.is_empty(),
            "a failure from before the shutdown stuck to the new stream: {:?}",
            reg.failed
        );
        assert!(reg.streams.contains_key("guided"), "the new stream was torn down by it");

        reg.shutdown();
    }

    #[test]
    fn a_barn_that_cannot_be_reached_is_recorded_rather_than_spawning_a_child() {
        // The real `reconcile`, the real `RemoteStream::spawn`: a barn with no
        // host cannot even produce an ssh argv. The failure has to be recorded
        // like any other, or reconcile retries it four times a second forever.
        let barns = [Barn { host: None, ..named_barn("ghost") }];
        let connected = sessions_for(&["ghost"]);
        let mut reg = RemoteStreams::new();

        reg.reconcile(&barns, &connected);

        assert!(reg.streams.is_empty(), "a barn with no host spawned something");
        assert!(
            reg.failed.get("ghost").is_some_and(|f| f.error.contains("ghost")),
            "the failure was swallowed: {:?}",
            failed_barns(&reg)
        );

        reg.reconcile(&barns, &connected);
        assert!(reg.streams.is_empty(), "retried a barn that cannot be reached");

        reg.shutdown();
    }
}
