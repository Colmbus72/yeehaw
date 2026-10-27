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
    project: &str,
) -> Result<bool> {
    // A run already in flight: leave the stored SHA alone, so the commit that
    // arrived meanwhile is still "new" on the first poll after it finishes
    // instead of being consumed by a launch that never happened.
    if crate::jobs::trail_is_running(livestock_name, trail_name) {
        return Ok(false);
    }

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

    match crate::jobs::launch_trail(livestock_name, trail_name, Some(project), Some(&barn.name), "poll")? {
        crate::jobs::Launch::Started => {
            // Recorded only once the run is actually under way, so a launch
            // that could not happen is retried on the next poll.
            config::write_poll_sha(livestock_name, branch, &remote_sha)?;
            Ok(true)
        }
        crate::jobs::Launch::NoSession | crate::jobs::Launch::AlreadyRunning => Ok(false),
    }
}
