//! A child process we can talk *to*, not just listen to.
//!
//! The client spawns a peer — `ssh <ranchhouse> yeehaw ranch serve` — writes
//! requests to its stdin and reads responses from its stdout.
//!
//! `remote_grid`'s `RemoteStream::from_command` cannot be reused for this, and
//! not by oversight: it hardcodes `stdin(Stdio::null())` because ssh forwards
//! its stdin to the remote command, and inheriting the TUI's would race
//! crossterm for the user's keystrokes. That makes it a one-way pipe by
//! construction. Sync is request/response, so it needs its own transport, and
//! the one thing this must get right is that stdin is *piped*.
//!
//! Three things beyond that are load-bearing, each because the far side is ssh
//! and ssh is chatty: the peer's stderr is drained on its own thread from the
//! moment of spawn (an undrained pipe is a deadlock, not a lost message), the
//! peer's stdout may carry a login banner ahead of the protocol, and `finish`
//! rather than `Drop` is how a session is meant to end — only `finish` can say
//! whether the peer left cleanly or died with something worth quoting.

use super::wire::{self, Message};

/// How much of a peer's stderr is kept for error reporting.
///
/// The *tail*, not the head: ssh puts the noise first (banner, MOTD, "Warning:
/// Permanently added...") and the thing that explains a failure last
/// ("Permission denied (publickey)"). Draining never stops at this cap — the
/// whole point is that the peer is never blocked by us — only the keeping does.
const MAX_STDERR_CAPTURE: usize = 64 * 1024;

/// The most bytes one frame may occupy before the peer is declared broken.
///
/// NDJSON's framing is the newline, and `read_line` will grow its buffer until
/// it meets one — which is a promise the *peer* gets to keep. Entity payloads
/// ride inside a frame as YAML strings, so the cap has to clear the largest
/// real entity comfortably; 8 MiB is orders of magnitude past any of them and
/// still a wall rather than an appetite.
const MAX_LINE: u64 = 8 * 1024 * 1024;

/// How many undecodable lines the handshake will step over before declaring
/// the peer broken.
///
/// A bound rather than "skip until something parses": a peer wedged in a login
/// loop, or an ssh that landed on the wrong host and is streaming a help text,
/// would otherwise hang a sync with no timeout anywhere to break it. Thirty-two
/// lines is more than any MOTD this codebase has met and far less than a stream.
const MAX_PREAMBLE_LINES: usize = 32;

/// How long `finish` waits for the drain thread to see EOF before giving up on
/// a complete capture. Long enough for a dead child's pipe to close, short
/// enough that nobody notices.
const STDERR_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

pub struct Transport {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    /// Filled by the drain thread; read by the error paths. `Arc` because the
    /// thread outlives nothing in particular — it ends on its own at EOF.
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
    /// Kept only so `finish` can let a just-exited peer's last words land
    /// before it quotes them. Never joined unconditionally — see `settle_stderr`.
    drain: Option<std::thread::JoinHandle<()>>,
    /// True once the peer has said something this code could decode.
    handshake_done: bool,
}

/// How a peer's session ended: the child's status, plus whatever it said on
/// stderr. Both halves matter — a peer that exits 255 having printed
/// "Permission denied (publickey)" is a different failure from one that exits
/// 127 because `yeehaw` is not installed on the far side, and `recv()`'s
/// `Ok(None)` cannot tell either from a clean goodbye.
#[derive(Debug)]
pub struct PeerExit {
    pub status: std::process::ExitStatus,
    pub stderr: String,
}

impl Transport {
    pub fn spawn(mut cmd: std::process::Command) -> anyhow::Result<Self> {
        // Unlike remote_grid, stdin is PIPED — this protocol is request/response.
        //
        // stderr is piped rather than inherited so a peer's complaint is ours to
        // report instead of scribble on the user's terminal mid-TUI —
        // remote_grid chose `Stdio::null()` for the same child shape precisely
        // because inherited ssh warnings "paint straight over the rendered
        // cells". Capturing without draining would be worse than either
        // endpoint: a peer that writes more than a pipe buffer's worth (16-64KB
        // here) before it speaks blocks mid-write, forever, and nothing in this
        // module has a timeout. ssh reaches that on its own — a `Banner`,
        // `update-motd.d`, a chatty `bash -lc` profile, or our own
        // `StrictHostKeyChecking=accept-new` saying "Warning: Permanently
        // added...". So the drain below starts at spawn, not at first use.
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("piped"));
        let (stderr, drain) = Self::drain_stderr(child.stderr.take().expect("piped"));
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout,
            stderr,
            drain: Some(drain),
            handshake_done: false,
        })
    }

    /// Reads the peer's stderr to EOF on its own thread, keeping the tail.
    ///
    /// The thread ends by itself when the pipe closes, so nothing joins it —
    /// the same reasoning remote_grid gives for not joining its pump. A join in
    /// `Drop` would be the deadlock this exists to avoid: a child's forked
    /// subshells hold the write end open past the child's own exit.
    fn drain_stderr(
        mut pipe: std::process::ChildStderr,
    ) -> (std::sync::Arc<std::sync::Mutex<String>>, std::thread::JoinHandle<()>) {
        let held = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = std::sync::Arc::clone(&held);
        let handle = std::thread::spawn(move || {
            use std::io::Read;
            let mut chunk = [0u8; 8 * 1024];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // Lossy per chunk: a multi-byte character split across
                        // two reads shows as a replacement char. This is
                        // diagnostic text, never protocol, so that is a fair
                        // trade for never failing on a peer's odd bytes.
                        let text = String::from_utf8_lossy(&chunk[..n]);
                        if let Ok(mut buf) = sink.lock() {
                            keep_tail(&mut buf, &text);
                        }
                        // A poisoned lock loses the capture, never the drain:
                        // reading is what keeps the peer unblocked.
                    }
                }
            }
        });
        (held, handle)
    }

    /// Whatever the peer has said on stderr so far.
    ///
    /// A snapshot, not a stream — the drain thread may append after this
    /// returns. Callers quote it in errors, where "so far" is what matters.
    pub fn stderr(&self) -> String {
        match self.stderr.lock() {
            Ok(buf) => buf.clone(),
            // A panic in the drain thread must not turn into a panic here; the
            // bytes it managed to capture are still worth reporting.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Writes one message to the peer.
    ///
    /// Every failure here carries what it was sending and what the peer said on
    /// its way out. Bare, an ssh that died on `Permission denied` surfaces to
    /// the user as "Broken pipe (os error 32)" — technically true and useless.
    pub fn send(&mut self, msg: &Message) -> anyhow::Result<()> {
        use anyhow::Context;
        use std::io::Write;

        let line = wire::encode(msg)
            .with_context(|| format!("encoding a {} message", msg.label()))?;

        let wrote = match self.stdin.as_mut() {
            // `writeln!`, not `write!`: the newline *is* the frame. Without it
            // the peer's `read_line` never returns and both sides wait forever.
            //
            // The `flush` is a no-op today — `ChildStdin` writes straight
            // through to the pipe with no buffer of its own — and kept anyway,
            // because it is the only thing standing between this and a deadlock
            // if that field ever becomes a `BufWriter`.
            Some(stdin) => writeln!(stdin, "{}", line).and_then(|()| stdin.flush()),
            None => anyhow::bail!(
                "cannot send {}: the session with the peer is already finished",
                msg.label()
            ),
        };

        // Built after the borrow above ends, so the peer's stderr can be quoted.
        wrote.with_context(|| {
            format!("sending {} to the peer{}", msg.label(), self.stderr_note())
        })?;
        Ok(())
    }

    /// Ends the session politely and reports how the peer left.
    ///
    /// Closing stdin is not tidiness, it is the whole reason this returns:
    /// `Child::wait` normally closes the child's stdin handle itself, exactly
    /// so a peer reading to EOF can exit — but `spawn` moved that handle into
    /// `self.stdin`, where it lives until `Transport` is dropped, which is
    /// after `wait` would have returned. Waiting without the `take()` below
    /// hangs against any peer that reads until EOF (verified against `cat`).
    /// `Drop`'s SIGKILL hides this today; a graceful close cannot rely on it.
    pub fn finish(&mut self) -> anyhow::Result<PeerExit> {
        use anyhow::Context;
        drop(self.stdin.take());
        let status = self.child.wait().context("waiting for the peer to exit")?;
        self.settle_stderr();
        Ok(PeerExit { status, stderr: self.stderr() })
    }

    /// Lets the drain thread catch up, briefly, so `finish` quotes a complete
    /// stderr rather than whatever happened to be flushed.
    ///
    /// Bounded on purpose. The thread ends at EOF, and EOF normally arrives the
    /// instant the child dies — but a grandchild that inherited the write end
    /// (ssh's `ControlMaster` does exactly that) can hold it open indefinitely,
    /// which is why remote_grid refuses to join its pump at all. So: wait for
    /// the common case, walk away from the pathological one.
    fn settle_stderr(&mut self) {
        let Some(handle) = self.drain.take() else { return };
        let deadline = std::time::Instant::now() + STDERR_SETTLE;
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
        // Otherwise the thread keeps running against a pipe someone else still
        // holds; it owns nothing but an `Arc<Mutex<String>>` and ends on its own.
    }

    /// The next message, or `None` at a clean EOF.
    ///
    /// Lenient exactly once. Before the peer's first decodable line, anything
    /// that will not parse is stepped over: `pam_motd` and `/etc/profile` write
    /// to the *remote command's* stdout, not only to stderr, so a login shell —
    /// which is what `bash -lc` gives us — can prepend a banner to the protocol
    /// stream. `parse_probe` matches whole lines for the same reason, "because
    /// barns print MOTDs".
    ///
    /// After that first message the stream is ours and parsing is strict: a
    /// line that will not decode is a truncated frame or a stray `println!` on
    /// the far side, and skipping it would drop entities without saying so.
    pub fn recv(&mut self) -> anyhow::Result<Option<Message>> {
        let mut skipped = 0usize;
        loop {
            let line = match self.read_line()? {
                Some(l) => l,
                None => return Ok(None), // clean EOF
            };
            let line = line.trim_end();
            match wire::decode(line) {
                Ok(msg) => {
                    self.handshake_done = true;
                    return Ok(Some(msg));
                }
                Err(e) if self.handshake_done => {
                    return Err(anyhow::Error::new(e).context(format!(
                        "the peer sent a line that is not a protocol message: {}{}",
                        snippet(line),
                        self.stderr_note()
                    )));
                }
                Err(_) => {
                    skipped += 1;
                    if skipped > MAX_PREAMBLE_LINES {
                        anyhow::bail!(
                            "no protocol message in the peer's first {} lines of preamble; \
                             last line: {}{}",
                            MAX_PREAMBLE_LINES,
                            snippet(line),
                            self.stderr_note()
                        );
                    }
                }
            }
        }
    }

    /// The tail of the peer's stderr, phrased for an error message, or empty
    /// when it said nothing. This is what piping stderr instead of nulling it
    /// buys: "Broken pipe" plus "Permission denied (publickey)" is a diagnosis.
    fn stderr_note(&self) -> String {
        let said = self.stderr();
        let said = said.trim();
        if said.is_empty() {
            String::new()
        } else {
            format!(" (the peer's stderr said: {})", snippet(said))
        }
    }

    /// One frame off the peer's stdout, or `None` at EOF.
    ///
    /// Bounded by [`MAX_LINE`] — `Take` limits the read without disturbing the
    /// `BufReader` underneath, so whatever was buffered past the cap is still
    /// there for the next call.
    ///
    /// Lossy UTF-8 rather than `read_line`'s hard error: the bytes ahead of the
    /// handshake are a login shell's, not ours, and a stray non-UTF-8 character
    /// in a MOTD should be a skipped line rather than a failed sync. Anything
    /// that matters has to decode as JSON afterwards regardless.
    fn read_line(&mut self) -> anyhow::Result<Option<String>> {
        use std::io::{BufRead, Read};
        let mut buf = Vec::new();
        let n = (&mut self.stdout).take(MAX_LINE).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        if !buf.ends_with(b"\n") && n as u64 >= MAX_LINE {
            anyhow::bail!(
                "the peer sent {} bytes without a newline; one frame is one line{}",
                n,
                self.stderr_note()
            );
        }
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }
}

/// A line, shortened for an error message.
///
/// Cut on a char boundary: this quotes a peer's bytes, which is precisely where
/// a multi-byte character will straddle the limit and panic a naive slice.
fn snippet(line: &str) -> String {
    const LIMIT: usize = 120;
    if line.len() <= LIMIT {
        return format!("{:?}", line);
    }
    let end = line
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= LIMIT)
        .last()
        .unwrap_or(0);
    format!("{:?}...", &line[..end])
}

/// Appends `chunk`, keeping at most [`MAX_STDERR_CAPTURE`] bytes of the tail.
///
/// Trimming lands on a char boundary — `String::drain` panics on anything else,
/// and a peer's stderr is exactly where a stray multi-byte character turns up.
fn keep_tail(buf: &mut String, chunk: &str) {
    buf.push_str(chunk);
    if buf.len() > MAX_STDERR_CAPTURE {
        let excess = buf.len() - MAX_STDERR_CAPTURE;
        let cut = buf
            .char_indices()
            .map(|(i, _)| i)
            .find(|i| *i >= excess)
            .unwrap_or(buf.len());
        buf.drain(..cut);
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        // Kill, then reap. Without the `wait` a defunct ssh accumulates per
        // sync — the same trap remote_grid documents — because a killed child
        // stays a zombie until someone collects its status.
        //
        // Kill rather than close-stdin-and-wait: this is the backstop, not the
        // exit. `finish` is the graceful path — it closes stdin, waits, and
        // hands back the status — and a caller that took it leaves nothing here
        // to do but collect an already-reaped child. Drop runs when that did
        // *not* happen: an error unwound the sync, or a peer wedged mid-exchange
        // would otherwise hold the process open indefinitely. Both errors are
        // ignored on purpose; a peer that already exited on its own, or that
        // `finish` already reaped, is the normal case rather than a failure.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Driven against `cat`, which echoes lines back verbatim. Proves the
    /// read/write pairing without needing ssh or a second yeehaw.
    #[test]
    fn a_request_response_exchange_survives_a_real_child_process() {
        let mut t = Transport::spawn(std::process::Command::new("cat")).unwrap();

        t.send(&Message::Hello { protocol: 1, barn: "a".into(), is_ranch_house: false }).unwrap();
        let got = t.recv().unwrap().expect("cat must echo the line back");
        assert_eq!(got, Message::Hello { protocol: 1, barn: "a".into(), is_ranch_house: false });

        t.send(&Message::Done).unwrap();
        assert_eq!(t.recv().unwrap(), Some(Message::Done));
    }

    #[test]
    fn a_closed_stream_reports_end_not_an_error() {
        let mut t = Transport::spawn(std::process::Command::new("true")).unwrap();
        // `true` exits immediately; recv must report clean EOF rather than erroring.
        assert!(matches!(t.recv(), Ok(None)));
    }

    /// A peer that writes more than a pipe buffer's worth to stderr before it
    /// speaks must not wedge the exchange. 200 KB is comfortably past macOS's
    /// 16-64 KB pipe buffer, so an undrained stderr blocks the child mid-write
    /// and it never reaches the `echo` — both sides then wait forever.
    #[test]
    fn a_peer_that_floods_stderr_still_finishes_the_exchange() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(
            "yes 'Warning: Permanently added the host to the list of known hosts.' \
             | head -c 200000 >&2; \
             echo '{\"msg\":\"done\"}'",
        );
        let mut t = Transport::spawn(cmd).unwrap();
        assert_eq!(
            t.recv().unwrap(),
            Some(Message::Done),
            "a chatty peer's stderr must not block its stdout"
        );
        // Draining is only half the job: the point of piping rather than
        // nulling stderr is that an error path can quote it.
        let said = t.stderr();
        assert!(
            said.contains("Permanently added"),
            "the drained stderr must be readable, got {} bytes: {:?}",
            said.len(),
            said
        );
    }

    /// `Transport` moved `stdin` out of the `Child`, which defeats the
    /// protection `Child::wait` normally provides: it closes its own stdin
    /// handle first precisely so a peer reading to EOF can exit. Ours lives in
    /// a struct field that outlives the wait, so `finish` has to close it
    /// explicitly or teardown hangs. `cat` is the whole test: it exits only
    /// when its stdin closes.
    #[test]
    fn finish_closes_stdin_so_the_peer_can_exit() {
        let mut t = Transport::spawn(std::process::Command::new("cat")).unwrap();
        t.send(&Message::Done).unwrap();
        assert_eq!(t.recv().unwrap(), Some(Message::Done));

        let done = t.finish().unwrap();
        assert!(done.status.success(), "cat exits 0 once stdin closes, got {:?}", done.status);
    }

    /// `recv() -> Ok(None)` alone cannot tell "the peer finished and left" from
    /// "ssh died with Permission denied". The exit status and the captured
    /// stderr are what separate them.
    #[test]
    fn finish_reports_a_failed_peer_and_what_it_said() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("echo 'Permission denied (publickey).' >&2; exit 255");
        let mut t = Transport::spawn(cmd).unwrap();

        assert_eq!(t.recv().unwrap(), None, "this peer never speaks protocol");

        let done = t.finish().unwrap();
        assert_eq!(done.status.code(), Some(255), "an ssh failure must not read as a clean end");
        assert!(
            done.stderr.contains("Permission denied"),
            "finish must carry the peer's complaint, got {:?}",
            done.stderr
        );
    }

    /// ssh is not a clean pipe. `pam_motd` and `/etc/profile` write to the
    /// *remote command's* stdout, not only to stderr, and this codebase already
    /// learned it once: `parse_probe` matches whole lines "because barns print
    /// MOTDs". A single banner line must not sink a sync.
    #[test]
    fn a_line_of_preamble_before_the_first_message_is_skipped() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(
            "echo 'Welcome to Ubuntu 22.04.4 LTS (GNU/Linux 5.15.0 aarch64)'; \
             echo ''; \
             echo '{\"msg\":\"hello\",\"protocol\":1,\"barn\":\"pi\",\"is_ranch_house\":true}'",
        );
        let mut t = Transport::spawn(cmd).unwrap();

        let got = t.recv().expect("a MOTD before the handshake must not be fatal");
        assert_eq!(
            got,
            Some(Message::Hello { protocol: 1, barn: "pi".into(), is_ranch_house: true }),
            "the first decodable line is the handshake"
        );
    }

    /// The tolerance is for the preamble and nothing else. Once the peer has
    /// spoken protocol, a line that will not decode is a real fault — a
    /// truncated frame, a mid-stream `println!` — and swallowing it would lose
    /// entities silently.
    #[test]
    fn garbage_after_the_handshake_is_still_an_error() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(
            "echo '{\"msg\":\"hello\",\"protocol\":1,\"barn\":\"pi\",\"is_ranch_house\":false}'; \
             echo 'sudo: unable to resolve host pi'; \
             echo '{\"msg\":\"done\"}'",
        );
        let mut t = Transport::spawn(cmd).unwrap();

        assert!(matches!(t.recv(), Ok(Some(Message::Hello { .. }))), "handshake first");
        assert!(
            t.recv().is_err(),
            "a non-protocol line after the handshake must not be skipped"
        );
    }

    /// A peer that only ever emits noise must end the exchange, not spin. An
    /// unbounded skip turns a broken far side into a hang with no timeout
    /// anywhere to break it.
    #[test]
    fn an_endless_preamble_terminates_instead_of_looping() {
        let mut cmd = std::process::Command::new("sh");
        // No `head`: this stream never ends on its own.
        cmd.arg("-c").arg("yes 'this is not a protocol message'");
        let mut t = Transport::spawn(cmd).unwrap();

        let err = t.recv().expect_err("an endless preamble must be an error, not a loop");
        let text = err.to_string();
        assert!(
            text.contains("preamble") || text.contains("lines"),
            "the error must say why it gave up, got {:?}",
            text
        );
    }

    /// `read_line` grows its buffer until it sees a newline, and the peer
    /// decides when that is. A far side streaming without one — a binary
    /// dumped into the stream, an ssh that landed on something that is not
    /// yeehaw — must hit a wall here rather than eat memory until the OS ends
    /// the argument. 9 MB against an 8 MiB cap, and the child stays alive
    /// afterwards so an unbounded read has no EOF to rescue it.
    #[test]
    fn a_frame_that_never_ends_is_an_error_not_an_appetite() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg("yes 0123456789abcdef | tr -d '\\n' | head -c 9000000; sleep 60");
        let mut t = Transport::spawn(cmd).unwrap();

        let err = t.recv().expect_err("an unterminated frame must be an error");
        assert!(
            err.to_string().contains("without a newline"),
            "the error must name the cause, got {:?}",
            err.to_string()
        );
    }

    /// A write to a departed peer fails with `Broken pipe (os error 32)` and
    /// nothing else — no hint of which message, or of the "command not found"
    /// the peer put on stderr on its way out. Both are knowable right here.
    #[test]
    fn a_send_to_a_departed_peer_explains_itself() {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("echo 'bash: yeehaw: command not found' >&2; exit 127");
        let mut t = Transport::spawn(cmd).unwrap();

        // EOF on stdout means the child is already gone, so the write below
        // cannot race its exit: the pipe has no reader left and fails at once.
        assert_eq!(t.recv().unwrap(), None, "the peer never speaks protocol");

        let err = t.send(&Message::Done).expect_err("a write to a dead peer must fail");
        let text = format!("{:#}", err);
        assert!(text.contains("done"), "the error must name the message, got {:?}", text);
        assert!(
            text.contains("command not found"),
            "the error must carry what the peer said, got {:?}",
            text
        );
    }

    /// `finish` takes stdin away. Sending afterwards is a caller bug, and it
    /// must read as one rather than as a panic on an absent handle.
    #[test]
    fn a_send_after_finish_says_the_session_is_over() {
        let mut t = Transport::spawn(std::process::Command::new("cat")).unwrap();
        t.finish().unwrap();

        let err = t.send(&Message::Done).expect_err("the session is over");
        assert!(
            format!("{:#}", err).contains("finished"),
            "got {:?}",
            err.to_string()
        );
    }
}
