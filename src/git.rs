use std::path::Path;
use std::process::Command;

use crate::types::Barn;

#[derive(Debug, Clone)]
pub struct GitInfo {
    pub is_git_repo: bool,
    pub remote_url: Option<String>,
    pub branch: Option<String>,
}

fn expand_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

/// Detect git info for a local path
pub fn detect_git_info(path: &str) -> GitInfo {
    let expanded = expand_path(path);

    // Check if .git exists
    if !Path::new(&expanded).join(".git").exists() {
        return GitInfo { is_git_repo: false, remote_url: None, branch: None };
    }

    let remote_url = Command::new("git")
        .args(["-C", &expanded, "config", "--get", "remote.origin.url"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    let branch = Command::new("git")
        .args(["-C", &expanded, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    GitInfo { is_git_repo: true, remote_url, branch }
}

fn has_valid_ssh_config(barn: &Barn) -> bool {
    barn.host.is_some() && barn.user.is_some() && barn.port.is_some() && barn.identity_file.is_some()
}

/// Detect git info on a remote server via SSH
pub fn detect_remote_git_info(path: &str, barn: &Barn) -> GitInfo {
    if !has_valid_ssh_config(barn) {
        return GitInfo { is_git_repo: false, remote_url: None, branch: None };
    }

    let host = barn.host.as_deref().unwrap();
    let user = barn.user.as_deref().unwrap();
    let port = barn.port.unwrap().to_string();
    let key = barn.identity_file.as_deref().unwrap();

    let remote_cmd = format!(
        "cd {} && git config --get remote.origin.url 2>/dev/null && git rev-parse --abbrev-ref HEAD 2>/dev/null",
        crate::tmux::shell_escape(path)
    );

    let result = Command::new("ssh")
        .args([
            "-p", &port,
            "-i", key,
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=5",
            &format!("{}@{}", user, host),
            &remote_cmd,
        ])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.trim().lines().collect();
            let remote_url = lines.first().map(|s| s.to_string()).filter(|s| !s.is_empty());
            let branch = lines.get(1).map(|s| s.to_string()).filter(|s| !s.is_empty());
            GitInfo { is_git_repo: true, remote_url, branch }
        }
        _ => GitInfo { is_git_repo: false, remote_url: None, branch: None },
    }
}
