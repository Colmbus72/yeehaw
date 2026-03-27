use anyhow::Result;
use crate::config;

/// Check if the remote branch has new commits. Returns true if a trail should trigger.
/// Called by the poll worm's exec command: `yeehaw trail poll {livestock} {trail}`
pub fn check_and_trigger(
    livestock_name: &str,
    trail_name: &str,
    repo_url: &str,
    branch: &str,
    barn_host: &str,
    barn_user: &str,
    barn_port: u16,
    barn_identity_file: &str,
) -> Result<bool> {
    // Run git ls-remote on the barn via SSH
    let output = std::process::Command::new("ssh")
        .arg("-p").arg(barn_port.to_string())
        .arg("-i").arg(barn_identity_file)
        .arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg("-o").arg("ConnectTimeout=10")
        .arg("-o").arg("BatchMode=yes")
        .arg(format!("{}@{}", barn_user, barn_host))
        .arg(format!("git ls-remote {} refs/heads/{}", repo_url, branch))
        .output()?;

    if !output.status.success() {
        anyhow::bail!("git ls-remote failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let remote_sha = stdout.split_whitespace().next()
        .unwrap_or("")
        .to_string();

    if remote_sha.is_empty() {
        anyhow::bail!("No SHA returned for {}/refs/heads/{}", repo_url, branch);
    }

    // Compare to stored SHA
    let stored_sha = config::read_poll_sha(livestock_name, branch);

    if stored_sha.as_deref() == Some(&remote_sha) {
        return Ok(false); // No change
    }

    // SHA changed — update immediately (prevents double-trigger)
    config::write_poll_sha(livestock_name, branch, &remote_sha)?;

    // Write trigger file
    let now = chrono::Utc::now();
    let filename = format!("poll-{}--{}--{}.json", livestock_name, trail_name,
                           now.format("%Y-%m-%dT%H-%M-%S"));
    let trigger_path = config::worm_triggers_dir().join(&filename);

    let trigger = serde_json::json!({
        "worm": format!("poll--{}--{}", livestock_name, trail_name),
        "triggered_at": now.to_rfc3339(),
        "trigger": "poll",
        "livestock": livestock_name,
        "trail": trail_name,
        "branch": branch,
        "sha": remote_sha,
    });

    std::fs::create_dir_all(config::worm_triggers_dir())?;
    std::fs::write(&trigger_path, serde_json::to_string_pretty(&trigger)?)?;

    Ok(true)
}
