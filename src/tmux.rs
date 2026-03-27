use std::fs;
use std::process::Command;
use anyhow::{Context, Result};

use crate::config;

pub const YEEHAW_SESSION: &str = "yeehaw";

#[derive(Debug, Clone)]
pub struct TmuxWindow {
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub pane_id: String,
    pub pane_title: String,
    pub pane_current_command: String,
    pub window_activity: u64,
    pub window_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Working,
    Idle,
    Waiting,
    Error,
}

#[derive(Debug, Clone)]
pub struct WindowStatusInfo {
    pub text: String,
    pub status: SessionStatus,
    pub icon: String,
}

pub fn has_tmux() -> bool {
    Command::new("which")
        .arg("tmux")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_inside_yeehaw_session() -> bool {
    if std::env::var("TMUX").is_err() {
        return false;
    }
    Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == YEEHAW_SESSION)
        .unwrap_or(false)
}

pub fn yeehaw_session_exists() -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", YEEHAW_SESSION])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmux_config_path() -> std::path::PathBuf {
    config::yeehaw_dir().join("tmux.conf")
}

fn generate_tmux_config() -> String {
    r##"# Yeehaw tmux configuration
# Auto-generated - do not edit manually

# Scrollback and mouse support
set -g mouse on
set -g history-limit 50000

# macOS clipboard support
# Enable copying to system clipboard when selecting with mouse
set -g set-clipboard on
bind-key -T copy-mode MouseDragEnd1Pane send-keys -X copy-pipe-and-cancel "pbcopy"
bind-key -T copy-mode-vi MouseDragEnd1Pane send-keys -X copy-pipe-and-cancel "pbcopy"
# Also support keyboard-based copy (Enter key in copy mode)
bind-key -T copy-mode Enter send-keys -X copy-pipe-and-cancel "pbcopy"
bind-key -T copy-mode-vi Enter send-keys -X copy-pipe-and-cancel "pbcopy"
# Use y to yank in vi mode
bind-key -T copy-mode-vi y send-keys -X copy-pipe-and-cancel "pbcopy"

# Yeehaw keybindings
bind-key -n C-y select-window -t :0    # Return to dashboard
bind-key -n C-h previous-window        # Go left one window
bind-key -n C-l next-window            # Go right one window
bind-key -n C-p run-shell 'echo "#{pane_id}" > ~/.yeehaw/vault-trigger' \; select-window -t :0  # Password vault

# Status bar styling (Yeehaw brand colors)
set -g status-style "bg=#b8860b,fg=#1a1a1a"
set -g status-left "#[bold] YEEHAW "
set -g status-left-length 20
set -g status-right " C-p: vault  C-y: dashboard "
set -g status-right-length 40

# Window status format
set -g window-status-format " #I:#W "
set -g window-status-current-format "#[bg=#daa520,fg=#1a1a1a,bold] #I:#W "

# Pane border styling
set -g pane-border-style "fg=#b8860b"
set -g pane-active-border-style "fg=#daa520"

# Message styling
set -g message-style "bg=#b8860b,fg=#1a1a1a""##.to_string()
}

fn write_and_source_tmux_config() {
    let path = tmux_config_path();
    let content = generate_tmux_config();
    let _ = fs::write(&path, &content);

    // Source the config into the yeehaw session
    let _ = Command::new("tmux")
        .args(["source-file", &path.to_string_lossy()])
        .output();
}

pub fn create_yeehaw_session() -> Result<()> {
    // Write the tmux config
    config::ensure_config_dirs();
    let config_path = tmux_config_path();
    let _ = fs::write(&config_path, generate_tmux_config());

    // Create the session with window 0 named "yeehaw", running yeehaw directly
    let status = Command::new("tmux")
        .args([
            "new-session", "-d",
            "-s", YEEHAW_SESSION,
            "-n", "yeehaw",
            "yeehaw",
        ])
        .status()
        .context("Failed to create tmux session")?;

    if !status.success() {
        anyhow::bail!("tmux new-session failed");
    }

    // Source the config and set up hooks
    write_and_source_tmux_config();
    setup_status_bar_hooks();
    Ok(())
}

fn setup_status_bar_hooks() {
    let status_check = "if-shell -F \"#{==:#{window_index},0}\" \"set status off\" \"set status on\"";

    // Start with status off
    let _ = Command::new("tmux")
        .args(["set", "-t", YEEHAW_SESSION, "status", "off"])
        .output();

    // Hook for window changes
    let _ = Command::new("tmux")
        .args(["set-hook", "-t", YEEHAW_SESSION, "after-select-window", status_check])
        .output();

    let _ = Command::new("tmux")
        .args(["set-hook", "-t", YEEHAW_SESSION, "window-unlinked", status_check])
        .output();

    let _ = Command::new("tmux")
        .args(["set-hook", "-t", YEEHAW_SESSION, "pane-focus-in", status_check])
        .output();

    let _ = Command::new("tmux")
        .args(["set-hook", "-t", YEEHAW_SESSION, "client-attached", status_check])
        .output();
}

pub fn attach_to_yeehaw() {
    let _ = Command::new("tmux")
        .args(["attach-session", "-t", YEEHAW_SESSION])
        .status();
    std::process::exit(0);
}

pub fn ensure_correct_status_bar() {
    if let Ok(output) = Command::new("tmux")
        .args(["display-message", "-p", "#{window_index}"])
        .output()
    {
        let idx = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if idx == "0" {
            let _ = Command::new("tmux")
                .args(["set", "-t", YEEHAW_SESSION, "status", "off"])
                .output();
        } else {
            let _ = Command::new("tmux")
                .args(["set", "-t", YEEHAW_SESSION, "status", "on"])
                .output();
        }
    }
}

pub fn list_yeehaw_windows() -> Vec<TmuxWindow> {
    let output = Command::new("tmux")
        .args([
            "list-windows", "-t", YEEHAW_SESSION,
            "-F",
            "#{window_index}\t#{window_name}\t#{window_active}\t#{pane_id}\t#{pane_title}\t#{pane_current_command}\t#{window_activity}\t#{@yeehaw_type}",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 7 {
                return None;
            }
            Some(TmuxWindow {
                index: parts[0].parse().unwrap_or(0),
                name: parts[1].to_string(),
                active: parts[2] == "1",
                pane_id: parts[3].to_string(),
                pane_title: parts.get(4).unwrap_or(&"").to_string(),
                pane_current_command: parts.get(5).unwrap_or(&"").to_string(),
                window_activity: parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0),
                window_type: parts.get(7).unwrap_or(&"").to_string(),
            })
        })
        .collect()
}

pub fn switch_to_window(window_index: u32) {
    let target = format!("{}:{}", YEEHAW_SESSION, window_index);
    let _ = Command::new("tmux")
        .args(["select-window", "-t", &target])
        .output();
}

pub fn detach_from_session() {
    let _ = Command::new("tmux")
        .args(["detach-client"])
        .output();
}

pub fn kill_yeehaw_session() {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", YEEHAW_SESSION])
        .output();
}

pub fn restart_yeehaw() {
    let target = format!("{}:0", YEEHAW_SESSION);
    let _ = Command::new("tmux")
        .args(["respawn-window", "-k", "-t", &target, "yeehaw"])
        .output();
}

pub fn create_shell_window(working_dir: &str, window_name: &str) -> Result<u32> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let shell_cmd = format!("{} -l", shell);

    let _ = Command::new("tmux")
        .args([
            "new-window", "-a",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            "-c", working_dir,
            &shell_cmd,
        ])
        .output();

    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{window_index}"])
        .output()
        .context("Failed to get window index")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    set_window_type(idx, "shell");
    Ok(idx)
}

pub fn create_ssh_window(
    window_name: &str,
    host: &str,
    user: &str,
    port: u16,
    identity_file: &str,
    remote_path: &str,
) -> Result<u32> {
    let resolved_path = remote_path.replace("~", "$HOME");
    let remote_cmd = format!("cd {} && exec $SHELL -l", resolved_path);
    let ssh_cmd = format!(
        "ssh -p {} -i {} {}@{} -t '{}'",
        port, shell_escape(identity_file), user, host, remote_cmd
    );

    let _ = Command::new("tmux")
        .args([
            "new-window", "-a",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            &ssh_cmd,
        ])
        .output();

    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{window_index}"])
        .output()
        .context("Failed to get window index")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    set_window_type(idx, "ssh");
    Ok(idx)
}

pub fn create_claude_window(working_dir: &str, window_name: &str) -> Result<u32> {
    let mcp_config = build_mcp_config();
    let allowed_tools = build_allowed_tools();
    let claude_cmd = format!(
        "claude --mcp-config {} --allowedTools {}",
        shell_escape(&mcp_config),
        shell_escape(&allowed_tools),
    );

    let output = Command::new("tmux")
        .args([
            "new-window", "-a", "-d",
            "-P", "-F", "#{window_index}",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            "-c", working_dir,
            &claude_cmd,
        ])
        .output()
        .context("Failed to create claude window")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    set_window_type(idx, "claude");
    Ok(idx)
}

pub fn create_claude_window_with_context(
    working_dir: &str,
    window_name: &str,
    context: &str,
) -> Result<u32> {
    let mcp_config = build_mcp_config();
    let allowed_tools = build_allowed_tools();

    let claude_cmd = if context.is_empty() {
        format!(
            "claude --mcp-config {} --allowedTools {}",
            shell_escape(&mcp_config),
            shell_escape(&allowed_tools),
        )
    } else {
        format!(
            "claude --mcp-config {} --allowedTools {} --system-prompt {}",
            shell_escape(&mcp_config),
            shell_escape(&allowed_tools),
            shell_escape(context),
        )
    };

    let output = Command::new("tmux")
        .args([
            "new-window", "-a", "-d",
            "-P", "-F", "#{window_index}",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            "-c", working_dir,
            &claude_cmd,
        ])
        .output()
        .context("Failed to create claude window")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    set_window_type(idx, "claude");
    Ok(idx)
}

pub fn create_worm_window(
    window_name: &str,
    command: &str,
    working_dir: &str,
) -> Result<u32> {
    let expanded = if working_dir.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(&working_dir[2..]).to_string_lossy().to_string()
        } else {
            working_dir.to_string()
        }
    } else {
        working_dir.to_string()
    };

    let output = Command::new("tmux")
        .args([
            "new-window", "-a", "-d",
            "-P", "-F", "#{window_index}",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            "-c", &expanded,
            command,
        ])
        .output()
        .context("Failed to create worm window")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    set_window_type(idx, "worm");
    Ok(idx)
}

pub fn create_claude_worm_window(
    window_name: &str,
    prompt: &str,
    working_dir: &str,
) -> Result<u32> {
    let mcp_config = build_mcp_config();
    let allowed_tools = build_allowed_tools();
    let claude_cmd = format!(
        "claude --mcp-config {} --allowedTools {} -p {}",
        shell_escape(&mcp_config),
        shell_escape(&allowed_tools),
        shell_escape(prompt),
    );

    let output = Command::new("tmux")
        .args([
            "new-window", "-a", "-d",
            "-P", "-F", "#{window_index}",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            "-c", working_dir,
            &claude_cmd,
        ])
        .output()
        .context("Failed to create claude worm window")?;

    let idx: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    set_window_type(idx, "worm");
    Ok(idx)
}

pub fn kill_window(window_index: u32) {
    let target = format!("{}:{}", YEEHAW_SESSION, window_index);
    let _ = Command::new("tmux")
        .args(["kill-window", "-t", &target])
        .output();
}

pub fn update_status_bar(project_name: Option<&str>) {
    let left = match project_name {
        Some(name) => format!("#[bold] YEEHAW | {} ", name),
        None => "#[bold] YEEHAW ".to_string(),
    };

    let _ = Command::new("tmux")
        .args(["set", "-t", YEEHAW_SESSION, "status-left", &left])
        .output();
}

pub fn set_window_type_pub(window_index: u32, window_type: &str) {
    set_window_type(window_index, window_type);
}

fn set_window_type(window_index: u32, window_type: &str) {
    let target = format!("{}:{}", YEEHAW_SESSION, window_index);
    let _ = Command::new("tmux")
        .args(["set-option", "-w", "-t", &target, "@yeehaw_type", window_type])
        .output();
}

/// All MCP tool names for auto-approval in Claude sessions
pub const YEEHAW_MCP_TOOLS: &[&str] = &[
    // Project management
    "mcp__yeehaw__list_projects",
    "mcp__yeehaw__get_project",
    "mcp__yeehaw__create_project",
    "mcp__yeehaw__update_project",
    "mcp__yeehaw__delete_project",
    // Livestock management
    "mcp__yeehaw__add_livestock",
    "mcp__yeehaw__remove_livestock",
    "mcp__yeehaw__read_livestock_logs",
    "mcp__yeehaw__read_livestock_env",
    // Barn management
    "mcp__yeehaw__list_barns",
    "mcp__yeehaw__get_barn",
    "mcp__yeehaw__create_barn",
    "mcp__yeehaw__update_barn",
    "mcp__yeehaw__delete_barn",
    // Critter management
    "mcp__yeehaw__add_critter",
    "mcp__yeehaw__remove_critter",
    "mcp__yeehaw__read_critter_logs",
    "mcp__yeehaw__discover_critters",
    // Wiki management
    "mcp__yeehaw__get_wiki",
    "mcp__yeehaw__get_wiki_section",
    "mcp__yeehaw__add_wiki_section",
    "mcp__yeehaw__update_wiki_section",
    "mcp__yeehaw__delete_wiki_section",
    // Herd management
    "mcp__yeehaw__list_herds",
    "mcp__yeehaw__get_herd",
    "mcp__yeehaw__create_herd",
    "mcp__yeehaw__delete_herd",
    "mcp__yeehaw__add_livestock_to_herd",
    "mcp__yeehaw__remove_livestock_from_herd",
    "mcp__yeehaw__add_critter_to_herd",
    "mcp__yeehaw__remove_critter_from_herd",
    // Worm management
    "mcp__yeehaw__list_worms",
    "mcp__yeehaw__get_worm",
    "mcp__yeehaw__create_worm",
    "mcp__yeehaw__update_worm",
    "mcp__yeehaw__delete_worm",
    "mcp__yeehaw__toggle_worm",
    "mcp__yeehaw__list_worm_runs",
    "mcp__yeehaw__read_worm_run_log",
    "mcp__yeehaw__run_worm_now",
    // RanchHand management
    "mcp__yeehaw__list_ranchhands",
    "mcp__yeehaw__get_ranchhand",
    "mcp__yeehaw__create_ranchhand",
    "mcp__yeehaw__delete_ranchhand",
    "mcp__yeehaw__discover_ranchhand_resources",
    "mcp__yeehaw__select_ranchhand_herds",
    "mcp__yeehaw__sync_ranchhand",
    "mcp__yeehaw__assign_ranchhand_resource_to_herd",
    "mcp__yeehaw__get_kubectl_contexts",
    "mcp__yeehaw__list_terraform_state_files",
];

fn mcp_server_path() -> String {
    // Prefer `which yeehaw` to find the installed binary.
    // Avoids baking in a cargo target/ path (unsigned dev builds trigger
    // syspolicyd loops on macOS).
    if let Ok(output) = Command::new("which").arg("yeehaw").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return path;
            }
        }
    }
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "yeehaw".to_string())
}

fn build_mcp_config() -> String {
    let server_path = mcp_server_path();
    serde_json::json!({
        "mcpServers": {
            "yeehaw": {
                "command": server_path,
                "args": ["mcp-server"]
            }
        }
    }).to_string()
}

fn build_allowed_tools() -> String {
    YEEHAW_MCP_TOOLS.join(",")
}

pub fn shell_escape(s: &str) -> String {
    if s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\' || c == '$' || c == '`') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

fn format_relative_time(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(timestamp);

    if diff < 60 { return "now".to_string(); }
    if diff < 3600 { return format!("{}m", diff / 60); }
    if diff < 86400 { return format!("{}h", diff / 3600); }
    format!("{}d", diff / 86400)
}

fn is_claude_working(pane_title: &str) -> bool {
    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '⠂', '⠐', '⠈'];
    spinner_chars.iter().any(|&c| pane_title.starts_with(c))
}

pub fn get_window_status(window: &TmuxWindow) -> WindowStatusInfo {
    let is_claude = window.window_type == "claude";
    let relative_time = format_relative_time(window.window_activity);

    // Claude sessions - check signals first
    if is_claude {
        // Try reading signal file for more accurate status
        if let Some(signal) = crate::signals::read_signal(&window.pane_id) {
            let icon = crate::signals::get_status_icon(&signal.status).to_string();
            let status = match signal.status {
                crate::signals::SessionStatus::Working => SessionStatus::Working,
                crate::signals::SessionStatus::Waiting => SessionStatus::Waiting,
                crate::signals::SessionStatus::Idle => SessionStatus::Idle,
                crate::signals::SessionStatus::Error => SessionStatus::Error,
            };
            let text = if !window.pane_title.is_empty() {
                window.pane_title.clone()
            } else {
                match signal.status {
                    crate::signals::SessionStatus::Working => "working".to_string(),
                    crate::signals::SessionStatus::Waiting => "waiting for input".to_string(),
                    crate::signals::SessionStatus::Idle => "idle".to_string(),
                    crate::signals::SessionStatus::Error => "error".to_string(),
                }
            };
            return WindowStatusInfo { text, status, icon };
        }

        // Fallback to heuristic-based detection
        if !window.pane_title.is_empty() {
            if is_claude_working(&window.pane_title) {
                return WindowStatusInfo {
                    text: window.pane_title.clone(),
                    status: SessionStatus::Working,
                    icon: "◐".to_string(),
                };
            }
            let text = if relative_time != "now" && relative_time != "1m" {
                format!("{} ({})", window.pane_title, relative_time)
            } else {
                window.pane_title.clone()
            };
            return WindowStatusInfo {
                text,
                status: SessionStatus::Idle,
                icon: "○".to_string(),
            };
        }
        let text = if relative_time == "now" {
            "active".to_string()
        } else {
            format!("idle {}", relative_time)
        };
        return WindowStatusInfo {
            text: format!("○ {}", text),
            status: SessionStatus::Idle,
            icon: "○".to_string(),
        };
    }

    // Worm sessions
    if window.window_type == "worm" {
        let cmd = &window.pane_current_command;
        if cmd.is_empty() || cmd == "sleep" {
            return WindowStatusInfo {
                text: "completed".to_string(),
                status: SessionStatus::Idle,
                icon: "○".to_string(),
            };
        }
        return WindowStatusInfo {
            text: "running".to_string(),
            status: SessionStatus::Working,
            icon: "◐".to_string(),
        };
    }

    // Dead pane
    if window.pane_current_command.is_empty() {
        return WindowStatusInfo {
            text: "✖ disconnected".to_string(),
            status: SessionStatus::Error,
            icon: "✖".to_string(),
        };
    }

    // Shell with running command
    let cmd = &window.pane_current_command;
    let idle_shells = ["zsh", "bash", "sh", "fish"];
    if !idle_shells.contains(&cmd.as_str()) {
        return WindowStatusInfo {
            text: cmd.clone(),
            status: SessionStatus::Working,
            icon: "◐".to_string(),
        };
    }

    // At shell prompt
    let text = if relative_time == "now" {
        "ready".to_string()
    } else {
        format!("idle {}", relative_time)
    };
    WindowStatusInfo {
        text: format!("○ {}", text),
        status: SessionStatus::Idle,
        icon: "○".to_string(),
    }
}
