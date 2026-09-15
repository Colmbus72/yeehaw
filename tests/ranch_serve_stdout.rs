//! `ranch serve` speaks a protocol on stdout. This pins that nothing else does.
//!
//! DEVIATION FROM THE PLAN, deliberate. Phase 2's plan says "no `tests/`
//! directory" — every other test in this crate is an inline `#[cfg(test)] mod
//! tests` — and then specifies `env!("CARGO_BIN_EXE_yeehaw")` for this one test.
//! Those two instructions are incompatible: cargo defines `CARGO_BIN_EXE_*`
//! only for integration targets, so the macro does not compile inside the
//! binary's own unit tests.
//!
//! Both workarounds for keeping it inline were measured and rejected:
//!
//! - Deriving `target/debug/yeehaw` from `std::env::current_exe()` compiles,
//!   but `cargo test` does not build that binary for a bin-only crate with no
//!   integration targets (verified: its mtime does not move across a
//!   `cargo test` following a `touch src/main.rs`). The test would silently
//!   pass against a stale binary — the exact failure this test exists to catch
//!   would be the one it missed.
//! - Shelling out to `cargo build --bin yeehaw` from inside the test does not
//!   deadlock on cargo's target lock, but costs ~19s on *every* run, not just
//!   after an edit: the test and non-test builds of the bin invalidate each
//!   other's fingerprints, so the "no-op" rebuild is never a no-op.
//!
//! An integration target is the mechanism cargo provides for exactly this, and
//! it is what makes the freshness guarantee real: adding this file is also what
//! makes `cargo test` build `target/debug/yeehaw` at all.

/// `ranch serve` speaks a protocol on stdout, so anything else written there
/// corrupts the stream. `config::ensure_config_dirs()` runs at `main.rs:42`,
/// *before* subcommand dispatch — it is silent today, and this pins it, because
/// the failure mode is a peer that cannot parse our first message and has no
/// way to say why.
///
/// A real subprocess, deliberately: the point is the startup path that runs
/// before `handle_subcommands`, which an in-process call would skip. The
/// `cfg(test)` no-harness panic in `config::yeehaw_dir()` does not reach a
/// spawned binary, so `YEEHAW_HOME` is the only thing keeping this off the
/// developer's real ranch.
#[test]
fn the_startup_path_writes_nothing_to_stdout() {
    let exe = env!("CARGO_BIN_EXE_yeehaw");
    let ranch = tempfile::tempdir().unwrap();

    let out = std::process::Command::new(exe)
        .args(["ranch", "serve", "--selftest"])
        .env("YEEHAW_HOME", ranch.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();

    // Without this the test passes vacuously. A serve that exits before writing
    // anything has an empty stdout, and an empty stdout trivially contains no
    // non-protocol lines — so the loop below would prove nothing.
    assert!(
        !lines.is_empty(),
        "serve must speak at least one protocol line; got nothing. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    for line in lines {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "non-protocol line on stdout would corrupt the stream: {:?}",
            line
        );
    }
}

/// Spawns a real `ranch serve`, sends `frames`, closes stdin, and reports how it
/// exited plus every line it put on stdout.
///
/// `wait_with_output` rather than a hand-rolled read: it pumps stdout and stderr
/// concurrently, so a peer that says something on both cannot deadlock against a
/// reader that is busy with the other.
fn converse(frames: &[&str]) -> (std::process::ExitStatus, Vec<serde_json::Value>) {
    use std::io::Write;

    let exe = env!("CARGO_BIN_EXE_yeehaw");
    let ranch = tempfile::tempdir().unwrap();

    let mut child = std::process::Command::new(exe)
        .args(["ranch", "serve"])
        .env("YEEHAW_HOME", ranch.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    {
        let mut stdin = child.stdin.take().unwrap();
        for frame in frames {
            // Ignored: a serve that has already refused the session exits
            // without reading the rest, so the write fails with EPIPE. That is
            // the behaviour under test, not a failure of the test.
            let _ = writeln!(stdin, "{}", frame);
        }
        // Dropped here: closing stdin is the EOF a session ends on.
    }

    let out = child.wait_with_output().unwrap();
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| {
                panic!("non-protocol line on stdout: {:?} ({})\nstderr: {}", l, e,
                       String::from_utf8_lossy(&out.stderr))
            })
        })
        .collect();
    (out.status, lines)
}

/// The carried-forward gap, through the real binary.
///
/// `serve` greets before it reads, so an incompatible peer cannot be turned away
/// at the door: the refusal has to arrive as a message the client can parse, and
/// the process then has to *exit*. A silent close would reach the client as
/// `recv() -> Ok(None)`, indistinguishable from ssh dying.
///
/// The in-process test of `serve_session` covers the decision; this covers the
/// wiring it is reached through — the real stdin lock, the real stdout, and the
/// startup path before `handle_subcommands`.
#[test]
fn an_incompatible_peer_gets_an_error_and_serve_exits_cleanly() {
    let (status, said) =
        converse(&[r#"{"msg":"hello","protocol":99999,"barn":"pi","is_ranch_house":true}"#]);

    assert_eq!(said.len(), 2, "greeting then refusal, got {:?}", said);
    assert_eq!(said[0]["msg"], "hello");
    assert_eq!(said[1]["msg"], "error", "an incompatible version must be refused in words");
    let why = said[1]["message"].as_str().unwrap_or_default();
    assert!(
        why.contains("99999"),
        "the refusal must name the version the peer declared: {:?}",
        why
    );

    // Exit 0: the refusal is a protocol outcome the client already has on the
    // wire, not a crash. A non-zero status would land it in the same bucket as
    // `Permission denied (publickey)` in `transport::PeerExit`.
    assert!(status.success(), "a refused session is still a clean exit, got {:?}", status);
}

/// The hang. A client sends something and blocks on a reply; a serve that drains
/// and discards never writes one, and never reaches EOF either, because the
/// client is still holding its end open waiting for the answer.
///
/// DELIBERATE CHANGE, Task D5. This was driven with a `manifest`, which `serve`
/// now answers with a manifest of its own — that exchange is what D5 adds, and
/// `a_manifest_is_exchanged_through_the_real_binary` below pins it.
///
/// DELIBERATE CHANGE, the push slice. It was then driven with an `entity`, on
/// the grounds that "a joining client reads and applies, it never pushes, so
/// nothing on this side accepts an entity yet". It pushes now: an `entity` is
/// held for the `commit` that authorizes writing it, and is deliberately the one
/// frame with no reply of its own. So this moves to `applied`, which is
/// unimplemented here for a reason that will not expire — it is the *house's*
/// answer to a commit, and a `serve` never receives one. The assertion is
/// otherwise untouched.
#[test]
fn a_message_serve_cannot_answer_gets_an_error_rather_than_silence() {
    let (status, said) = converse(&[r#"{"msg":"applied","keys":["project/abc"]}"#]);

    assert_eq!(said.len(), 2, "the message must be answered, got {:?}", said);
    assert_eq!(said[0]["msg"], "hello");
    assert_eq!(said[1]["msg"], "error", "silence is the one answer a client cannot act on");
    assert!(
        said[1]["message"].as_str().unwrap_or_default().contains("applied"),
        "the error must name what went unanswered: {:?}",
        said[1]
    );
    assert!(status.success(), "got {:?}", status);
}

/// The manifest exchange, through the real binary. A client cannot ask for what
/// it has not been told about, so a manifest has to be answered with one.
///
/// An empty ranch deliberately: the point is that the *answer* is a manifest,
/// and a fresh `YEEHAW_HOME` is the only ranch this test is allowed to have.
#[test]
fn a_manifest_is_exchanged_through_the_real_binary() {
    let (status, said) = converse(&[r#"{"msg":"manifest","entries":[],"tombstones":[]}"#]);

    assert_eq!(said.len(), 2, "the manifest must be answered, got {:?}", said);
    assert_eq!(said[0]["msg"], "hello");
    assert_eq!(
        said[1]["msg"], "manifest",
        "a manifest is answered with a manifest, not an error: {:?}",
        said[1]
    );
    assert!(status.success(), "got {:?}", status);
}
