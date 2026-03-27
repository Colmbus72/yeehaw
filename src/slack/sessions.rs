use std::collections::HashMap;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSessionMapping {
    pub thread_key: String,
    pub session_id: String,
    pub project: Option<String>,
    pub started_at: String,
    pub last_message_at: String,
}

fn sessions_file() -> std::path::PathBuf {
    config::slack_dir().join("sessions.json")
}

pub fn load_sessions() -> HashMap<String, SlackSessionMapping> {
    let path = sessions_file();
    if !path.exists() {
        return HashMap::new();
    }
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

pub fn get_session(thread_key: &str) -> Option<SlackSessionMapping> {
    let sessions = load_sessions();
    sessions.get(thread_key).cloned()
}

pub fn save_session(mapping: &SlackSessionMapping) {
    let mut sessions = load_sessions();
    sessions.insert(mapping.thread_key.clone(), mapping.clone());
    let dir = config::slack_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(content) = serde_json::to_string_pretty(&sessions) {
        let _ = fs::write(sessions_file(), content);
    }
}
