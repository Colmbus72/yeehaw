//! Lock-guarded file writes for the config store.
//!
//! The guarantee here is *atomic visibility*, not crash durability: a reader
//! of a target path sees either the complete old bytes or the complete new
//! bytes, never a half-written file. That is what the store actually needs,
//! because every writer and reader is a live Yeehaw process racing another.
//!
//! Crash durability is deliberately not claimed. See [`write_atomic`].

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use fs2::FileExt;

/// Distinguishes concurrent `write_atomic` calls inside one process. The pid
/// alone is not enough: two threads writing the same target would build the
/// same temp path, and whichever renamed first would pull the file out from
/// under the other, failing its rename with ENOENT.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Writes `content` to `path` with atomic visibility: a temp file in the same
/// directory is written and renamed over the target, so a concurrent reader
/// observes either the old file or the new one and never a partial write.
///
/// Same-directory placement matters — `fs::rename` is only atomic within a
/// filesystem, and `/tmp` is frequently a different one.
///
/// Concurrent writers of the same path are safe: each call gets its own temp
/// file, so the outcome is atomic last-wins rather than an error.
///
/// This is *not* a crash-durability guarantee. `sync_all` makes the temp
/// file's contents durable, but the rename that publishes them is a directory
/// metadata change and the parent directory is **not** fsynced. After a power
/// loss the target can still resolve to its previous inode. ext4's
/// `data=ordered` mostly papers over this; APFS and network filesystems do
/// not. Torn-read protection, which is the reason this helper exists, holds
/// either way — `rename` delivers it regardless of durability.
pub fn write_atomic(path: &Path, content: &str) -> Result<()> {
    write_atomic_bytes(path, content.as_bytes())
}

/// [`write_atomic`] for content that is not text.
///
/// The store holds one non-UTF-8 file — the bundled `.skill` ZIP archive that
/// `hooks::install_skill` drops into `~/.yeehaw/skills/`. `ensure_config_dirs()`
/// writes it only when it is missing (`config.rs` guards the call with
/// `hooks::skill_installed()`), so in practice that is a fresh ranch or an
/// explicit `yeehaw skills install` — not every load. It still needs
/// temp-and-rename: two processes hitting a first run at once would otherwise
/// hand `read_skill_markdown` a truncated ZIP.
pub fn write_atomic_bytes(path: &Path, content: &[u8]) -> Result<()> {
    write_inner(path, content, None)
}

/// [`write_atomic_bytes`] with explicit permission bits on the target.
///
/// `File::create` gives a fresh temp file `0666 & ~umask`, and the rename
/// carries that mode onto the target — so a plain atomic write silently
/// relaxes the permissions of anything that was hardened, such as the 0600
/// `vault.enc`.
///
/// The chmod is applied to the **temp file, before the rename**. That ordering
/// is the whole point: chmod-after-rename leaves a window in which the
/// finished file is already reachable at its name with default permissions.
///
/// `mode` is ignored on non-Unix targets, which have no such bits.
pub fn write_atomic_with_mode(path: &Path, content: &[u8], mode: u32) -> Result<()> {
    write_inner(path, content, Some(mode))
}

fn write_inner(path: &Path, content: &[u8], mode: Option<u32>) -> Result<()> {
    let dir = path.parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(dir).context("failed to create parent directory")?;

    // Unique per call: the pid keeps concurrent *processes* apart, the counter
    // keeps concurrent *threads* within this process apart.
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> Result<()> {
        {
            let mut f = fs::File::create(&tmp).context("failed to create temp file")?;
            f.write_all(content).context("failed to write temp file")?;
            f.sync_all().context("failed to fsync temp file")?;
        }
        // Before the rename, never after: once the rename lands the file is
        // reachable at its real name, and a chmod after that point is a race
        // any other process can win.
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
                .context("failed to set temp file permissions")?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        fs::rename(&tmp, path).context("failed to rename temp file into place")
    })();

    if result.is_err() {
        // Now that temp names are never reused, a failed call would otherwise
        // orphan its temp file forever. No other writer can hold this name, so
        // removing it cannot disturb anyone.
        let _ = fs::remove_file(&tmp);
    }

    result
}

/// Held for the duration of a read-modify-write. Releases on drop.
pub struct EntityLock {
    file: fs::File,
}

impl Drop for EntityLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// The lock file stem [`lock_entity`] uses for `name`.
///
/// Exposed because a caller that holds two locks at once has to reason about
/// the *lock*, not the name. This mapping is deliberately many-to-one — every
/// character outside `[A-Za-z0-9_-]` collapses to `_`, and `validate_name`
/// permits plenty of those — so `"my api"` and `"my_api"` are two entity names
/// sharing a single lock file.
///
/// Two consequences for a multi-lock path, both of them hangs rather than
/// errors, because `fs2::lock_exclusive()` has no timeout:
///
/// - It must dedupe on this key. Locking one file twice in a thread blocks it
///   against itself forever; `fs2` locks are not re-entrant.
/// - It must *order* on this key, not on the name. Name order and key order can
///   disagree (`'X'` sorts below `'_'`, so `"aXb"` and `"a.b"` swap places when
///   the `.` collapses), and two threads that disagree about the order of the
///   same two locks are the deadlock this ordering exists to prevent.
pub fn lock_key(name: &str) -> String {
    sanitized_stem(name)
}

/// `name` reduced to `[A-Za-z0-9_-]`, everything else collapsed to `_`.
///
/// The one copy of a rule that used to be written out three times, byte for
/// byte, in [`lock_key`], `tombstones::file_stem_for` and
/// `ranch::base::file_stem_for`. All three want the same thing — a caller-
/// supplied string reduced to something safe to put in a filename, with
/// `Path::join` on `"../../escaped"` made structurally impossible rather than
/// left to every call site to check.
///
/// The mapping is many-to-one and that is a property callers must reckon with,
/// not a detail: see [`lock_key`] for what it means when two names share a lock,
/// and `ranch::base::file_stem_for` for a caller that *refuses* an input this
/// changes rather than accepting the collision.
pub(crate) fn sanitized_stem(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Takes an exclusive advisory lock for one entity.
///
/// Lock files live in a `.locks` subdirectory of `dir` and are never deleted —
/// deleting them would race with another process that has the same path open,
/// which is the classic way to break advisory locking. They are empty, and
/// there is one per [`lock_key`].
///
/// Advisory locks are cooperative: they only exclude other callers of this
/// function. That is sufficient here, because every writer is Yeehaw.
///
/// Holding more than one of these at a time is allowed, but only in the total
/// order documented on [`lock_key`] and demonstrated by `config::rename_project`.
pub fn lock_entity(dir: &Path, name: &str) -> Result<EntityLock> {
    let lock_dir = dir.join(".locks");
    fs::create_dir_all(&lock_dir).context("failed to create lock directory")?;

    let safe = lock_key(name);

    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_dir.join(format!("{}.lock", safe)))
        .context("failed to open lock file")?;

    file.lock_exclusive().context("failed to acquire entity lock")?;
    Ok(EntityLock { file })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.yaml");
        write_atomic(&path, "hello").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.yaml");
        write_atomic(&path, "first").unwrap();
        write_atomic(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn leaves_no_temp_files_behind() {
        let tmp = tempfile::tempdir().unwrap();
        write_atomic(&tmp.path().join("a.yaml"), "x").unwrap();
        let names: Vec<String> = fs::read_dir(tmp.path()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.yaml".to_string()]);
    }

    /// Temp names are now unique per call, so a failed write can no longer be
    /// tidied up by a later write reusing the same name. The error path has to
    /// remove its own temp file or it orphans one permanently.
    #[test]
    fn failed_write_leaves_no_temp_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.yaml");
        // A directory at the target makes the final rename fail.
        fs::create_dir(&path).unwrap();

        assert!(write_atomic(&path, "x").is_err(), "expected rename onto a directory to fail");

        let names: Vec<String> = fs::read_dir(tmp.path()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.yaml".to_string()], "orphaned temp file: {:?}", names);
    }

    /// Two threads writing the same target path must both succeed. The temp
    /// file name has to be unique per *call*, not per process: if it is not,
    /// both threads create the same temp path, the first rename moves it away,
    /// and the second rename fails with ENOENT. Correct behavior for
    /// concurrent writers of one file is atomic last-wins, never an error.
    #[test]
    fn concurrent_writers_to_same_path_all_succeed() {
        use std::sync::Arc;

        let tmp = Arc::new(tempfile::tempdir().unwrap());
        let path = tmp.path().join("a.yaml");

        const THREADS: usize = 8;
        const ROUNDS: usize = 40;

        // Each thread writes a distinct, easily identifiable payload.
        let contents: Vec<String> = (0..THREADS)
            .map(|i| format!("writer-{}", i).repeat(500))
            .collect();

        let mut handles = Vec::new();
        for i in 0..THREADS {
            let path = path.clone();
            let content = contents[i].clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..ROUNDS {
                    // Assertion 1: no call may return Err.
                    write_atomic(&path, &content)
                        .unwrap_or_else(|e| panic!("writer {} failed: {:?}", i, e));
                    std::thread::yield_now();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Assertion 2: the surviving file is exactly one of the written values,
        // never a mixture and never empty.
        let final_content = fs::read_to_string(&path).unwrap();
        assert!(
            contents.iter().any(|c| *c == final_content),
            "final content is not any single writer's payload (len {})",
            final_content.len()
        );
    }

    #[test]
    fn creates_missing_parent_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("deep").join("nested").join("a.yaml");
        write_atomic(&path, "x").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");
    }

    /// A reader must observe either the old bytes or the new bytes — never a
    /// partial file. Without temp-and-rename this fails intermittently.
    #[test]
    fn concurrent_readers_never_see_partial_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.yaml");
        let old = "a".repeat(200_000);
        let new = "b".repeat(200_000);
        write_atomic(&path, &old).unwrap();

        let reader_path = path.clone();
        let (old_c, new_c) = (old.clone(), new.clone());
        let reader = std::thread::spawn(move || {
            for _ in 0..500 {
                if let Ok(s) = fs::read_to_string(&reader_path) {
                    assert!(s == old_c || s == new_c, "torn read: {} bytes", s.len());
                }
            }
        });

        for _ in 0..100 {
            write_atomic(&path, &new).unwrap();
            write_atomic(&path, &old).unwrap();
        }
        reader.join().unwrap();
    }

    #[test]
    fn lock_serializes_concurrent_read_modify_write() {
        use std::sync::Arc;

        let tmp = Arc::new(tempfile::tempdir().unwrap());
        let path = tmp.path().join("counter.txt");
        fs::write(&path, "0").unwrap();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let tmp = Arc::clone(&tmp);
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    let _guard = lock_entity(tmp.path(), "counter").unwrap();
                    let n: u64 = fs::read_to_string(&path).unwrap().trim().parse().unwrap();
                    std::thread::yield_now();
                    write_atomic(&path, &(n + 1).to_string()).unwrap();
                }
            }));
        }
        for h in handles { h.join().unwrap(); }

        let final_n: u64 = fs::read_to_string(&path).unwrap().trim().parse().unwrap();
        assert_eq!(final_n, 400, "lost updates: lock did not serialize writers");
    }

    /// The target's permission bits must be exactly what the caller asked for.
    /// A fresh temp file is created with `0666 & ~umask`, and `rename` carries
    /// that mode onto the target, so a 0600 secret written this way would land
    /// world-readable unless the mode is applied explicitly.
    #[cfg(unix)]
    #[test]
    fn write_atomic_with_mode_sets_the_requested_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret.enc");
        write_atomic_with_mode(&path, b"shh", 0o600).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
        assert_eq!(fs::read(&path).unwrap(), b"shh");
    }

    /// The chmod happens on the temp file, *before* the rename. Doing it after
    /// leaves a window in which the finished secret is visible at the default
    /// mode. The observable consequence is that an existing target's old mode
    /// never leaks into the new file, and the new mode is in force the instant
    /// the name resolves.
    #[cfg(unix)]
    #[test]
    fn write_atomic_with_mode_overrides_an_existing_targets_mode() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret.enc");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        write_atomic_with_mode(&path, b"new", 0o600).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
    }

    #[test]
    fn write_atomic_with_mode_leaves_no_temp_files_behind() {
        let tmp = tempfile::tempdir().unwrap();
        write_atomic_with_mode(&tmp.path().join("secret.enc"), b"x", 0o600).unwrap();
        let names: Vec<String> = fs::read_dir(tmp.path()).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["secret.enc".to_string()]);
    }
}
