use anyhow::Result;
use crate::config;
use crate::ssh;
use crate::types::Barn;

/// Check if the remote branch has new commits. Returns true if a trail should trigger.
/// Called by the poll worm's exec command: `yeehaw trail poll {livestock} {trail}`
pub fn check_and_trigger(
    livestock_name: &str,
    trail_name: &str,
    repo_url: &str,
    branch: &str,
    barn: &Barn,
) -> Result<bool> {
    // Run git ls-remote on the barn via SSH. BatchMode: this runs from a cron
    // worm, so it must fail rather than block on a password prompt.
    let stdout = ssh::run(
        barn,
        &format!("git ls-remote {} refs/heads/{}", repo_url, branch),
        ssh::Opts { batch: true, ..Default::default() },
    )
    .map_err(|e| anyhow::anyhow!("git ls-remote failed: {}", e))?;

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
