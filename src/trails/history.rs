use std::path::PathBuf;

use crate::config;
use super::TrailRun;

/// Load all trail runs for a given livestock + trail combination.
/// Returns runs sorted by most recent first.
pub fn load_runs(livestock_name: &str, trail_name: &str) -> Vec<TrailRun> {
    config::load_trail_runs(livestock_name, trail_name)
}

/// Get the run directory path for a specific trail run.
pub fn run_dir_for(run: &TrailRun) -> PathBuf {
    let timestamp = run.started_at
        .replace(':', "-");
    // Take just the date-time portion for the directory name
    let ts_clean: String = timestamp.chars().take(19).collect();
    config::trail_run_dir_for(&run.livestock, &run.trail, &ts_clean)
}

/// Load the log content for a specific step of a trail run.
pub fn load_step_log(run: &TrailRun, step_index: usize) -> Option<String> {
    let dir = run_dir_for(run);
    config::load_trail_step_log(&dir, step_index)
}
