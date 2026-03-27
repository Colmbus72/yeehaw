use std::process::Command;

use crate::config;
use crate::types::Barn;

#[derive(Debug, Clone, PartialEq)]
pub enum DetectionState {
    NotChecked,
    Checking,
    Available,
    Unavailable,
    Unreachable,
}

#[derive(Debug, Clone)]
pub struct BarnDetectionResult {
    pub barn_name: String,
    pub state: DetectionState,
    pub checked_at: u64,
}

const CACHE_TTL_SECS: u64 = 5 * 60; // 5 minutes
const SSH_TIMEOUT_SECONDS: u32 = 5;

fn has_valid_ssh_config(barn: &Barn) -> bool {
    barn.host.is_some() && barn.user.is_some() && barn.port.is_some() && barn.identity_file.is_some()
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Probe barns to check if Yeehaw is running on them.
/// Returns results for all SSH-capable barns.
pub fn probe_barns(barns: &[Barn]) -> Vec<BarnDetectionResult> {
    let ssh_barns: Vec<&Barn> = barns
        .iter()
        .filter(|b| !config::is_local_barn(b) && has_valid_ssh_config(b))
        .collect();

    ssh_barns
        .iter()
        .map(|barn| {
            let host = barn.host.as_deref().unwrap();
            let user = barn.user.as_deref().unwrap();
            let port = barn.port.unwrap().to_string();
            let key = barn.identity_file.as_deref().unwrap();

            let result = Command::new("ssh")
                .args([
                    "-o", &format!("ConnectTimeout={}", SSH_TIMEOUT_SECONDS),
                    "-o", "BatchMode=yes",
                    "-o", "StrictHostKeyChecking=accept-new",
                    "-p", &port,
                    "-i", key,
                    &format!("{}@{}", user, host),
                    "tmux has-session -t yeehaw 2>/dev/null && echo 'yeehaw:running'",
                ])
                .output();

            let state = match result {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.contains("yeehaw:running") {
                        DetectionState::Available
                    } else {
                        DetectionState::Unavailable
                    }
                }
                _ => DetectionState::Unreachable,
            };

            BarnDetectionResult {
                barn_name: barn.name.clone(),
                state,
                checked_at: now_epoch(),
            }
        })
        .collect()
}

/// Check if a cached result is still fresh (within 5-min TTL).
pub fn is_cache_fresh(result: &BarnDetectionResult) -> bool {
    now_epoch().saturating_sub(result.checked_at) < CACHE_TTL_SECS
}
