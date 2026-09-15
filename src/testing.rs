//! Test-only helpers. Compiled only under `cfg(test)`.

use std::cell::RefCell;
use std::ffi::OsString;

/// What `config::yeehaw_dir()` sees in place of the real `YEEHAW_HOME`.
#[derive(Clone, Debug)]
pub enum RanchEnv {
    /// `YEEHAW_HOME` as if it held exactly this value.
    Set(OsString),
    /// `YEEHAW_HOME` as if it were absent, so `yeehaw_dir()` takes the
    /// `$HOME/.yeehaw` fallback.
    Unset,
}

thread_local! {
    /// The ranch the calling thread's test is pointed at.
    ///
    /// Deliberately *not* an env var. `std::env::set_var` mutates process-wide
    /// state and is not thread-safe against concurrent `getenv` / `environ`
    /// iteration, and this suite has real concurrent readers: every
    /// `tempfile::tempdir()` reads `TMPDIR`, and every `Command` spawn walks
    /// `environ`. Rust 2024 made `set_var` unsafe for exactly this reason;
    /// this crate is edition 2021, so it compiled silently.
    ///
    /// Being thread-local also means there is no shared state left to
    /// serialize, so no lock is needed and tests run fully in parallel.
    static RANCH: RefCell<Option<RanchEnv>> = const { RefCell::new(None) };
}

/// The ranch override for the calling thread, if any.
///
/// Under `cfg(test)` this is the *only* thing `config::yeehaw_dir()` consults.
/// The process's own `YEEHAW_HOME` is ignored there, so a developer who happens
/// to have it exported cannot silently disarm the no-harness panic for the
/// whole suite.
pub fn ranch_env() -> Option<RanchEnv> {
    RANCH.with(|r| r.borrow().clone())
}

/// Restores the previous override on drop, so a panicking test cannot leak its
/// ranch into whatever the harness runs next on the same thread.
struct RanchGuard(Option<RanchEnv>);

impl Drop for RanchGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        RANCH.with(|r| *r.borrow_mut() = previous);
    }
}

fn set_ranch(value: Option<RanchEnv>) -> RanchGuard {
    RanchGuard(RANCH.with(|r| r.replace(value)))
}

pub struct TempRanch {
    /// Field order is load-bearing: Rust drops fields in declaration order, so
    /// the guard must come first. It puts the previous override back *before*
    /// `dir` deletes the temp directory, leaving no window in which the
    /// thread-local ranch names a path that no longer exists.
    _guard: RanchGuard,
    pub dir: tempfile::TempDir,
}

/// Runs `f` with `~/.yeehaw` pointed at a fresh temp directory.
///
/// Every test that reaches `config::yeehaw_dir()` — directly, or through any
/// `config::`, `ssh::`, `tmux::` or `hooks::` function that builds a path under
/// the ranch — must be wrapped in this. Without it `yeehaw_dir()` panics under
/// `cfg(test)` rather than let a forgotten harness read or write the
/// developer's real ranch.
///
/// CONSTRAINT: the override is thread-local, and a thread spawned inside `f`
/// does **not** inherit it. Such a thread calling `yeehaw_dir()` panics with
/// the no-harness message. A worker thread that needs the temp ranch has to
/// establish its own — pass it `ranch.dir.path()` and wrap its body in
/// [`with_ranch_env`].
pub fn with_temp_ranch<T>(f: impl FnOnce(&TempRanch) -> T) -> T {
    f(&temp_ranch())
}

/// Points `~/.yeehaw` at a fresh temp directory until the returned value is
/// dropped.
///
/// The statement form of [`with_temp_ranch`], for tests whose whole body would
/// otherwise be wrapped in a closure just to establish a ranch:
///
/// ```ignore
/// let _ranch = testing::temp_ranch();
/// ```
///
/// Same thread-local constraint: see [`with_temp_ranch`].
pub fn temp_ranch() -> TempRanch {
    let dir = tempfile::tempdir().unwrap();
    let guard = set_ranch(Some(RanchEnv::Set(dir.path().as_os_str().to_os_string())));
    TempRanch { _guard: guard, dir }
}

/// Runs `f` with `yeehaw_dir()` seeing exactly `value` as `YEEHAW_HOME`.
///
/// For the cases where the *value* is what is under test — empty, relative,
/// not valid UTF-8 — without touching the process environment.
pub fn with_ranch_env<T>(value: impl Into<OsString>, f: impl FnOnce() -> T) -> T {
    let _guard = set_ranch(Some(RanchEnv::Set(value.into())));
    f()
}

/// Runs `f` with `YEEHAW_HOME` seen as absent, so `yeehaw_dir()` falls back to
/// `$HOME/.yeehaw`.
///
/// The explicit opt-out from the no-harness panic: that fallback is real
/// shipping behavior and has to stay exercisable. It only computes a path —
/// callers must not create or write anything under it.
pub fn without_yeehaw_home<T>(f: impl FnOnce() -> T) -> T {
    let _guard = set_ranch(Some(RanchEnv::Unset));
    f()
}

/// Runs `f` with no override at all — exactly the state a test that forgot the
/// harness, or a thread spawned inside one, starts in.
///
/// Only for testing the guard itself.
pub fn without_ranch_override<T>(f: impl FnOnce() -> T) -> T {
    let _guard = set_ranch(None);
    f()
}
