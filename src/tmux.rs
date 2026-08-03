use std::collections::HashSet;
use std::fs;
use std::process::Command;
use anyhow::{Context, Result};

use crate::config;
use crate::types::Barn;

pub const YEEHAW_SESSION: &str = "yeehaw";

/// Prefix every connected-barn session name carries. Emitted by
/// [`barn_session_name`] and matched by [`connected_barn_sessions`] — the two
/// must agree or the connected indicator silently reports nothing.
pub const BARN_SESSION_PREFIX: &str = "yh-barn-";

/// tmux key table a barn session runs in. Must match the table named in the
/// `bind-key -T` line of [`generate_tmux_config`], which cannot interpolate
/// (it is a raw string literal containing `#{...}` tmux formats).
pub const REMOTE_KEY_TABLE: &str = "yeehaw-remote";

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
    /// Project this window was spawned from (`@yeehaw_project`). Empty if untagged.
    pub project: String,
    /// Barn this window runs on (`@yeehaw_barn`). Empty if untagged.
    pub barn: String,
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

# Barn connect: inside a barn session the key-table is 'yeehaw-remote', which
# holds exactly one binding, so every root binding is out of the way and all
# other keys reach the remote yeehaw untouched.
#
# The key table does NOT disable the prefix. tmux's
# server_client_is_default_key_table() compares the current table against the
# *session's* key-table option, so with both set to 'yeehaw-remote' it returns
# true and the prefix check still runs — C-b c would create a local window
# inside the barn session. connect_to_barn() therefore also sets
# 'prefix None' on the session, which is what actually delivers full
# pass-through. Both are per-session; neither touches the local ranch.
#
# C-q chosen over C-g (vault generate, vault_view.rs:442), C-o (nvim
# jump-back), C-\ (SIGQUIT), C-^ (nvim alternate file) — all collide with
# something that runs on a barn. C-q is historically XON/XOFF flow control,
# but tmux and every TUI run the tty in raw mode with IXON off.
#
# '=yeehaw', not 'yeehaw': a bare -t target is a prefix pattern (rule 3), so
# with no 'yeehaw' session and a 'yeehaw-scratch' present this would land the
# user in the scratch session instead of the local ranch.
bind-key -T yeehaw-remote C-q switch-client -t =yeehaw

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

/// Regenerate `~/.yeehaw/tmux.conf` and source it into the running tmux server.
///
/// Safe to call repeatedly. Everything in the generated config is a `set -g` or
/// a `bind-key`, both of which are last-write-wins: re-sourcing overwrites the
/// same globals with the same values and rebinds the same keys, so `list-keys
/// -T yeehaw-remote` still reports exactly one `C-q`. Session-local options —
/// the `status off` on the yeehaw session (`setup_status_bar_hooks`) and the red
/// bar plus `key-table`/`prefix` on a barn session (`connect_to_barn`) — are not
/// touched, because a session-local value shadows the global one.
///
/// Returns tmux's own stderr on failure. `connect_to_barn` must not create a
/// barn session unless this succeeded — see [`ensure_remote_key_table`].
fn write_and_source_tmux_config() -> Result<()> {
    let path = tmux_config_path();
    let content = generate_tmux_config();
    fs::write(&path, &content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    let output = Command::new("tmux")
        .args(["source-file", &path.to_string_lossy()])
        .output()
        .context("failed to run tmux source-file")?;
    check_tmux_ok(&output, "source the yeehaw tmux config")
}

/// Best-effort re-source of the generated config, for callers that want the
/// running server brought up to date but must not fail if it cannot be.
///
/// Used at TUI startup so an in-place upgrade picks up new bindings instead of
/// running on whatever the previously installed version bound. Callers that
/// depend on a specific binding existing want [`ensure_remote_key_table`].
pub fn refresh_tmux_config() {
    let _ = write_and_source_tmux_config();
}

/// Guarantee the `yeehaw-remote` key table exists on the running tmux server.
///
/// tmux key tables live in the **server's memory**, not in the config file, and
/// the config is only sourced when the yeehaw session is first created
/// (`create_yeehaw_session`, reached from `main.rs` only when the session does
/// not already exist). Upgrading yeehaw in place therefore leaves a server that
/// has never seen the `bind-key -T yeehaw-remote` line: `list-keys -T
/// yeehaw-remote` reports "table yeehaw-remote doesn't exist" and C-q is dead.
///
/// Because a barn session also sets `prefix None`, a missing table is not a dead
/// key but a session with no way out at all. So this re-sources the config and
/// then *verifies* the table is really there, and every failure is fatal to the
/// caller.
pub fn ensure_remote_key_table() -> Result<()> {
    write_and_source_tmux_config()?;

    // `list-keys -T <table>` exits non-zero for a table that does not exist.
    // Checking is cheap (~1ms, no network) and turns "we hope sourcing worked"
    // into "the escape hatch is bound right now".
    let output = Command::new("tmux")
        .args(["list-keys", "-T", REMOTE_KEY_TABLE])
        .output()
        .context("failed to run tmux list-keys")?;
    check_tmux_ok(
        &output,
        &format!("confirm the '{}' key table exists", REMOTE_KEY_TABLE),
    )
}

/// Turn a finished tmux command into a `Result`, the way
/// [`parse_new_window_index`] does.
///
/// `Command::status()`/`output()` return `Err` only when the process cannot be
/// **spawned**; a tmux that ran and exited 1 is `Ok`. Every call that matters
/// has to look at `status.success()` itself or the failure vanishes.
pub(crate) fn check_tmux_ok(output: &std::process::Output, what: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        anyhow::bail!("tmux failed to {}", what);
    }
    anyhow::bail!("tmux failed to {}: {}", what, stderr)
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

    // Source the config and set up hooks. Best-effort here: the session already
    // exists at this point, and a ranch with unstyled keybindings is better than
    // a startup that bails. `connect_to_barn` re-sources and *does* check, so the
    // one binding that must exist before it can strand a user always does.
    let _ = write_and_source_tmux_config();
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

const WINDOW_LIST_FORMAT: &str = "#{window_index}\t#{window_name}\t#{window_active}\t#{pane_id}\t#{pane_title}\t#{pane_current_command}\t#{window_activity}\t#{@yeehaw_type}\t#{@yeehaw_project}\t#{@yeehaw_barn}";

/// Parse one `list-windows -F WINDOW_LIST_FORMAT` line.
///
/// Trailing user-option fields are empty strings when the option is unset, so
/// windows created before tagging existed degrade to empty tags rather than
/// failing to parse.
fn parse_window_line(line: &str) -> Option<TmuxWindow> {
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
        project: parts.get(8).unwrap_or(&"").to_string(),
        barn: parts.get(9).unwrap_or(&"").to_string(),
    })
}

pub fn list_yeehaw_windows() -> Vec<TmuxWindow> {
    let output = Command::new("tmux")
        .args([
            "list-windows", "-t", YEEHAW_SESSION,
            "-F", WINDOW_LIST_FORMAT,
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
        .filter_map(parse_window_line)
        .collect()
}

/// Sentinel printed between panes when capturing several in one tmux call.
/// Uses control characters that cannot appear in rendered pane output.
const CAPTURE_SENTINEL: &str = "\u{1}\u{2}YHGRID\u{2}\u{1}";

/// Split the concatenated stdout of a batched capture back into per-pane screens.
fn split_captures(raw: &str, count: usize) -> Vec<Vec<String>> {
    let mut panes: Vec<Vec<String>> = raw
        .split(CAPTURE_SENTINEL)
        .map(|chunk| {
            chunk
                .trim_matches('\n')
                .lines()
                .map(|l| l.to_string())
                .collect()
        })
        .collect();
    // A pane that produced no output still owes us a slot.
    panes.resize(count, Vec::new());
    panes.truncate(count);
    panes
}

/// Capture the visible screen of several panes in a single tmux invocation.
///
/// Read-only: this does not attach a client, so it cannot resize or otherwise
/// disturb the panes being watched. One process spawn regardless of pane count.
/// Returns one entry per requested pane, in order.
pub fn capture_panes(pane_ids: &[String]) -> Vec<Vec<String>> {
    if pane_ids.is_empty() {
        return vec![];
    }

    let mut args: Vec<String> = Vec::with_capacity(pane_ids.len() * 8);
    for (i, pane) in pane_ids.iter().enumerate() {
        if i > 0 {
            args.push(";".to_string());
            args.push("display-message".to_string());
            args.push("-p".to_string());
            args.push(CAPTURE_SENTINEL.to_string());
            args.push(";".to_string());
        }
        // -e keeps SGR colour. capture-pane emits a rendered screen, so SGR is
        // the only escape class present; the grid decodes it before truncating.
        args.push("capture-pane".to_string());
        args.push("-p".to_string());
        args.push("-e".to_string());
        args.push("-t".to_string());
        args.push(pane.clone());
    }

    let output = match Command::new("tmux").args(&args).output() {
        Ok(o) if o.status.success() => o,
        _ => return vec![Vec::new(); pane_ids.len()],
    };

    split_captures(&String::from_utf8_lossy(&output.stdout), pane_ids.len())
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
    // Exact target. A bare "yeehaw" falls through to tmux's rule-3 prefix match
    // if the session is already gone, which would kill a user session named
    // "yeehaw-anything" instead. Safe today only because rule 2 (exact) is tried
    // first and the session normally exists — not a guarantee worth relying on.
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &exact_target(YEEHAW_SESSION)])
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

    // `-P -F '#{window_index}'` returns the new window's index directly. A
    // separate `display-message` read races the client's active window.
    let output = Command::new("tmux")
        .args([
            "new-window", "-a",
            "-P", "-F", "#{window_index}",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            "-c", working_dir,
            &shell_cmd,
        ])
        .output()
        .context("Failed to create shell window")?;

    let idx = parse_new_window_index(&output, "shell")?;

    set_window_type(idx, "shell");
    Ok(idx)
}

/// Read the index `new-window -P -F '#{window_index}'` printed.
///
/// Never falls back to 0. Window 0 is the yeehaw dashboard, so a failed
/// `new-window` that silently produced 0 would make the caller retag and jump
/// to the dashboard as though the requested window had opened.
pub(crate) fn parse_new_window_index(output: &std::process::Output, kind: &str) -> Result<u32> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            anyhow::bail!("tmux new-window failed for {} window", kind);
        }
        anyhow::bail!("tmux new-window failed for {} window: {}", kind, stderr);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim().parse::<u32>().map_err(|_| {
        anyhow::anyhow!(
            "tmux did not report a window index for {} window (got {:?})",
            kind,
            stdout.trim()
        )
    })
}

pub fn create_ssh_window(window_name: &str, barn: &Barn, remote_path: &str) -> Result<u32> {
    let remote_cmd = format!("cd {} && exec $SHELL -l", shell_escape(remote_path));

    // tmux runs this string through a shell, so every argv element is quoted
    // exactly once by `shell_escape`, which quotes unconditionally. A barn whose
    // host, user, or identity file contains `;`, `|`, `&`, or a newline is
    // therefore inert here — those values come from MCP callers, k8s node
    // addresses, and terraform state, none of which are trusted input.
    let mut parts = vec!["ssh".to_string()];
    parts.extend(crate::ssh::ssh_args(barn, crate::ssh::Opts { tty: true, ..Default::default() })?);
    parts.push(remote_cmd);
    let ssh_cmd = parts.iter().map(|p| shell_escape(p)).collect::<Vec<_>>().join(" ");

    crate::ssh::ensure_control_dir();

    let output = Command::new("tmux")
        .args([
            "new-window", "-a",
            "-P", "-F", "#{window_index}",
            "-t", YEEHAW_SESSION,
            "-n", window_name,
            &ssh_cmd,
        ])
        .output()
        .context("Failed to create ssh window")?;

    let idx = parse_new_window_index(&output, "ssh")?;

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

    let idx = parse_new_window_index(&output, "claude")?;

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

    let idx = parse_new_window_index(&output, "claude")?;

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

    let idx = parse_new_window_index(&output, "worm")?;

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

    let idx = parse_new_window_index(&output, "claude worm")?;

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
    set_window_option(window_index, "@yeehaw_type", window_type);
}

/// Tag a window with the project and barn it belongs to, so the session grid can
/// filter by scope without resorting to window-name prefix matching.
///
/// Pass an empty barn to default to the local barn.
pub fn set_window_scope(window_index: u32, project: &str, barn: Option<&str>) {
    if !project.is_empty() {
        set_window_option(window_index, "@yeehaw_project", project);
    }
    let barn = barn.filter(|b| !b.is_empty()).unwrap_or(config::LOCAL_BARN_NAME);
    set_window_option(window_index, "@yeehaw_barn", barn);
}

fn set_window_option(window_index: u32, option: &str, value: &str) {
    let target = format!("{}:{}", YEEHAW_SESSION, window_index);
    let _ = Command::new("tmux")
        .args(["set-option", "-w", "-t", &target, option, value])
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

/// Path to *this* running yeehaw binary, for spawning a child that must be the
/// same build as the parent.
///
/// Deliberately the opposite preference to [`mcp_server_path`], and the two must
/// not be merged. `mcp_server_path` prefers `which yeehaw` because its result is
/// baked into an MCP config that Claude re-spawns for the life of a session, and
/// on macOS a `target/debug` path there means an unsigned binary that sends
/// syspolicyd into a re-verification loop on every spawn.
///
/// Here the child is `yeehaw connect <barn>`, a subcommand that only exists in
/// builds new enough to have it. Resolving through PATH means a dev build spawns
/// whatever release happens to be installed — an older `~/.local/bin/yeehaw`
/// does not know `connect`, falls through to launching the TUI, and the barn
/// session shows a second local dashboard instead of the remote ranch. Same
/// build as the parent is the only answer that is always right.
fn current_exe_path() -> String {
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

/// tmux session **name** for a connected barn. Use with `new-session -s`.
///
/// For any `-t` target use [`barn_session_target`] instead — a bare target is a
/// prefix pattern to tmux, not a name.
///
/// Barn names are free-form and currently include spaces and capitals
/// (`camera pi`, `BIG UPS`), so the readable part is reduced to `[a-z0-9-]`.
/// That reduction is lossy and many-to-one — `camera pi` and `camera-pi` share a
/// slug — so an FNV-1a hash of the *original* name is always appended. Two barns
/// therefore never share a session, which would otherwise mean connecting to one
/// and landing on the other's host.
pub fn barn_session_name(barn_name: &str) -> String {
    let mut slug = String::with_capacity(barn_name.len());
    for c in barn_name.chars() {
        let c = if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' };
        if c == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(c);
    }
    let slug = slug.trim_matches('-');

    // FNV-1a over the original name, so names differing only in separators,
    // case, or non-ASCII characters still get distinct sessions.
    let h: u32 = barn_name
        .bytes()
        .fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));

    // `x` keeps the name well-formed when the slug is empty (a name that is
    // entirely punctuation or non-ASCII).
    let base = if slug.is_empty() { "x" } else { slug };
    format!("{}{}-{:08x}", BARN_SESSION_PREFIX, base, h)
}

/// tmux **target** for a connected barn's session, for every `-t` use.
///
/// The `=` prefix forces exact matching. Without it tmux falls through to rule 3
/// of target resolution — "the start of a session name" — so `-t yh-barn-guided`
/// resolves to `yh-barn-guided-2` when the former does not exist. That turns
/// connect into a wrong-host attach and disconnect into a wrong-session kill.
pub fn barn_session_target(barn_name: &str) -> String {
    exact_target(&barn_session_name(barn_name))
}

/// Turn a session **name** into an exact `-t` target.
///
/// Same `=` rule as [`barn_session_target`], for the case where the name is
/// already known — a name read back out of `list-sessions`, which cannot be fed
/// to `barn_session_target` because that hashes a *barn* name and the session
/// name is not invertible back into one.
pub fn exact_target(session_name: &str) -> String {
    format!("={}", session_name)
}

/// Filter a list of tmux session names down to connected barn sessions.
pub fn connected_barn_sessions(names: &[String]) -> HashSet<String> {
    names
        .iter()
        .filter(|n| n.starts_with(BARN_SESSION_PREFIX))
        .cloned()
        .collect()
}

/// Every live local tmux session name. Local call, ~1ms, no network.
pub fn list_session_names() -> Vec<String> {
    Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Connect to a barn: spawn the session if needed, style it, and switch to it.
/// Idempotent — calling twice yields one session and an instant switch.
///
/// Every `-t` below is [`barn_session_target`] (or `target` plus a `:` for the
/// option calls, whose `-t` is a target-pane), never the bare `session`. A bare
/// target is a prefix pattern to tmux, so `-t yh-barn-guided` attaches to (or
/// kills) `yh-barn-guided-2` — a different production host.
pub fn connect_to_barn(barn: &Barn) -> Result<()> {
    let session = barn_session_name(&barn.name); // bare: for `new-session -s`
    let target = barn_session_target(&barn.name); // '='-prefixed: for every `-t`

    // Before anything can strand a user: the `yeehaw-remote` table holds the only
    // key that leaves a barn session, it lives in the tmux server's memory rather
    // than on disk, and the config that defines it is otherwise sourced only when
    // the yeehaw session is first created. Upgrading in place leaves a server
    // without it. Fatal, and first — a barn session sets `prefix None`, so
    // entering one with no table bound is a session with no way out.
    ensure_remote_key_table()?;

    let exists = Command::new("tmux")
        .args(["has-session", "-t", &target])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !exists {
        // `current_exe_path`, not `mcp_server_path`: the child must be this same
        // build. `which yeehaw` can resolve to an installed release with no
        // `connect` subcommand, which falls through to launching the TUI.
        let exe = current_exe_path();

        // `single_quote`, not `shell_escape`: a barn *name* is an argument, not a
        // path, so a name of `~` or `~/thing` must stay those literal bytes rather
        // than expanding to the home directory and connecting to some other barn.
        let runner = format!("{} connect {}", shell_escape(&exe), single_quote(&barn.name));

        // `.output()`, not `.status()`. `status()` returns Err only when tmux
        // cannot be *spawned*; a tmux that ran and exited 1 is Ok, so the old code
        // fell through a failed new-session, fell through a failed switch-client,
        // and returned Ok(()) — the caller set no error and `c` was a silent
        // no-op. Same shape as `parse_new_window_index`.
        let output = Command::new("tmux")
            .args(["new-session", "-d", "-s", &session, &runner])
            .output()
            .context("failed to run tmux new-session")?;
        check_tmux_ok(&output, "create the barn session")?;

        // `set-option -t` takes a *target-pane*, not a target-session, so the
        // session has to be an explicit `session:` component — `-t =yh-barn-x`
        // errors with "no such session: =yh-barn-x" and every option below would
        // be dropped, leaving the session in the root key table with no C-q home.
        // The trailing colon means "current window of that session". The `=` still
        // forces exact matching in this form (verified on tmux 3.6a:
        // `-t =yh-barn-guided:` errors rather than resolving to yh-barn-guided-2,
        // while bare `-t yh-barn-guided` hits it).
        let opt_target = format!("{}:", target);

        for opt in barn_session_options(barn) {
            let result = Command::new("tmux")
                .args(["set", "-t", &opt_target, opt.name, &opt.value])
                .output()
                .with_context(|| format!("failed to run tmux set {}", opt.name))
                .and_then(|o| check_tmux_ok(&o, &format!("set {} on the barn session", opt.name)));

            if let Err(e) = result {
                if !opt.required {
                    // Cosmetics. A missing red bar is a worse-looking session, not
                    // an unsafe one, and is not worth refusing the connection over.
                    continue;
                }
                // The session is unsafe to enter and, worse, would be *reused* on
                // the next `c` — `has-session` would succeed, this whole block
                // would be skipped, and switch-client would drop the user into a
                // session with no key table and possibly no prefix. Tear it down
                // so the next attempt builds it again from scratch.
                disconnect_barn(&barn.name);
                // Flattened with `{:#}` rather than `.context()`: the caller
                // renders this with Display, which shows only the outermost layer
                // of a context chain — and the layer worth reading is tmux's own
                // stderr underneath.
                anyhow::bail!(
                    "refusing to connect to '{}' — the barn session could not be isolated: {:#}",
                    barn.name,
                    e
                );
            }
        }
    }

    let output = Command::new("tmux")
        .args(["switch-client", "-t", &target])
        .output()
        .context("failed to run tmux switch-client")?;
    check_tmux_ok(&output, "switch to the barn session")
}

/// A single `set -t` applied to a freshly created barn session.
pub(crate) struct BarnSessionOption {
    pub name: &'static str,
    pub value: String,
    /// Whether failing to apply this makes the session unsafe to enter.
    pub required: bool,
}

/// Every session option a barn session needs, in the order they must be applied.
///
/// Split into two classes:
///
/// - **Required** — `key-table` and `prefix`. Together they are the entire
///   isolation contract: `key-table` moves the session off `root` so the local
///   C-y/C-h/C-l/C-p do not eat the remote yeehaw's own navigation, and `prefix
///   None` stops C-b from acting on the *local* server (C-b c creating a local
///   window, C-b d detaching the client, C-b & killing the barn session). A
///   session missing either is not a cosmetic problem, it is a session that lies
///   about where the user's keystrokes are going.
/// - **Best-effort** — the red status bar and its lengths. If they fail the
///   session still behaves correctly; it just looks like the local ranch.
///
/// Order matters as much as the classification. `key-table` is applied *before*
/// `prefix`, so that a failure at any point never leaves a session where the
/// prefix has been removed but C-q is not yet bound — that ordering is the
/// difference between a degraded session and a locked one.
pub(crate) fn barn_session_options(barn: &Barn) -> Vec<BarnSessionOption> {
    let who = format!(
        " {}@{} ",
        barn.user.as_deref().unwrap_or("root"),
        barn.host.as_deref().unwrap_or("?")
    );

    let required = |name, value: &str| BarnSessionOption {
        name,
        value: value.to_string(),
        required: true,
    };
    let cosmetic = |name, value: String| BarnSessionOption {
        name,
        value,
        required: false,
    };

    vec![
        required("key-table", REMOTE_KEY_TABLE),
        // A custom key-table does NOT disable the prefix, whatever nested-tmux
        // folklore says. tmux's server_client_is_default_key_table() compares the
        // client's current table against the *session's* key-table option, so with
        // both set to `yeehaw-remote` it returns true and the prefix check runs
        // anyway. Verified on tmux 3.6a: inside a barn session with only
        // `key-table` set, C-b c creates a local window and C-b & kills the barn
        // session. `prefix None` is what actually delivers the pass-through the
        // design promises, and it is per-session — the local ranch keeps C-b.
        required("prefix", "None"),
        cosmetic("detach-on-destroy", "off".into()),
        cosmetic("status", "on".into()),
        cosmetic("status-style", "bg=#8b1a1a,fg=#f0f0f0,bold".into()),
        cosmetic("status-left", format!(" REMOTE {} ", barn.name)),
        cosmetic("status-left-length", "40".into()),
        cosmetic("status-right", format!("{}· C-q: local ranch ", who)),
        cosmetic("status-right-length", "60".into()),
    ]
}

/// Tear down a barn's local session. The remote ranch is unaffected.
pub fn disconnect_barn(barn_name: &str) {
    let target = barn_session_target(barn_name);
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &target])
        .output();
}

/// Tear down every local barn session. Remote ranches are unaffected — only the
/// local windows onto them close.
///
/// Called before [`kill_yeehaw_session`] on quit: barn sessions are siblings of
/// the yeehaw session, not children of it, so killing yeehaw alone leaves every
/// `yh-barn-*` running with no dashboard left to reach it from.
///
/// `connected_barn_sessions` yields names exactly as `list-sessions` prints
/// them, bare, so each one is put through [`exact_target`] before it reaches
/// `-t`. A bare target is a prefix pattern to tmux, and quitting is not the
/// moment to have `-t yh-barn-guided` also take out `yh-barn-guided-2`.
pub fn kill_all_barn_sessions() {
    for name in connected_barn_sessions(&list_session_names()) {
        let target = exact_target(&name);
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &target])
            .output();
    }
}

/// Wrap `s` in single quotes so a POSIX shell reproduces it byte for byte.
///
/// Unconditional by design. There is no "does it look dangerous" character
/// class to keep in sync — inside single quotes every byte is literal, so `;`
/// `|` `&` `>` `<` `(` `)` `*` `?` `[` `]` `#` `!` `$`, backticks, backslashes,
/// and newlines are all inert. Embedded single quotes are closed, escaped, and
/// reopened (`'\''`), which is the only sequence single quotes cannot contain.
///
/// A leading `~/` (or a bare `~`) is the one deliberate exception: `cd '~/app'`
/// would target a literal directory named `~`. The tilde is emitted as
/// `"$HOME"` — double-quoted, so it stays one word even when the home path
/// contains spaces — and the remainder is single-quoted as usual:
/// `~/sites/my app` becomes `"$HOME"'/sites/my app'`. Only a *leading* tilde is
/// treated this way, matching what the shell itself does; `a~b` stays literal.
///
/// The `$HOME` that results expands in whichever shell finally runs the string.
/// For a remote path the caller re-escapes the whole command before handing it
/// to tmux, so the `$HOME` is protected locally and expands on the remote host.
pub fn shell_escape(s: &str) -> String {
    if s == "~" {
        return "\"$HOME\"".to_string();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return format!("\"$HOME\"{}", single_quote(&format!("/{}", rest)));
    }
    single_quote(s)
}

/// `shell_escape` without the leading-tilde exception.
///
/// Use this for values that are *not* paths — a grep pattern, a systemd unit
/// name — where a leading `~/` must stay the two literal bytes the caller wrote
/// rather than expanding to the home directory.
pub(crate) fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(fields: &[&str]) -> String {
        fields.join("\t")
    }

    #[test]
    fn parses_a_fully_tagged_window() {
        let w = parse_window_line(&line(&[
            "4", "Guided Pages-claude", "1", "%18", "* building", "node", "1700",
            "claude", "Guided Pages", "local",
        ]))
        .expect("should parse");

        assert_eq!(w.index, 4);
        assert_eq!(w.name, "Guided Pages-claude");
        assert!(w.active);
        assert_eq!(w.pane_id, "%18");
        assert_eq!(w.window_type, "claude");
        assert_eq!(w.project, "Guided Pages");
        assert_eq!(w.barn, "local");
    }

    #[test]
    fn untagged_window_degrades_to_empty_tags_rather_than_failing() {
        // Windows created before scope tagging existed emit empty trailing fields.
        let w = parse_window_line(&line(&[
            "2", "old-window", "0", "%3", "title", "bash", "1700", "", "", "",
        ]))
        .expect("should still parse");

        assert_eq!(w.window_type, "");
        assert_eq!(w.project, "");
        assert_eq!(w.barn, "");
    }

    #[test]
    fn tolerates_lines_truncated_before_the_optional_tag_fields() {
        let w = parse_window_line(&line(&[
            "2", "old-window", "0", "%3", "title", "bash", "1700",
        ]))
        .expect("seven fields is enough");

        assert_eq!(w.index, 2);
        assert_eq!(w.window_type, "");
        assert_eq!(w.project, "");
    }

    #[test]
    fn rejects_lines_missing_required_fields() {
        assert!(parse_window_line("4\tname\t1").is_none());
        assert!(parse_window_line("").is_none());
    }

    #[test]
    fn splits_a_batched_capture_into_one_screen_per_pane() {
        let raw = format!(
            "pane one line a\npane one line b\n{s}\npane two only line\n{s}\npane three",
            s = CAPTURE_SENTINEL
        );
        let panes = split_captures(&raw, 3);

        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0], vec!["pane one line a", "pane one line b"]);
        assert_eq!(panes[1], vec!["pane two only line"]);
        assert_eq!(panes[2], vec!["pane three"]);
    }

    #[test]
    fn split_captures_preserves_slots_for_panes_that_produced_nothing() {
        let raw = format!("{s}\nonly the middle pane spoke\n{s}", s = CAPTURE_SENTINEL);
        let panes = split_captures(&raw, 3);

        assert_eq!(panes.len(), 3);
        assert!(panes[0].is_empty());
        assert_eq!(panes[1], vec!["only the middle pane spoke"]);
        assert!(panes[2].is_empty());
    }

    #[test]
    fn split_captures_always_returns_exactly_the_requested_count() {
        assert_eq!(split_captures("", 4).len(), 4);
        assert_eq!(split_captures("just one pane", 1).len(), 1);
    }

    #[test]
    fn sentinel_never_leaks_into_captured_output() {
        let raw = format!("alpha\n{s}\nbeta", s = CAPTURE_SENTINEL);
        for pane in split_captures(&raw, 2) {
            for l in pane {
                assert!(!l.contains(CAPTURE_SENTINEL), "sentinel leaked: {l:?}");
            }
        }
    }

    #[test]
    fn barn_session_name_slugifies_spaces_and_case() {
        assert!(barn_session_name("camera pi").starts_with("yh-barn-camera-pi-"));
        assert!(barn_session_name("BIG UPS").starts_with("yh-barn-big-ups-"));
    }

    #[test]
    fn barn_session_name_strips_every_character_tmux_treats_specially() {
        // ':' and '.' are target separators; '$', '%', '@' are ID sigils; '=' forces
        // exact match; '*', '?', '[' are glob metacharacters honored by rule 4.
        for name in ["prod:web.1", "a/b\\c", "$a%b@c=d", "x*y?z[0]"] {
            let s = barn_session_name(name);
            let body = s.strip_prefix("yh-barn-").expect("prefix");
            assert!(
                body.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{name} produced {s}, which leaves a character tmux would interpret"
            );
        }
    }

    #[test]
    fn barn_session_name_collapses_and_trims_separators() {
        assert!(barn_session_name("  green   pro  ").starts_with("yh-barn-green-pro-"));
        assert!(barn_session_name("--x--").starts_with("yh-barn-x-"));
    }

    #[test]
    fn barn_session_name_distinguishes_names_that_slugify_identically() {
        // These four reduce to the same slug. Without the hash suffix they would be
        // one session, so connecting to one would land you on another's host.
        let names = ["camera pi", "camera-pi", "camera_pi", "Camera Pi"];
        let mut seen = std::collections::HashSet::new();
        for n in names {
            assert!(seen.insert(barn_session_name(n)), "{n} collided with an earlier name");
        }
    }

    #[test]
    fn barn_session_name_distinguishes_names_that_slugify_to_nothing() {
        assert_ne!(barn_session_name("!!!"), barn_session_name("???"));
        assert_ne!(barn_session_name("!!!"), barn_session_name(""));
    }

    #[test]
    fn barn_session_name_is_stable_across_releases() {
        // Pins the hash. Changing it orphans every live yh-barn-* session: has-session
        // misses, a duplicate spawns, and the old one leaks until Q.
        assert_eq!(barn_session_name("guided"), "yh-barn-guided-df511ffd");
        assert_eq!(barn_session_name(""), "yh-barn-x-811c9dc5");
    }

    #[test]
    fn no_barn_session_name_is_a_prefix_of_another() {
        // tmux rule 3 resolves a bare target by prefix, so a prefix relationship
        // between two names is a wrong-host connect. The '=' target form is the real
        // defense; this asserts the names are well-shaped independently of it.
        let names = ["guided", "guided-2", "camera pi", "BIG UPS", "greenpro-prod",
                     "greenpro", "killswitch-demo", "killswitch", "a", ""];
        let sessions: Vec<String> = names.iter().map(|n| barn_session_name(n)).collect();
        for (i, a) in sessions.iter().enumerate() {
            for (j, b) in sessions.iter().enumerate() {
                if i != j {
                    assert!(!b.starts_with(a.as_str()), "{a} is a prefix of {b}");
                }
            }
        }
    }

    #[test]
    fn barn_session_target_forces_exact_matching() {
        // Without the '=', 'tmux kill-session -t yh-barn-guided' kills yh-barn-guided-2.
        let t = barn_session_target("guided");
        assert!(t.starts_with('='));
        assert_eq!(&t[1..], barn_session_name("guided"));
    }

    #[test]
    fn exact_target_prefixes_a_session_name_read_back_from_tmux() {
        // The quit path only has session names, never barn names — the hash in
        // barn_session_name is one-way, so barn_session_target cannot be reached
        // from here. Every such name still has to become an exact target before
        // it is handed to kill-session.
        for name in connected_barn_sessions(&[
            barn_session_name("guided"),
            barn_session_name("guided-2"),
        ]) {
            let t = exact_target(&name);
            assert!(t.starts_with('='), "{t} would prefix-match a sibling session");
            assert_eq!(&t[1..], name);
        }
    }

    #[test]
    fn exact_target_and_barn_session_target_agree() {
        assert_eq!(
            barn_session_target("camera pi"),
            exact_target(&barn_session_name("camera pi"))
        );
    }

    #[test]
    fn identifies_connected_barn_sessions_from_a_session_list() {
        let names = ["yeehaw", "yh-barn-guided", "yh-barn-camera-pi", "other"];
        let connected = connected_barn_sessions(&names.map(String::from));

        assert!(connected.contains("yh-barn-guided"));
        assert!(connected.contains("yh-barn-camera-pi"));
        assert!(!connected.contains("yeehaw"));
        assert!(!connected.contains("other"));
    }

    #[test]
    fn connected_set_membership_is_exact_not_prefix() {
        // NOTE: this asserts a property of the in-memory HashSet only. It does NOT
        // protect the live tmux path — `tmux -t <bare name>` prefix-matches
        // regardless of what this set contains. That defense is barn_session_target()'s
        // '=' prefix, asserted in Task 1 and used by every -t call site below.
        let names = [barn_session_name("guided-2")];
        let connected = connected_barn_sessions(&names);

        assert!(connected.contains(&barn_session_name("guided-2")));
        assert!(!connected.contains(&barn_session_name("guided")));
    }

    // === barn session isolation =========================================

    fn test_barn(name: &str) -> Barn {
        Barn {
            name: name.into(),
            host: Some("172.233.141.59".into()),
            user: Some("forge".into()),
            port: Some(2222),
            identity_file: None,
            critters: vec![],
            source: None,
            connection_type: None,
            connection_config: None,
            connectable: None,
        }
    }

    fn option_named<'a>(opts: &'a [BarnSessionOption], name: &str) -> &'a BarnSessionOption {
        opts.iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("barn sessions must set '{name}'"))
    }

    fn position_of(opts: &[BarnSessionOption], name: &str) -> usize {
        opts.iter().position(|o| o.name == name).expect(name)
    }

    #[test]
    fn barn_session_moves_off_the_root_key_table() {
        let opts = barn_session_options(&test_barn("guided"));
        let table = option_named(&opts, "key-table");
        assert_eq!(table.value, REMOTE_KEY_TABLE);
        assert!(
            table.required,
            "without key-table the local C-y/C-h/C-l/C-p eat the remote yeehaw's own navigation"
        );
    }

    #[test]
    fn barn_session_disables_the_prefix() {
        // A custom key-table does NOT disable the prefix: tmux's
        // server_client_is_default_key_table() compares the current table against
        // the *session's* key-table option, so with both set to 'yeehaw-remote' it
        // returns true and the prefix check still runs. Verified on tmux 3.6a —
        // with key-table alone, C-b c creates a local window inside the barn
        // session and C-b & kills it. 'prefix None' is the part that works.
        let opts = barn_session_options(&test_barn("guided"));
        let prefix = option_named(&opts, "prefix");
        assert_eq!(prefix.value, "None");
        assert!(prefix.required, "a live C-b inside a barn session acts on the LOCAL server");
    }

    #[test]
    fn key_table_is_applied_before_the_prefix_is_removed() {
        // Ordering is a safety property, not a style choice. If 'prefix None' were
        // applied first and 'key-table' then failed, the session would have no
        // prefix and no C-q — a lockout. This way the escape hatch is always
        // bound before the fallback is taken away.
        let opts = barn_session_options(&test_barn("guided"));
        assert!(position_of(&opts, "key-table") < position_of(&opts, "prefix"));
    }

    #[test]
    fn only_isolation_options_are_required_styling_is_best_effort() {
        // A missing red bar is a worse-looking session; a missing key-table or
        // prefix is a session that lies about where keystrokes are going. Only the
        // second kind may refuse a connection.
        let opts = barn_session_options(&test_barn("guided"));
        let required: Vec<&str> = opts.iter().filter(|o| o.required).map(|o| o.name).collect();
        assert_eq!(required, vec!["key-table", "prefix"]);
    }

    #[test]
    fn barn_session_status_bar_names_the_barn_and_its_host() {
        let opts = barn_session_options(&test_barn("camera pi"));
        assert!(option_named(&opts, "status-left").value.contains("camera pi"));
        assert!(option_named(&opts, "status-right").value.contains("forge@172.233.141.59"));
    }

    #[test]
    fn barn_session_status_bar_survives_a_barn_with_no_user_or_host() {
        let mut b = test_barn("guided");
        b.user = None;
        b.host = None;
        let opts = barn_session_options(&b);
        assert!(option_named(&opts, "status-right").value.contains("root@?"));
    }

    // === generated tmux config ==========================================

    fn remote_table_bindings() -> Vec<String> {
        generate_tmux_config()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| l.starts_with(&format!("bind-key -T {}", REMOTE_KEY_TABLE)))
            .collect()
    }

    #[test]
    fn the_remote_key_table_reserves_exactly_one_key() {
        // The whole promise of a barn session is that one key is taken and every
        // other one reaches the remote. A second binding here breaks that silently.
        let bindings = remote_table_bindings();
        assert_eq!(bindings.len(), 1, "expected one binding, got {bindings:?}");
        assert!(bindings[0].contains(" C-q "), "the reserved key must be C-q: {}", bindings[0]);
    }

    #[test]
    fn the_escape_binding_targets_the_yeehaw_session_exactly() {
        // A bare '-t yeehaw' is a prefix pattern (tmux target rule 3), so with no
        // 'yeehaw' session and a 'yeehaw-scratch' present, C-q would drop the user
        // into the scratch session instead of the local ranch. Same hazard the
        // rest of this feature exists to avoid.
        let binding = &remote_table_bindings()[0];
        assert!(binding.ends_with("switch-client -t =yeehaw"), "{binding}");
    }

    #[test]
    fn the_generated_config_defines_the_table_connect_to_barn_requires() {
        // ensure_remote_key_table() sources this file and then asserts the table
        // exists. If the constant and the config literal ever drift apart, that
        // check fails at connect time instead of here.
        assert!(generate_tmux_config().contains(&format!("bind-key -T {} ", REMOTE_KEY_TABLE)));
    }

    // === shell_escape ===================================================
    //
    // These run the escaped text through a real `sh`. Asserting that the shell
    // hands `printf` back exactly the original bytes is an end-to-end proof that
    // nothing was interpreted, rather than a restatement of the implementation.

    /// `printf %s <escaped>` under sh, with an optional HOME override.
    fn sh_expand(escaped: &str, home: Option<&str>) -> String {
        let script = format!("printf %s {}", escaped);
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", &script]);
        if let Some(h) = home {
            cmd.env("HOME", h);
        }
        let out = cmd.output().expect("sh should be runnable");
        assert!(
            out.status.success(),
            "sh could not even parse {script:?} (stderr: {})",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stderr.is_empty(),
            "sh wrote to stderr for {script:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// The escaped form of `input` must expand back to `input`, byte for byte.
    fn assert_sh_roundtrip(input: &str) {
        let escaped = shell_escape(input);
        assert_eq!(
            sh_expand(&escaped, None),
            input,
            "{input:?} escaped to {escaped:?}, which sh did not reproduce verbatim"
        );
    }

    #[test]
    fn shell_metacharacters_survive_a_real_shell_unchanged() {
        // Every one of these passed through raw before escaping became
        // unconditional, so a barn host of "1.2.3.4;reboot" ran reboot locally.
        for input in [
            "1.2.3.4;id",
            "host|nc",
            "host&&id",
            "host>/tmp/pwn",
            "/opt/app;reboot",
            "host<in",
            "$(id)",
            "`id`",
            "a&b",
            "x(1)",
            "*",
            "?",
            "[abc]",
            "#comment",
            "!!",
            "a\nb",
            "a\tb",
            "~not/leading/../a~b",
            "",
            " ",
            "/var/www/html",
            "--kubeconfig=/etc/k.yaml",
        ] {
            assert_sh_roundtrip(input);
        }
    }

    #[test]
    fn quotes_and_backslashes_survive_a_real_shell_unchanged() {
        for input in [
            "it's",
            "''",
            "'",
            "a'b'c",
            "'; id; '",
            "\"",
            "say \"hi\"",
            "mixed \"a\" and 'b'",
            "back\\slash",
            "a\\'b",
            "$HOME",
            "${HOME}",
            "100$",
            "back`tick`",
        ] {
            assert_sh_roundtrip(input);
        }
    }

    #[test]
    fn a_json_mcp_config_survives_a_real_shell_unchanged() {
        // build_mcp_config produces this shape and it is passed to --mcp-config
        // through the same single shell tmux runs.
        assert_sh_roundtrip(
            r#"{"mcpServers":{"yeehaw":{"command":"/usr/local/bin/yeehaw","args":["mcp-server"]}}}"#,
        );
    }

    #[test]
    fn injected_commands_do_not_run() {
        // The roundtrip above proves this, but state it directly: if `id` ran,
        // its output would appear and the payload text would not.
        let escaped = shell_escape("1.2.3.4;id");
        let out = sh_expand(&escaped, None);
        assert_eq!(out, "1.2.3.4;id");
        assert!(!out.contains("uid="), "the injected `id` executed: {out:?}");
    }

    #[test]
    fn every_escaped_value_is_a_single_shell_word() {
        // Splitting into two words is as damaging as injection: `ssh -i my key`
        // silently reads a different file and takes `key` as the destination.
        for input in ["a b", "a\nb", "*", "", "one'two three", "~/my dir/x"] {
            let script = format!("set -- {}; printf %s $#", shell_escape(input));
            let out = std::process::Command::new("sh")
                .args(["-c", &script])
                .output()
                .expect("sh should be runnable");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "1",
                "{input:?} escaped to more (or fewer) than one word"
            );
        }
    }

    #[test]
    fn escaping_is_unconditional_even_for_innocuous_input() {
        // The old implementation returned plain strings unquoted and so had to
        // enumerate every dangerous character. Pin the "no character class"
        // property so a future edit cannot reintroduce that decision.
        assert_eq!(shell_escape("plain"), "'plain'");
        assert_eq!(shell_escape("/var/log/app.log"), "'/var/log/app.log'");
        assert_eq!(shell_escape(""), "''");
        assert_eq!(shell_escape("1.2.3.4;id"), "'1.2.3.4;id'");
    }

    #[test]
    fn a_leading_tilde_still_expands_to_home() {
        // create_ssh_window passes a bare "~" for "connect to the barn's home",
        // and livestock paths are frequently "~/sites/app". Quoting these
        // literally would cd into a directory actually named "~".
        assert_eq!(sh_expand(&shell_escape("~"), Some("/home/forge")), "/home/forge");
        assert_eq!(
            sh_expand(&shell_escape("~/sites/app"), Some("/home/forge")),
            "/home/forge/sites/app"
        );
    }

    #[test]
    fn a_tilde_path_stays_one_word_when_home_contains_a_space() {
        // "$HOME" is double-quoted for exactly this reason; bare $HOME would
        // split "/Users/cam smith/x" into two arguments.
        let script = format!("set -- {}; printf '%s|%s' \"$#\" \"$1\"", shell_escape("~/my dir/x"));
        let out = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("HOME", "/home/first last")
            .output()
            .expect("sh should be runnable");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "1|/home/first last/my dir/x"
        );
    }

    #[test]
    fn a_tilde_path_is_still_safe_after_expansion() {
        // The remainder after "~/" is single-quoted like anything else, so a
        // path of "~/app;reboot" must not inject.
        assert_eq!(
            sh_expand(&shell_escape("~/app;reboot"), Some("/home/forge")),
            "/home/forge/app;reboot"
        );
        assert_eq!(
            sh_expand(&shell_escape("~/it's here"), Some("/home/forge")),
            "/home/forge/it's here"
        );
    }

    #[test]
    fn only_a_leading_tilde_is_special() {
        // The previous code did a global replace of "~" with "$HOME", which
        // corrupted every non-leading tilde. sh does not expand those either.
        for input in ["/opt/a~b", "a~", "~~", "x/~/y", "~user/dir"] {
            assert_sh_roundtrip(input);
        }
    }

    #[test]
    fn a_tilde_only_expands_in_the_shell_that_finally_runs_it() {
        // create_ssh_window escapes the remote path, folds it into a command
        // string, then escapes that whole string again for the local shell.
        // The local pass must leave $HOME untouched so it expands remotely.
        let remote_cmd = format!("cd {} && exec $SHELL -l", shell_escape("~/sites/app"));
        let for_tmux = shell_escape(&remote_cmd);
        assert_eq!(
            sh_expand(&for_tmux, Some("/local/home")),
            "cd \"$HOME\"'/sites/app' && exec $SHELL -l",
            "the local shell expanded a variable meant for the remote host"
        );
    }

    // === parse_new_window_index =========================================

    fn output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn reads_the_index_tmux_printed() {
        assert_eq!(parse_new_window_index(&output(0, "7\n", ""), "ssh").unwrap(), 7);
    }

    #[test]
    fn a_failed_new_window_is_an_error_not_window_zero() {
        // Window 0 is the yeehaw dashboard. Falling back to 0 made the caller
        // retag it as an ssh window and jump to it, as though the connection
        // had opened.
        let err = parse_new_window_index(&output(1, "", "no space for new window"), "ssh")
            .expect_err("a failed new-window must not yield an index");
        assert!(err.to_string().contains("no space for new window"));
    }

    #[test]
    fn a_failure_with_no_stderr_still_produces_a_message() {
        let err = parse_new_window_index(&output(1, "", "\n"), "shell").unwrap_err();
        assert!(!err.to_string().trim().is_empty());
        assert!(err.to_string().contains("shell"));
    }

    #[test]
    fn unparseable_output_is_an_error_not_window_zero() {
        // A success exit with junk (or empty) stdout used to parse to 0 too.
        for stdout in ["", "\n", "not-a-number", "%12", "-1"] {
            assert!(
                parse_new_window_index(&output(0, stdout, ""), "ssh").is_err(),
                "{stdout:?} should not have produced a window index"
            );
        }
    }

    // === check_tmux_ok ==================================================

    #[test]
    fn a_tmux_command_that_succeeded_is_ok() {
        assert!(check_tmux_ok(&output(0, "", ""), "do the thing").is_ok());
    }

    #[test]
    fn a_nonzero_exit_is_an_error_carrying_tmux_stderr() {
        // This is the whole point. Command::status()/output() return Err only when
        // tmux cannot be *spawned* — a tmux that ran and exited 1 is Ok. connect
        // used to only check the spawn, so a failed new-session and a failed
        // switch-client both fell through and connect_to_barn returned Ok(()):
        // pressing `c` moved nothing and reported nothing.
        let err = check_tmux_ok(&output(1, "", "no such session: =yh-barn-guided"), "switch")
            .expect_err("a non-zero tmux exit must be an error");
        let msg = err.to_string();
        assert!(msg.contains("no such session"), "{msg}");
        assert!(msg.contains("switch"), "{msg}");
    }

    #[test]
    fn a_silent_failure_still_names_what_was_attempted() {
        let err = check_tmux_ok(&output(1, "", "  \n"), "create the barn session").unwrap_err();
        assert!(err.to_string().contains("create the barn session"));
    }
}
