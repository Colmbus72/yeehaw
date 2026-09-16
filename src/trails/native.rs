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

        // `barn_is_this_machine`, not `is_local_barn`: the target is a `Barn`
        // record, and after `migrate::adopt_this_machine` the record for the
        // machine this runner is *on* carries that machine's real name. Asking
        // only about the synthetic `local` sent every step of every trail
        // targeting the self-barn down the ssh branch — to a record that
        // deliberately has no host, because you do not ssh to yourself.
        //
        // Hoisted out of the thread below on purpose. It reads the config file,
        // and doing that once per step re-asked a question that cannot change
        // mid-run.
        let run_here = config::barn_is_this_machine(&barn);

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
                // Wrap the step body in a subshell so trailing newlines don't
                // break the `2>&1` redirection, and its exit code propagates.
                let full_command = if env_exports.is_empty() {
                    format!("( {}\n) 2>&1", step.run)
                } else {
                    format!("{}; ( {}\n) 2>&1", env_exports, step.run)
                };

                // Build command — local or SSH
                let mut cmd = if run_here {
                    let repo_path = base_env.iter()
                        .find(|(k, _)| k == "REPO_PATH")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or(".");
                    // Expand leading `~` / `~/` to the user's home dir so
                    // `cd` works under single quotes (which suppress tilde expansion).
                    let expanded = if repo_path == "~" {
                        std::env::var("HOME").unwrap_or_else(|_| repo_path.to_string())
                    } else if let Some(rest) = repo_path.strip_prefix("~/") {
                        match std::env::var("HOME") {
                            Ok(home) => format!("{}/{}", home.trim_end_matches('/'), rest),
                            Err(_) => repo_path.to_string(),
                        }
                    } else {
                        repo_path.to_string()
                    };
                    let safe_path = expanded.replace('\'', "'\\''");
                    let local_cmd = format!("cd '{}' && {}", safe_path, full_command);
                    let mut c = Command::new("sh");
                    c.arg("-c").arg(&local_cmd);
                    c
                } else {
                    // BatchMode: a trail runs unattended, so it must fail rather
                    // than block forever on a password prompt no one will answer.
                    match crate::ssh::command(
                        &barn,
                        &full_command,
                        crate::ssh::Opts { batch: true, ..Default::default() },
                    ) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(StepUpdate {
                                step_index: i,
                                status: StepStatus::Failed { exit_code: -1 },
                                output_line: Some(format!("SSH error: {}", e)),
                            });
                            break;
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trails::{Trail, TrailJob, TrailStep};
    use crate::types::{Barn, Livestock};
    use std::path::Path;

    /// A one-step trail that runs `command` with `repo_path` as `REPO_PATH`.
    fn context(barn: Barn, repo_path: &Path, run_dir: &Path, command: &str) -> TrailContext {
        TrailContext {
            livestock: Livestock {
                name: "web".into(),
                path: repo_path.display().to_string(),
                barn: None,
                repo: None,
                branch: None,
                log_path: None,
                env_path: None,
                source: None,
                k8s_metadata: None,
                trails: vec![],
            },
            barn,
            trail: Trail {
                name: "deploy".into(),
                on: None,
                env: None,
                jobs: Default::default(),
                id: None,
                created_at: None,
                updated_at: None,
            },
            job: TrailJob {
                runs_on: "native".into(),
                env: None,
                steps: vec![TrailStep {
                    name: "run".into(),
                    run: command.into(),
                    env: None,
                    timeout_minutes: Some(1),
                }],
            },
            run_dir: run_dir.to_path_buf(),
            env_vars: vec![("REPO_PATH".into(), repo_path.display().to_string())],
            run_id: "run-1".into(),
            run_number: 1,
            project_name: Some("api".into()),
        }
    }

    /// Every update the run produced. The runner streams from a thread it owns,
    /// so the end of the channel is the end of the run.
    fn drain(mut rx: mpsc::Receiver<StepUpdate>) -> Vec<StepUpdate> {
        let mut updates = Vec::new();
        while let Some(update) = rx.blocking_recv() {
            updates.push(update);
        }
        updates
    }

    /// THE BUG. A trail targeting this machine's own barn has to run *here*.
    ///
    /// Before adoption the target was the synthetic `local` and `is_local_barn`
    /// said so. Adoption replaces it with a real record under the machine's real
    /// name — which `is_local_barn` calls `false` — so the runner took the ssh
    /// branch and tried to ssh to the machine it was already running on. That
    /// record deliberately carries no `host`, so every step of every trail on an
    /// adopted machine failed with "has no host configured" instead of running.
    #[test]
    fn a_trail_on_the_adopted_self_barn_runs_here_instead_of_ssh_to_itself() {
        let ranch = crate::testing::temp_ranch();
        crate::migrate::adopt_this_machine("imac").unwrap();

        let imac = config::load_barns()
            .into_iter()
            .find(|b| b.name == "imac")
            .expect("adoption mints the record");
        // Both halves of the control. Without them a pass proves nothing: the
        // first says this barn *is* the ssh branch under the old rule, the
        // second says the ssh branch has nothing to dial and so cannot
        // accidentally succeed.
        assert!(!config::is_local_barn(&imac), "control: not the synthetic placeholder");
        assert!(imac.host.is_none(), "control: a self-barn has nothing to dial");

        let run_dir = ranch.dir.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();

        let rx = NativeProvider::new()
            .execute(context(imac, ranch.dir.path(), &run_dir, "echo yeehaw-ran-here"))
            .unwrap();
        let updates = drain(rx);

        let transcript: Vec<_> =
            updates.iter().map(|u| (&u.status, u.output_line.as_deref())).collect();
        assert!(
            updates
                .iter()
                .any(|u| u.output_line.as_deref().is_some_and(|l| l.contains("yeehaw-ran-here"))),
            "the step never ran on this machine: {:?}",
            transcript
        );
        assert!(
            updates.iter().any(|u| u.status == StepStatus::Success),
            "the run did not finish: {:?}",
            transcript
        );
    }
}
