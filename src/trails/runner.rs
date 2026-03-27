use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use tokio::sync::mpsc;

use super::{Trail, TrailJob, TrailStep, TrailRun, TrailStepRun};
use super::provider::{StepUpdate, TrailContext, TrailProvider};
use super::native::NativeProvider;
use crate::config;
use crate::types::{Barn, Livestock};

/// Build the merged env var map for a step.
/// Resolution order (later wins): auto-injected < top-level < job-level < step-level.
pub fn build_env_vars(
    trail: &Trail,
    job: &TrailJob,
    step: &TrailStep,
    livestock: &Livestock,
    barn: &Barn,
    run_id: &str,
    run_number: u64,
    project_name: Option<&str>,
) -> Vec<(String, String)> {
    let mut env: HashMap<String, String> = HashMap::new();

    // Layer 1: Auto-injected
    env.insert("NAME".into(), livestock.name.clone());
    env.insert("REPO_PATH".into(), livestock.path.clone());
    env.insert("BRANCH".into(), livestock.branch.as_deref().unwrap_or("main").into());
    env.insert("BARN".into(), barn.host.as_deref().unwrap_or("localhost").into());
    env.insert("BARN_USER".into(), barn.user.as_deref().unwrap_or("root").into());
    env.insert("PROJECT".into(), project_name.unwrap_or("").into());
    env.insert("TRAIL".into(), trail.name.clone());
    env.insert("RUN_ID".into(), run_id.into());
    env.insert("RUN_NUMBER".into(), run_number.to_string());
    env.insert("STEP_NAME".into(), step.name.clone());

    // Layer 2: Top-level env
    if let Some(ref top_env) = trail.env {
        for (k, v) in top_env {
            env.insert(k.clone(), v.clone());
        }
    }

    // Layer 3: Job-level env
    if let Some(ref job_env) = job.env {
        for (k, v) in job_env {
            env.insert(k.clone(), v.clone());
        }
    }

    // Layer 4: Step-level env
    if let Some(ref step_env) = step.env {
        for (k, v) in step_env {
            env.insert(k.clone(), v.clone());
        }
    }

    env.into_iter().collect()
}

/// Start a trail execution. Returns the run directory, a receiver for updates, and the provider.
pub fn start_trail(
    trail: &Trail,
    livestock: &Livestock,
    barn: &Barn,
    project_name: Option<&str>,
) -> Result<(PathBuf, mpsc::Receiver<StepUpdate>, NativeProvider)> {
    // Extract first job (V2: single job only)
    let (_job_name, job) = trail.first_job()
        .ok_or_else(|| anyhow::anyhow!("Trail '{}' has no jobs defined", trail.name))?;

    // Create run directory
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let run_dir = config::trail_run_dir_for(&livestock.name, &trail.name, &timestamp);
    std::fs::create_dir_all(&run_dir)?;

    let run_number = config::count_trail_runs(&livestock.name, &trail.name) + 1;

    // Create initial run record
    let run = TrailRun {
        livestock: livestock.name.clone(),
        trail: trail.name.clone(),
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
        status: "running".to_string(),
        steps: job.steps.iter().map(|s| TrailStepRun {
            name: s.name.clone(),
            status: "pending".to_string(),
            exit_code: None,
            started_at: None,
            duration_ms: None,
        }).collect(),
    };
    config::save_trail_run(&run, &run_dir)?;

    // Build base env vars using the first step (STEP_NAME will be updated per-step by the provider)
    let env_vars = build_env_vars(
        trail, job, &job.steps[0], livestock, barn, &timestamp, run_number, project_name,
    );

    let ctx = TrailContext {
        livestock: livestock.clone(),
        barn: barn.clone(),
        trail: trail.clone(),
        job: job.clone(),
        run_dir: run_dir.clone(),
        env_vars,
        run_id: timestamp,
        run_number,
        project_name: project_name.map(|s| s.to_string()),
    };

    let provider = NativeProvider::new();
    let rx = provider.execute(ctx)?;

    Ok((run_dir, rx, provider))
}
