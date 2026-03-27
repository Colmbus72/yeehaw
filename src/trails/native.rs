use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;

use crate::config;
use super::provider::{StepStatus, StepUpdate, TrailContext, TrailProvider};

pub struct NativeProvider {
    cancelled: Arc<AtomicBool>,
}

impl NativeProvider {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl TrailProvider for NativeProvider {
    fn name(&self) -> &str {
        "native"
    }

    fn execute(&self, ctx: TrailContext) -> Result<mpsc::Receiver<StepUpdate>> {
        let (tx, rx) = mpsc::channel(256);
        let cancelled = self.cancelled.clone();

        // Reset cancellation flag
        cancelled.store(false, Ordering::SeqCst);

        let barn = ctx.barn;
        let job = ctx.job;
        let run_dir = ctx.run_dir;
        let base_env = ctx.env_vars;

        std::thread::spawn(move || {
            for (i, step) in job.steps.iter().enumerate() {
                if cancelled.load(Ordering::SeqCst) {
                    let _ = tx.blocking_send(StepUpdate {
                        step_index: i,
                        status: StepStatus::Failed { exit_code: -1 },
                        output_line: Some("Cancelled by user".to_string()),
                    });
                    break;
                }

                // Merge step-level env on top of base env
                let mut step_env: Vec<(String, String)> = base_env.clone();

                // Update STEP_NAME for this specific step
                if let Some(entry) = step_env.iter_mut().find(|(k, _)| k == "STEP_NAME") {
                    entry.1 = step.name.clone();
                }

                // Layer step-level env (highest priority)
                if let Some(ref extra) = step.env {
                    for (k, v) in extra {
                        if let Some(entry) = step_env.iter_mut().find(|(key, _)| key == k) {
                            entry.1 = v.clone();
                        } else {
                            step_env.push((k.clone(), v.clone()));
                        }
                    }
                }

                let timeout_secs = step.timeout_minutes.unwrap_or(1) * 60;
                let step_start = std::time::Instant::now();

                // Signal step is running
                let _ = tx.blocking_send(StepUpdate {
                    step_index: i,
                    status: StepStatus::Running,
                    output_line: None,
                });

                // Prepend env var exports so steps can use $NAME, $REPO_PATH, etc.
                let env_exports: String = step_env.iter()
                    .map(|(k, v)| format!("export {}='{}'", k, v.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join("; ");
                let full_command = if env_exports.is_empty() {
                    format!("{} 2>&1", step.run)
                } else {
                    format!("{}; {} 2>&1", env_exports, step.run)
                };

                // Build command — local or SSH
                let mut cmd = if config::is_local_barn(&barn) {
                    let repo_path = base_env.iter()
                        .find(|(k, _)| k == "REPO_PATH")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or(".");
                    let safe_path = repo_path.replace('\'', "'\\''");
                    let local_cmd = format!("cd '{}' && {}", safe_path, full_command);
                    let mut c = Command::new("sh");
                    c.arg("-c").arg(&local_cmd);
                    c
                } else {
                    let host = barn.host.as_deref().unwrap_or(&barn.name);
                    let user = barn.user.as_deref().unwrap_or("root");
                    let port = barn.port.unwrap_or(22);

                    let mut c = Command::new("ssh");
                    c.arg("-p").arg(port.to_string());

                    if let Some(ref key) = barn.identity_file {
                        c.arg("-i").arg(key);
                    }

                    c.arg("-o").arg("StrictHostKeyChecking=accept-new");
                    c.arg("-o").arg("ConnectTimeout=10");
                    c.arg(format!("{}@{}", user, host));
                    c.arg(&full_command);
                    c
                };

                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::null()); // stderr merged into stdout via 2>&1

                // Open log file for this step
                let log_path = run_dir.join(format!("step-{}.log", i));
                let mut log_file = std::fs::File::create(&log_path).ok();

                match cmd.spawn() {
                    Ok(mut child) => {
                        let mut timed_out = false;

                        // Stream stdout (stderr is merged via 2>&1)
                        if let Some(stdout) = child.stdout.take() {
                            let reader = BufReader::new(stdout);
                            for line in reader.lines() {
                                if cancelled.load(Ordering::SeqCst) {
                                    let _ = child.kill();
                                    break;
                                }
                                if step_start.elapsed().as_secs() > timeout_secs {
                                    let _ = child.kill();
                                    timed_out = true;
                                    break;
                                }
                                if let Ok(line) = line {
                                    if let Some(ref mut f) = log_file {
                                        let _ = writeln!(f, "{}", line);
                                    }
                                    let _ = tx.blocking_send(StepUpdate {
                                        step_index: i,
                                        status: StepStatus::Running,
                                        output_line: Some(line),
                                    });
                                }
                            }
                        }

                        if timed_out {
                            let timeout_min = step.timeout_minutes.unwrap_or(1);
                            let _ = tx.blocking_send(StepUpdate {
                                step_index: i,
                                status: StepStatus::Failed { exit_code: -1 },
                                output_line: Some(format!("Timed out after {} minute(s)", timeout_min)),
                            });
                            break;
                        }

                        match child.wait() {
                            Ok(status) => {
                                let exit_code = status.code().unwrap_or(-1);
                                if exit_code == 0 {
                                    let _ = tx.blocking_send(StepUpdate {
                                        step_index: i,
                                        status: StepStatus::Success,
                                        output_line: None,
                                    });
                                } else {
                                    let _ = tx.blocking_send(StepUpdate {
                                        step_index: i,
                                        status: StepStatus::Failed { exit_code },
                                        output_line: Some(format!("Exit code: {}", exit_code)),
                                    });
                                    break; // Stop on first failure
                                }
                            }
                            Err(e) => {
                                let _ = tx.blocking_send(StepUpdate {
                                    step_index: i,
                                    status: StepStatus::Failed { exit_code: -1 },
                                    output_line: Some(format!("Failed to wait: {}", e)),
                                });
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(StepUpdate {
                            step_index: i,
                            status: StepStatus::Failed { exit_code: -1 },
                            output_line: Some(format!("Failed to spawn: {}", e)),
                        });
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    fn cancel(&self) -> Result<()> {
        self.cancelled.store(true, Ordering::SeqCst);
        Ok(())
    }
}
