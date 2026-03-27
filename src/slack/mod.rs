pub mod listener;
pub mod runner;
pub mod sessions;

use std::sync::mpsc;
use std::thread;

use crate::config;
use crate::issues::auth;

#[derive(Debug, Clone)]
pub struct SlackStatus {
    pub connected: bool,
    pub enabled: bool,
    pub active_runs: usize,
    pub last_error: Option<String>,
}

impl Default for SlackStatus {
    fn default() -> Self {
        Self {
            connected: false,
            enabled: false,
            active_runs: 0,
            last_error: None,
        }
    }
}

#[derive(Debug)]
pub enum SlackEvent {
    Connected,
    Disconnected,
    RunStarted { channel: String, user: String },
    RunCompleted { channel: String, success: bool },
    Error(String),
}

/// Start the Slack listener in a background thread.
/// Returns a receiver for status events, or None if Slack is not configured.
pub fn start_slack_listener() -> Option<mpsc::Receiver<SlackEvent>> {
    let cfg = config::load_config();
    let slack_cfg = match cfg.slack {
        Some(ref s) if s.enabled => s.clone(),
        _ => return None,
    };

    let tokens = match auth::get_slack_tokens() {
        Some(t) => t,
        None => return None,
    };

    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let mut slack_listener = listener::SlackListener::new(
            tokens.bot_token,
            tokens.app_token,
            slack_cfg,
            tx,
        );
        slack_listener.run();
    });

    Some(rx)
}
