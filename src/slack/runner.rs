use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

use crate::config;
use crate::tmux;

// Use the shared tools list from tmux.rs as the single source of truth

fn results_dir() -> PathBuf {
    config::slack_dir().join("results")
}

fn mcp_config_json() -> String {
    // Prefer `which yeehaw` to find the installed binary.
    // Avoids baking in a cargo target/ path (unsigned dev builds trigger
    // syspolicyd loops on macOS).
    let exe_str = std::process::Command::new("which")
        .arg("yeehaw")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !p.is_empty() { Some(p) } else { None }
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap_or_else(|_| PathBuf::from("yeehaw"))
                .to_string_lossy()
                .to_string()
        });

    serde_json::json!({
        "mcpServers": {
            "yeehaw": {
                "command": exe_str,
                "args": ["mcp-server"]
            }
        }
    })
    .to_string()
}

fn tools_list() -> String {
    tmux::YEEHAW_MCP_TOOLS.join(",")
}

/// Spawn a new Claude session in tmux for a Slack request.
/// Returns the tmux window index.
pub fn run_slack_claude(
    prompt: &str,
    working_dir: &str,
    result_path: &str,
    system_prompt: Option<&str>,
) -> Result<u32> {
    let _ = fs::create_dir_all(results_dir());

    let mcp_config = mcp_config_json();
    let tools = tools_list();

    let mut cmd = format!(
        "claude --output-format json --mcp-config {} --allowedTools {} -p {}",
        tmux::shell_escape(&mcp_config),
        tmux::shell_escape(&tools),
        tmux::shell_escape(prompt),
    );

    if let Some(sp) = system_prompt {
        cmd = format!("{} --system-prompt {}", cmd, tmux::shell_escape(sp));
    }

    // Redirect output to result file
    cmd = format!("{} > {}", cmd, tmux::shell_escape(result_path));

    let window_name = format!("slack:{}", chrono::Utc::now().format("%H%M%S"));

    let output = Command::new("tmux")
        .args([
            "new-window",
            "-a",
            "-d",
            "-P",
            "-F",
            "#{window_index}",
            "-t",
            tmux::YEEHAW_SESSION,
            "-n",
            &window_name,
            "-c",
            working_dir,
            &cmd,
        ])
        .output()
        .context("Failed to create slack claude window")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    tmux::set_window_type_pub(idx, "slack");
    Ok(idx)
}

/// Resume an existing Claude session for a Slack thread reply.
/// Returns the tmux window index.
pub fn run_slack_claude_resume(
    prompt: &str,
    session_id: &str,
    working_dir: &str,
    result_path: &str,
    system_prompt: Option<&str>,
) -> Result<u32> {
    let _ = fs::create_dir_all(results_dir());

    let mcp_config = mcp_config_json();
    let tools = tools_list();

    let mut cmd = format!(
        "claude --output-format json --mcp-config {} --allowedTools {} --resume {} -p {}",
        tmux::shell_escape(&mcp_config),
        tmux::shell_escape(&tools),
        tmux::shell_escape(session_id),
        tmux::shell_escape(prompt),
    );

    if let Some(sp) = system_prompt {
        cmd = format!("{} --system-prompt {}", cmd, tmux::shell_escape(sp));
    }

    cmd = format!("{} > {}", cmd, tmux::shell_escape(result_path));

    let window_name = format!("slack:{}", chrono::Utc::now().format("%H%M%S"));

    let output = Command::new("tmux")
        .args([
            "new-window",
            "-a",
            "-d",
            "-P",
            "-F",
            "#{window_index}",
            "-t",
            tmux::YEEHAW_SESSION,
            "-n",
            &window_name,
            "-c",
            working_dir,
            &cmd,
        ])
        .output()
        .context("Failed to create slack claude resume window")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    tmux::set_window_type_pub(idx, "slack");
    Ok(idx)
}

#[derive(Debug)]
pub struct ClaudeResult {
    pub result: String,
    pub session_id: Option<String>,
    pub is_error: bool,
}

/// Poll for the Claude result file, blocking until available or timeout.
pub fn poll_for_result(result_path: &str, timeout_secs: u64) -> Option<ClaudeResult> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let poll_interval = std::time::Duration::from_secs(2);

    loop {
        if start.elapsed() > timeout {
            return None;
        }

        if let Ok(content) = fs::read_to_string(result_path) {
            if !content.is_empty() {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    let result_text = val
                        .get("result")
                        .and_then(|r| r.as_str())
                        .or_else(|| val.get("error").and_then(|e| e.as_str()))
                        .unwrap_or("No response")
                        .to_string();

                    let session_id = val
                        .get("session_id")
                        .and_then(|s| s.as_str())
                        .map(String::from);

                    let is_error = val
                        .get("is_error")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false)
                        || val.get("error").is_some();

                    // Clean up result file
                    let _ = fs::remove_file(result_path);

                    return Some(ClaudeResult {
                        result: result_text,
                        session_id,
                        is_error,
                    });
                }
            }
        }

        std::thread::sleep(poll_interval);
    }
}
