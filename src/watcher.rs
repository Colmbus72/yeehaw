use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

pub enum WatchEvent {
    ConfigChanged,
    WormTrigger(String),
}

pub fn start_watcher(yeehaw_dir: &Path) -> Option<mpsc::Receiver<WatchEvent>> {
    let (tx, rx) = mpsc::channel();
    let triggers_dir = yeehaw_dir.join("worm-triggers");

    let event_tx = tx.clone();
    let triggers_path = triggers_dir.clone();

    let mut watcher = match RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    notify::EventKind::Create(_)
                    | notify::EventKind::Modify(_)
                    | notify::EventKind::Remove(_) => {
                        // Check if this is a worm trigger file
                        for path in &event.paths {
                            if path.starts_with(&triggers_path) {
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
