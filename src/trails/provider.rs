use anyhow::Result;
use tokio::sync::mpsc;

use super::{Trail, TrailJob};
use crate::types::{Barn, Livestock};

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Success,
    Failed { exit_code: i32 },
    Skipped,
}

#[derive(Debug, Clone)]
pub struct StepUpdate {
    pub step_index: usize,
    pub status: StepStatus,
    pub output_line: Option<String>,
}

/// Context needed to execute a trail against a livestock target.
pub struct TrailContext {
    pub livestock: Livestock,
    pub barn: Barn,
    pub trail: Trail,
    pub job: TrailJob,
    /// Directory where run artifacts (run.json, step-N.log) are written.
    pub run_dir: std::path::PathBuf,
    /// Base environment variables (auto-injected + top-level + job-level).
    /// Step-level env is merged per-step during execution.
    pub env_vars: Vec<(String, String)>,
    pub run_id: String,
    pub run_number: u64,
    pub project_name: Option<String>,
}

pub trait TrailProvider: Send + Sync {
    /// Human-readable provider name (e.g., "native", "github-actions").
    fn name(&self) -> &str;

    /// Execute a trail. Returns a receiver that streams step updates in real-time.
    fn execute(
        &self,
        ctx: TrailContext,
    ) -> Result<mpsc::Receiver<StepUpdate>>;

    /// Request cancellation of the currently running trail.
    fn cancel(&self) -> Result<()>;
}
