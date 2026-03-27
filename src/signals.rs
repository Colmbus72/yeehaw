use std::fs;
use std::path::PathBuf;

use crate::config;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Working,
    Waiting,
    Idle,
    Error,
}

#[derive(Debug, Clone)]
pub struct SessionSignal {
    pub status: SessionStatus,
    pub updated: u64,
}

const SIGNAL_MAX_AGE_SECS: u64 = 5 * 60; // 5 minutes
const STALE_AGE_SECS: u64 = 60 * 60; // 1 hour

fn sanitize_pane_id(pane_id: &str) -> String {
    pane_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn signal_file(pane_id: &str) -> PathBuf {
    config::signals_dir().join(format!("{}.json", sanitize_pane_id(pane_id)))
}

/// Read signal file for a tmux pane
pub fn read_signal(pane_id: &str) -> Option<SessionSignal> {
    let dir = config::signals_dir();
    if !dir.exists() {
        return None;
    }

    let filepath = signal_file(pane_id);
    if !filepath.exists() {
        return None;
    }

    let content = fs::read_to_string(&filepath).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let status_str = json["status"].as_str()?;
    let updated = json["updated"].as_u64()?;

    let status = match status_str {
        "working" => SessionStatus::Working,
        "waiting" => SessionStatus::Waiting,
        "idle" => SessionStatus::Idle,
        "error" => SessionStatus::Error,
        _ => return None,
    };

    // Check if signal is stale
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if now.saturating_sub(updated) > SIGNAL_MAX_AGE_SECS {
        return None;
    }

    Some(SessionSignal { status, updated })
}

/// Get status icon for a session status
pub fn get_status_icon(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Working => "⠿",
        SessionStatus::Waiting => "◆",
        SessionStatus::Idle => "○",
        SessionStatus::Error => "✖",
    }
}

/// Ensure the signals directory exists
pub fn ensure_signals_dir() {
    let dir = config::signals_dir();
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
}

/// Clean up signal file for a pane
pub fn cleanup_signal(pane_id: &str) {
    let filepath = signal_file(pane_id);
    let _ = fs::remove_file(&filepath);
}

/// Clean up all stale signal files (older than 1 hour)
pub fn cleanup_stale_signals() {
    let dir = config::signals_dir();
    if !dir.exists() {
        return;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let should_delete = match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(json) => match json["updated"].as_u64() {
                        Some(updated) => now.saturating_sub(updated) > STALE_AGE_SECS,
                        None => true, // malformed
                    },
                    Err(_) => true, // malformed
                },
                Err(_) => true,
            };
            if should_delete {
                let _ = fs::remove_file(&path);
            }
        }
    }
}
