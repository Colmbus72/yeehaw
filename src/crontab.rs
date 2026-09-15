//! The `yeehaw`-managed block of the user's crontab.

use anyhow::Result;

use crate::config;
use crate::types::Worm;

const BEGIN_MARKER: &str = "# BEGIN YEEHAW MANAGED - DO NOT EDIT";
const END_MARKER: &str = "# END YEEHAW MANAGED";

/// The real `crontab(1)`. Compiled out of test builds — see [`sync_crontab`].
#[cfg(not(test))]
mod sys {
    use std::io::Write;
    use std::process::Command;

    use anyhow::{Context, Result};

    pub fn read_crontab() -> String {
        Command::new("crontab")
            .arg("-l")
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn write_crontab(content: &str) -> Result<()> {
        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn crontab")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(content.as_bytes())
                .context("Failed to write to crontab stdin")?;
        }

        let status = child.wait().context("Failed to wait for crontab")?;
        if !status.success() {
            anyhow::bail!("crontab command failed");
        }
        Ok(())
    }
}

/// A thread-local stand-in for `crontab(1)`. See [`sync_crontab`] for why the
/// real binary must never be reached from a test.
///
/// Thread-local for the same reason `testing::RANCH` is: the Rust test harness
/// gives each test its own thread, so per-thread state is per-test isolation
/// with no lock and no ordering between tests. A thread spawned *inside* a test
/// gets its own empty recorder, exactly as it gets its own (absent) ranch.
#[cfg(test)]
mod sys {
    use std::cell::RefCell;

    use anyhow::Result;

    #[derive(Default)]
    struct Recorder {
        /// What `crontab -l` reports.
        current: String,
        /// Every payload `crontab -` was handed, oldest first.
        writes: Vec<String>,
    }

    thread_local! {
        static REC: RefCell<Recorder> = RefCell::new(Recorder::default());
    }

    pub fn read_crontab() -> String {
        REC.with(|r| r.borrow().current.clone())
    }

    pub fn write_crontab(content: &str) -> Result<()> {
        REC.with(|r| r.borrow_mut().writes.push(content.to_string()));
        Ok(())
    }

    /// Sets what the next [`read_crontab`] reports, standing in for lines the
    /// user already had in their crontab.
    pub fn set_crontab(content: &str) {
        REC.with(|r| r.borrow_mut().current = content.to_string());
    }

    /// Every payload `sync_crontab` would have installed on this thread,
    /// oldest first. Empty means it never got as far as writing.
    pub fn written_crontabs() -> Vec<String> {
        REC.with(|r| r.borrow().writes.clone())
    }
}

fn yeehaw_binary() -> String {
    // Prefer `which yeehaw` to find the installed binary (mirrors TS version).
    // This avoids baking in a cargo target/ path which can trigger syspolicyd
    // loops on macOS when the dev binary is unsigned.
    if let Ok(output) = std::process::Command::new("which")
        .arg("yeehaw")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return path;
            }
        }
    }
    // Fallback to current executable path
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "yeehaw".to_string())
}

fn build_crontab_entry(worm: &Worm) -> String {
    let bin = yeehaw_binary();
    format!("{} {} worm exec {}", worm.schedule, bin, worm.name)
}

/// Rewrites the `yeehaw`-managed block of the user's crontab from the enabled
/// worms in the ranch, preserving every line outside the markers.
///
/// # Never reaches `crontab(1)` under `cfg(test)`
///
/// This is a read-modify-write of a resource that is neither under `~/.yeehaw`
/// nor scoped to this process: the *machine's* crontab. It reads the whole
/// file, rebuilds the managed block from whatever the worm store currently
/// holds, and installs the result.
///
/// Under `cfg(test)` the worm store is a fresh, empty temp ranch. Reaching the
/// real binary from there rebuilds the managed block from *no worms* and
/// installs that — deleting every yeehaw cron entry the developer has,
/// silently, while the test reports `ok`. `testing::temp_ranch()` cannot
/// prevent it, because the crontab is not a file under the ranch.
///
/// That was not hypothetical. `config::unlink_trail_from_livestock` reaches
/// `config::remove_poll_worm`, which calls this unconditionally, so one
/// ordinary `updated_at` test in `config.rs` wiped the developer's crontab on
/// every `cargo test`. Guarding the single test would have left the next one
/// free to reintroduce it, so the guard lives here instead: under `cfg(test)`
/// the two ends that leave the process are redirected at a thread-local
/// recorder. Everything between them — marker stripping, the enabled filter,
/// entry building — is still the real code path, so a test can assert exactly
/// what *would* have been installed via [`sys::written_crontabs`].
pub fn sync_crontab() -> Result<()> {
    let current = sys::read_crontab();

    // Strip existing yeehaw section
    let mut lines: Vec<&str> = Vec::new();
    let mut in_section = false;
    for line in current.lines() {
        if line.trim() == BEGIN_MARKER {
            in_section = true;
            continue;
        }
        if line.trim() == END_MARKER {
            in_section = false;
            continue;
        }
        if !in_section {
            lines.push(line);
        }
    }

    // Remove trailing empty lines
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }

    // Build new section from enabled worms
    let worms = config::load_worms();
    let enabled: Vec<&Worm> = worms.iter().filter(|w| w.enabled).collect();

    let mut new_content = lines.join("\n");
    if !new_content.is_empty() {
        new_content.push('\n');
    }

    if !enabled.is_empty() {
        new_content.push('\n');
        new_content.push_str(BEGIN_MARKER);
        new_content.push('\n');
        for worm in &enabled {
            new_content.push_str(&build_crontab_entry(worm));
            new_content.push('\n');
        }
        new_content.push_str(END_MARKER);
        new_content.push('\n');
    }

    sys::write_crontab(&new_content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn worm(name: &str, schedule: &str, enabled: bool) -> Worm {
        Worm {
            name: name.into(),
            command: "echo hi".into(),
            schedule: schedule.into(),
            worm_type: "shell".into(),
            enabled,
            project: None,
            working_dir: None,
            id: None,
            created_at: None,
            updated_at: None,
        }
    }

    /// The seam replaces only the two ends that leave the process. Everything
    /// between them is the shipping code path, so the payload it hands the
    /// recorder is byte-for-byte what the real `crontab -` would have received.
    #[test]
    fn installs_every_enabled_worm_between_the_markers() {
        testing::with_temp_ranch(|_| {
            config::save_worm(&mut worm("nightly", "0 3 * * *", true)).unwrap();
            config::save_worm(&mut worm("hourly", "0 * * * *", true)).unwrap();

            sync_crontab().unwrap();

            let installed = sys::written_crontabs();
            assert_eq!(installed.len(), 1, "exactly one crontab install per sync");
            let body = &installed[0];

            assert!(body.contains(BEGIN_MARKER) && body.contains(END_MARKER));
            for (name, schedule) in [("nightly", "0 3 * * *"), ("hourly", "0 * * * *")] {
                let line = body
                    .lines()
                    .find(|l| l.ends_with(&format!(" worm exec {}", name)))
                    .unwrap_or_else(|| panic!("no entry for '{}' in:\n{}", name, body));
                assert!(
                    line.starts_with(&format!("{} ", schedule)),
                    "'{}' must carry its own schedule, got: {}",
                    name,
                    line
                );
            }
        });
    }

    /// A disabled worm is a worm the user turned off. Installing it anyway
    /// would run it on schedule regardless.
    #[test]
    fn a_disabled_worm_is_not_installed() {
        testing::with_temp_ranch(|_| {
            config::save_worm(&mut worm("nightly", "0 3 * * *", true)).unwrap();
            config::save_worm(&mut worm("retired", "0 4 * * *", false)).unwrap();

            sync_crontab().unwrap();

            let body = sys::written_crontabs().remove(0);
            assert!(body.contains("worm exec nightly"));
            assert!(
                !body.contains("worm exec retired"),
                "a disabled worm must not reach the crontab:\n{}",
                body
            );
        });
    }

    /// The user's own crontab lines live in the same file. The managed block is
    /// replaced; everything outside it has to survive untouched.
    #[test]
    fn lines_outside_the_managed_block_survive_a_sync() {
        testing::with_temp_ranch(|_| {
            sys::set_crontab(&format!(
                "MAILTO=cam\n0 6 * * * /usr/local/bin/backup\n{}\n0 9 * * * stale-entry\n{}\n",
                BEGIN_MARKER, END_MARKER
            ));
            config::save_worm(&mut worm("nightly", "0 3 * * *", true)).unwrap();

            sync_crontab().unwrap();

            let body = sys::written_crontabs().remove(0);
            assert!(body.contains("MAILTO=cam"), "user lines lost:\n{}", body);
            assert!(body.contains("/usr/local/bin/backup"), "user lines lost:\n{}", body);
            assert!(
                !body.contains("stale-entry"),
                "the previous managed block must be replaced, not kept:\n{}",
                body
            );
            assert!(body.contains("worm exec nightly"));
        });
    }

    /// With no enabled worms the managed block disappears entirely rather than
    /// being written as an empty pair of markers.
    #[test]
    fn an_empty_ranch_installs_no_managed_block() {
        testing::with_temp_ranch(|_| {
            sys::set_crontab("MAILTO=cam\n");

            sync_crontab().unwrap();

            let body = sys::written_crontabs().remove(0);
            assert_eq!(body, "MAILTO=cam\n");
        });
    }

    /// The recorder is per-thread, which is what makes it per-test: one test's
    /// syncs must never show up in another's. A thread spawned inside a test
    /// starts with its own empty recorder, exactly as it starts with no ranch.
    #[test]
    fn a_spawned_thread_does_not_see_this_threads_installs() {
        let ranch = testing::temp_ranch();
        config::save_worm(&mut worm("nightly", "0 3 * * *", true)).unwrap();
        sync_crontab().unwrap();
        assert_eq!(sys::written_crontabs().len(), 1);

        let path = ranch.dir.path().to_path_buf();
        std::thread::spawn(move || {
            testing::with_ranch_env(path, || {
                assert!(
                    sys::written_crontabs().is_empty(),
                    "a spawned thread must start with a clean recorder"
                );
            })
        })
        .join()
        .unwrap();
    }
}
