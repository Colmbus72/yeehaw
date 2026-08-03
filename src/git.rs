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

/// Detect git info on a remote server via SSH
pub fn detect_remote_git_info(path: &str, barn: &Barn) -> GitInfo {
    let remote_cmd = format!(
        "cd {} && git config --get remote.origin.url 2>/dev/null && git rev-parse --abbrev-ref HEAD 2>/dev/null",
        crate::tmux::shell_escape(path)
    );

    // BatchMode: detection is a background convenience, so it must fail rather
    // than block on an auth prompt.
    match crate::ssh::run(barn, &remote_cmd, crate::ssh::Opts { batch: true, ..Default::default() }) {
        Ok(stdout) => {
            let lines: Vec<&str> = stdout.trim().lines().collect();
            let remote_url = lines.first().map(|s| s.to_string()).filter(|s| !s.is_empty());
            let branch = lines.get(1).map(|s| s.to_string()).filter(|s| !s.is_empty());
            GitInfo { is_git_repo: true, remote_url, branch }
        }
        Err(_) => GitInfo { is_git_repo: false, remote_url: None, branch: None },
    }
}
