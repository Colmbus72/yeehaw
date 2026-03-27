use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::config;

// Embedded skill file
const SKILL_BYTES: &[u8] = include_bytes!("../skills/yeehaw-project-setup.skill");

const HOOK_SCRIPT_NAME: &str = "claude-hook";

const HOOK_SCRIPT_CONTENT: &str = r#"#!/bin/bash
# Yeehaw Claude Hook - writes session status for the CLI to read
STATUS="$1"
PANE_ID="${TMUX_PANE:-unknown}"
SIGNAL_DIR="$HOME/.yeehaw/session-signals"
SIGNAL_FILE="$SIGNAL_DIR/${PANE_ID//[^a-zA-Z0-9]/_}.json"

mkdir -p "$SIGNAL_DIR"
cat > "$SIGNAL_FILE" << EOF
{"status":"$STATUS","updated":$(date +%s)}
EOF
"#;

pub fn hooks_dir() -> PathBuf {
    config::yeehaw_dir().join("bin")
}

pub fn hook_script_path() -> PathBuf {
    hooks_dir().join(HOOK_SCRIPT_NAME)
}

/// Install the Claude hook script to ~/.yeehaw/bin/
pub fn install_hook_script() -> anyhow::Result<PathBuf> {
    let dir = hooks_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }

    let signals_dir = config::signals_dir();
    if !signals_dir.exists() {
        fs::create_dir_all(&signals_dir)?;
    }

    let path = hook_script_path();
    fs::write(&path, HOOK_SCRIPT_CONTENT)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;

    Ok(path)
}

/// Check if hook script exists
pub fn hook_script_exists() -> bool {
    hook_script_path().exists()
}

/// Get the Claude settings.json hooks configuration as JSON
pub fn get_claude_hooks_config() -> serde_json::Value {
    let path = hook_script_path().to_string_lossy().to_string();

    serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [format!("{} working", path)],
            }],
            "Stop": [{
                "matcher": "*",
                "hooks": [format!("{} waiting", path)],
            }],
            "Notification": [{
                "matcher": "idle_prompt",
                "hooks": [format!("{} waiting", path)],
            }],
        }
    })
}

/// Install the bundled yeehaw-project-setup skill to ~/.yeehaw/skills/
pub fn install_skill() -> anyhow::Result<PathBuf> {
    let skills_dir = config::yeehaw_dir().join("skills");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir)?;
    }

    let path = skills_dir.join("yeehaw-project-setup.skill");
    fs::write(&path, SKILL_BYTES)?;
    Ok(path)
}

/// Check if the skill file exists
pub fn skill_installed() -> bool {
    config::yeehaw_dir().join("skills").join("yeehaw-project-setup.skill").exists()
}

/// Check if Claude hooks are already configured in ~/.claude/settings.json
pub fn check_claude_hooks_installed() -> bool {
    let claude_settings = dirs::home_dir()
        .map(|h| h.join(".claude").join("settings.json"))
        .unwrap_or_default();

    if !claude_settings.exists() {
        return false;
    }

    let content = match fs::read_to_string(&claude_settings) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    settings["hooks"]["PreToolUse"]
        .as_array()
        .map(|arr| {
            arr.iter().any(|h| {
                h["hooks"].as_array().map(|hooks| {
                    hooks.iter().any(|cmd| {
                        cmd.as_str().map(|s| s.contains("yeehaw")).unwrap_or(false)
                    })
                }).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}
