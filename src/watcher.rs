use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// Top-level entries under the ranch dir that hold run output and runtime
/// state, not config. Nothing in them is read by `App::reload`, and they are
/// written constantly: a trail appends to `trail-runs/…/step-N.log` once per
/// output line, so a `cargo build` step was a full reload of every project,
/// worm and barn for every line it printed.
const IGNORED: &[&str] = &[
    "trail-runs",
    "worm-runs",
    "session-signals",
    "poll-state",
    "ssh",
    "panic.log",
];

/// Whether a change at `path` can matter to the app. Paths outside both
/// spellings of the root are kept: better a spare reload than a missed one.
fn is_relevant(path: &Path, roots: &[&Path]) -> bool {
    for root in roots {
        if let Ok(rel) = path.strip_prefix(root) {
            return match rel.components().next() {
                Some(first) => !IGNORED.iter().any(|i| first.as_os_str() == *i),
                None => true,
            };
        }
    }
    true
}

pub enum WatchEvent {
    ConfigChanged,
    WormTrigger(String),
}

pub fn start_watcher(yeehaw_dir: &Path) -> Option<mpsc::Receiver<WatchEvent>> {
    let (tx, rx) = mpsc::channel();
    let triggers_dir = yeehaw_dir.join("worm-triggers");

    let event_tx = tx.clone();
    // FSEvents reports canonical paths, so compare against the canonical form
    // or a symlinked ranch dir would never match a trigger.
    let triggers_path = triggers_dir.canonicalize().unwrap_or_else(|_| triggers_dir.clone());
    let root = yeehaw_dir.to_path_buf();
    let root_canon = yeehaw_dir.canonicalize().unwrap_or_else(|_| root.clone());

    let mut watcher = match RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    notify::EventKind::Create(_)
                    | notify::EventKind::Modify(_)
                    | notify::EventKind::Remove(_) => {
                        let roots = [root.as_path(), root_canon.as_path()];
                        if !event.paths.iter().any(|p| is_relevant(p, &roots)) {
                            return;
                        }
                        // Check if this is a worm trigger file
                        for path in &event.paths {
                            if path.starts_with(&triggers_path) || path.starts_with(&triggers_dir) {
                                if let Some(filename) = path.file_name() {
                                    let _ = event_tx.send(WatchEvent::WormTrigger(
                                        filename.to_string_lossy().to_string(),
                                    ));
                                    return;
                                }
                            }
                        }
                        let _ = event_tx.send(WatchEvent::ConfigChanged);
                    }
                    _ => {}
                }
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    ) {
        Ok(w) => w,
        Err(_) => return None,
    };

    if watcher.watch(yeehaw_dir, RecursiveMode::Recursive).is_err() {
        return None;
    }

    // Leak the watcher so it stays alive for the lifetime of the process.
    // The app runs until exit, so this is fine.
    std::mem::forget(watcher);

    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trigger file dropped into `worm-triggers/` must arrive as a
    /// `WormTrigger`, not a generic reload. The temp dir is under `/var`, which
    /// macOS symlinks to `/private/var` — FSEvents reports the canonical path,
    /// so this also covers the prefix comparison.
    #[test]
    fn run_output_is_not_config() {
        let root = Path::new("/r");
        let roots = [root];
        assert!(!is_relevant(Path::new("/r/trail-runs/app/deploy/x/step-0.log"), &roots));
        assert!(!is_relevant(Path::new("/r/worm-runs/nightly/a.log"), &roots));
        assert!(!is_relevant(Path::new("/r/session-signals/%3.json"), &roots));
        assert!(!is_relevant(Path::new("/r/panic.log"), &roots));
        assert!(is_relevant(Path::new("/r/barns/pi.yaml"), &roots));
        assert!(is_relevant(Path::new("/r/config.yaml"), &roots));
        assert!(is_relevant(Path::new("/r/worm-triggers/n-1.json"), &roots));
        // A sibling whose name merely starts with an ignored one still counts.
        assert!(is_relevant(Path::new("/r/trail-runs-old/x"), &roots));
    }

    /// End to end on the real backend: a burst of log writes produces nothing,
    /// a barn edit produces a reload.
    #[test]
    fn log_writes_do_not_reload_but_config_edits_do() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("trail-runs").join("app").join("deploy");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(dir.path().join("barns")).unwrap();
        let rx = start_watcher(dir.path()).expect("watcher starts");
        std::thread::sleep(Duration::from_millis(500));

        for i in 0..50 {
            std::fs::write(logs.join(format!("step-{}.log", i % 3)), format!("line {i}")).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1000));
        assert!(rx.try_recv().is_err(), "trail log writes reached the app as events");

        std::fs::write(dir.path().join("barns").join("pi.yaml"), "name: pi").unwrap();
        let got = rx.recv_timeout(Duration::from_secs(10));
        assert!(matches!(got, Ok(WatchEvent::ConfigChanged)), "barn edit did not reload");
    }

    #[test]
    fn a_trigger_file_arrives_as_a_worm_trigger() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("worm-triggers")).unwrap();
        let rx = start_watcher(dir.path()).expect("watcher starts");
        // FSEvents streams start asynchronously; give it a moment to attach.
        std::thread::sleep(Duration::from_millis(500));

        std::fs::write(dir.path().join("worm-triggers").join("nightly-1.json"), "{}").unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Ok(WatchEvent::WormTrigger(name)) = rx.recv_timeout(Duration::from_millis(200)) {
                assert_eq!(name, "nightly-1.json");
                return;
            }
        }
        panic!("no WormTrigger for a file written into worm-triggers/");
    }
}
