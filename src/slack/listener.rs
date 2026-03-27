use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use crate::config;
use crate::types::SlackConfig;

use super::runner;
use super::sessions;
use super::SlackEvent;

const SLACK_API_BASE: &str = "https://slack.com/api";
const POLL_TIMEOUT_SECS: u64 = 600; // 10 minutes
const MAX_MESSAGE_LENGTH: usize = 3900;
const RECONNECT_DELAY_SECS: u64 = 5;

pub struct SlackListener {
    bot_token: String,
    app_token: String,
    config: SlackConfig,
    tx: mpsc::Sender<SlackEvent>,
    active_runs: Arc<AtomicUsize>,
}

impl SlackListener {
    pub fn new(
        bot_token: String,
        app_token: String,
        config: SlackConfig,
        tx: mpsc::Sender<SlackEvent>,
    ) -> Self {
        Self {
            bot_token,
            app_token,
            config,
            tx,
            active_runs: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Main loop: connect, listen, reconnect on failure.
    pub fn run(&mut self) {
        loop {
            match self.connect_and_listen() {
                Ok(()) => {
                    // Clean disconnect (e.g. Slack asked us to reconnect)
                    let _ = self.tx.send(SlackEvent::Disconnected);
                    // Reconnect immediately for "disconnect" events
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                Err(e) => {
                    let _ = self
                        .tx
                        .send(SlackEvent::Error(format!("Slack error: {}", e)));
                    let _ = self.tx.send(SlackEvent::Disconnected);
                    std::thread::sleep(std::time::Duration::from_secs(RECONNECT_DELAY_SECS));
                }
            }
        }
    }

    fn connect_and_listen(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Step 1: Get WebSocket URL via apps.connections.open
        let resp = ureq::post(&format!("{}/apps.connections.open", SLACK_API_BASE))
            .set("Authorization", &format!("Bearer {}", self.app_token))
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string("")?;

        let body: serde_json::Value = resp.into_json()?;

        if !body["ok"].as_bool().unwrap_or(false) {
            let error = body["error"].as_str().unwrap_or("unknown");
            return Err(format!("apps.connections.open failed: {}", error).into());
        }

        let ws_url = body["url"]
            .as_str()
            .ok_or("No URL in apps.connections.open response")?;

        // Step 2: Connect to WebSocket
        let (mut socket, _) = tungstenite::connect(ws_url)?;

        let _ = self.tx.send(SlackEvent::Connected);

        // Step 3: Listen for events
        loop {
            let msg = socket.read()?;

            match msg {
                tungstenite::Message::Text(ref text) => {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(text) {
                        let event_type = event["type"].as_str().unwrap_or("");

                        // Acknowledge envelope
                        if let Some(envelope_id) = event["envelope_id"].as_str() {
                            let ack = serde_json::json!({ "envelope_id": envelope_id });
                            socket
                                .send(tungstenite::Message::Text(ack.to_string().into()))?;
                        }

                        match event_type {
                            "hello" => {
                                // Connected successfully, nothing to do
                            }
                            "events_api" => {
                                self.handle_events_api(&event);
                            }
                            "disconnect" => {
                                // Slack asking us to reconnect
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                }
                tungstenite::Message::Ping(data) => {
                    socket.send(tungstenite::Message::Pong(data))?;
                }
                tungstenite::Message::Close(_) => {
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    fn handle_events_api(&self, envelope: &serde_json::Value) {
        let payload = match envelope.get("payload") {
            Some(p) => p,
            None => return,
        };

        let event = match payload.get("event") {
            Some(e) => e,
            None => return,
        };

        let event_type = event["type"].as_str().unwrap_or("");

        match event_type {
            "message" => {
                // DM - only process if no subtype (ignore edits, deletes, bot messages)
                if event.get("subtype").is_some() {
                    return;
                }
                // Ignore bot messages
                if event.get("bot_id").is_some() {
                    return;
                }
                self.handle_message(event);
            }
            "app_mention" => {
                self.handle_mention(event);
            }
            _ => {}
        }
    }

    fn handle_message(&self, event: &serde_json::Value) {
        let user = match event["user"].as_str() {
            Some(u) => u.to_string(),
            None => return,
        };

        let text = event["text"].as_str().unwrap_or("").to_string();
        let channel = event["channel"].as_str().unwrap_or("").to_string();
        let ts = event["ts"].as_str().unwrap_or("").to_string();
        let thread_ts = event
            .get("thread_ts")
            .and_then(|t| t.as_str())
            .unwrap_or(&ts)
            .to_string();

        if text.is_empty() || channel.is_empty() {
            return;
        }

        self.process_request(user, text, channel, ts, thread_ts);
    }

    fn handle_mention(&self, event: &serde_json::Value) {
        let user = match event["user"].as_str() {
            Some(u) => u.to_string(),
            None => return,
        };

        let raw_text = event["text"].as_str().unwrap_or("").to_string();
        let text = remove_mention(&raw_text);
        let channel = event["channel"].as_str().unwrap_or("").to_string();
        let ts = event["ts"].as_str().unwrap_or("").to_string();
        let thread_ts = event
            .get("thread_ts")
            .and_then(|t| t.as_str())
            .unwrap_or(&ts)
            .to_string();

        if text.is_empty() || channel.is_empty() {
            return;
        }

        self.process_request(user, text, channel, ts, thread_ts);
    }

    fn process_request(
        &self,
        user: String,
        text: String,
        channel: String,
        ts: String,
        thread_ts: String,
    ) {
        // Check authorization
        if !self.config.allowed_users.contains(&user) {
            return;
        }

        // Clone what we need for the worker thread
        let bot_token = self.bot_token.clone();
        let config = self.config.clone();
        let tx = self.tx.clone();
        let active_runs = self.active_runs.clone();

        // Spawn worker thread for this request
        std::thread::spawn(move || {
            active_runs.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(SlackEvent::RunStarted {
                channel: channel.clone(),
                user: user.clone(),
            });

            let result =
                process_slack_request(&bot_token, &config, &text, &channel, &ts, &thread_ts);

            active_runs.fetch_sub(1, Ordering::SeqCst);

            let success = result.is_ok();
            if let Err(e) = result {
                let _ = tx.send(SlackEvent::Error(format!("Run failed: {}", e)));
                // Post error to Slack
                let _ = slack_post_message(
                    &bot_token,
                    &channel,
                    &thread_ts,
                    &format!("Sorry, I encountered an error: {}", e),
                );
            }

            let _ = tx.send(SlackEvent::RunCompleted {
                channel: channel.clone(),
                success,
            });
        });
    }
}

// ============================================================================
// Request Processing
// ============================================================================

fn process_slack_request(
    bot_token: &str,
    config: &SlackConfig,
    text: &str,
    channel: &str,
    ts: &str,
    thread_ts: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Add hourglass reaction
    let _ = slack_add_reaction(bot_token, channel, ts, "hourglass_flowing_sand");
    if ts != thread_ts {
        let _ = slack_add_reaction(bot_token, channel, thread_ts, "hourglass_flowing_sand");
    }

    // Check for existing session (thread continuity)
    let thread_key = format!("{}:{}", channel, thread_ts);
    let existing_session = sessions::get_session(&thread_key);

    // Resolve project and working directory
    let (project_name, working_dir) = resolve_project_and_dir(config, channel);

    // Generate result file path
    let now = chrono::Utc::now();
    let rand_suffix: u32 = (now.timestamp_millis() as u32) % 100000;
    let result_filename = format!("{}-{}.json", now.format("%Y%m%d%H%M%S"), rand_suffix);
    let result_path = config::slack_dir()
        .join("results")
        .join(&result_filename);
    let result_path_str = result_path.to_string_lossy().to_string();

    // Spawn Claude
    let _window_idx = if let Some(ref session) = existing_session {
        runner::run_slack_claude_resume(
            text,
            &session.session_id,
            &working_dir,
            &result_path_str,
            config.system_prompt.as_deref(),
        )?
    } else {
        runner::run_slack_claude(
            text,
            &working_dir,
            &result_path_str,
            config.system_prompt.as_deref(),
        )?
    };

    // Poll for result (blocks up to 10 minutes)
    match runner::poll_for_result(&result_path_str, POLL_TIMEOUT_SECS) {
        Some(result) => {
            // Save session mapping for thread continuity
            if let Some(session_id) = &result.session_id {
                let mapping = sessions::SlackSessionMapping {
                    thread_key: thread_key.clone(),
                    session_id: session_id.clone(),
                    project: project_name.clone(),
                    started_at: existing_session
                        .as_ref()
                        .map(|s| s.started_at.clone())
                        .unwrap_or_else(|| now.to_rfc3339()),
                    last_message_at: now.to_rfc3339(),
                };
                sessions::save_session(&mapping);
            }

            // Post result to Slack (chunked if needed)
            let response_text = if result.is_error {
                format!("Error: {}", result.result)
            } else {
                result.result
            };

            let chunks = chunk_message(&response_text);
            for chunk in &chunks {
                slack_post_message(bot_token, channel, thread_ts, chunk)?;
            }

            // Update reactions: hourglass -> checkmark
            let _ = slack_remove_reaction(bot_token, channel, ts, "hourglass_flowing_sand");
            let _ = slack_add_reaction(bot_token, channel, ts, "white_check_mark");
            if ts != thread_ts {
                let _ =
                    slack_remove_reaction(bot_token, channel, thread_ts, "hourglass_flowing_sand");
                let _ = slack_add_reaction(bot_token, channel, thread_ts, "white_check_mark");
            }
        }
        None => {
            // Timeout
            slack_post_message(
                bot_token,
                channel,
                thread_ts,
                "Sorry, the request timed out after 10 minutes.",
            )?;

            let _ = slack_remove_reaction(bot_token, channel, ts, "hourglass_flowing_sand");
            let _ = slack_add_reaction(bot_token, channel, ts, "warning");
        }
    }

    Ok(())
}

fn resolve_project_and_dir(config: &SlackConfig, channel: &str) -> (Option<String>, String) {
    // Check channel-specific project mapping
    let project_name = config
        .channel_projects
        .as_ref()
        .and_then(|cp| cp.get(channel))
        .cloned()
        .or_else(|| config.default_project.clone());

    if let Some(ref name) = project_name {
        let projects = config::load_projects();
        if let Some(project) = projects.iter().find(|p| p.name == *name) {
            let path = expand_path(&project.path);
            return (project_name, path);
        }
    }

    let home = dirs::home_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    (project_name, home)
}

fn expand_path(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

/// Remove <@U...> mention patterns from text.
fn remove_mention(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' && chars.peek() == Some(&'@') {
            // Skip until >
            for cc in chars.by_ref() {
                if cc == '>' {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result.trim().to_string()
}

/// Chunk a message for Slack's ~4000 character limit.
fn chunk_message(text: &str) -> Vec<String> {
    if text.len() <= MAX_MESSAGE_LENGTH {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= MAX_MESSAGE_LENGTH {
            chunks.push(remaining.to_string());
            break;
        }

        // Try to split at newline, then space, then hard split
        let chunk_end = &remaining[..MAX_MESSAGE_LENGTH];
        let split_pos = chunk_end
            .rfind('\n')
            .or_else(|| chunk_end.rfind(' '))
            .unwrap_or(MAX_MESSAGE_LENGTH);

        chunks.push(remaining[..split_pos].to_string());
        remaining = remaining[split_pos..].trim_start();
    }

    chunks
}

// ============================================================================
// Slack API Helpers
// ============================================================================

fn slack_post_message(
    token: &str,
    channel: &str,
    thread_ts: &str,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ureq::post(&format!("{}/chat.postMessage", SLACK_API_BASE))
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(serde_json::json!({
            "channel": channel,
            "thread_ts": thread_ts,
            "text": text,
        }))?;
    Ok(())
}

fn slack_add_reaction(
    token: &str,
    channel: &str,
    timestamp: &str,
    name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = ureq::post(&format!("{}/reactions.add", SLACK_API_BASE))
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(serde_json::json!({
            "channel": channel,
            "timestamp": timestamp,
            "name": name,
        }));
    Ok(())
}

fn slack_remove_reaction(
    token: &str,
    channel: &str,
    timestamp: &str,
    name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = ureq::post(&format!("{}/reactions.remove", SLACK_API_BASE))
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(serde_json::json!({
            "channel": channel,
            "timestamp": timestamp,
            "name": name,
        }));
    Ok(())
}
