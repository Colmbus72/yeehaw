use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub mod provider;
pub mod native;
pub mod runner;
pub mod history;
pub mod polling;

// ============================================================================
// Trail Definition (loaded from ~/.yeehaw/trails/*.yaml)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trail {
    pub name: String,
    #[serde(default)]
    pub on: Option<TrailTrigger>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    pub jobs: BTreeMap<String, TrailJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailTrigger {
    #[serde(default)]
    pub push: Option<PushTrigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushTrigger {
    #[serde(default)]
    pub branches: Option<Vec<String>>,
    /// Poll interval in seconds. Default 30.
    #[serde(default, rename = "poll-interval")]
    pub poll_interval: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailJob {
    #[serde(default = "default_runs_on", rename = "runs-on")]
    pub runs_on: String,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    pub steps: Vec<TrailStep>,
}

fn default_runs_on() -> String { "native".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailStep {
    pub name: String,
    pub run: String,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Timeout in minutes. Default 1.
    #[serde(default, rename = "timeout-minutes")]
    pub timeout_minutes: Option<u64>,
}

// ============================================================================
// Trail Run (persisted to ~/.yeehaw/trail-runs/)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailRun {
    pub livestock: String,
    pub trail: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String, // "running", "success", "failed", "cancelled"
    pub steps: Vec<TrailStepRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailStepRun {
    pub name: String,
    pub status: String, // "pending", "running", "success", "failed"
    pub exit_code: Option<i32>,
    pub started_at: Option<String>,
    pub duration_ms: Option<u64>,
}

// ============================================================================
// PartialEq implementations
// ============================================================================

impl PartialEq for Trail {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl PartialEq for TrailStep {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl PartialEq for TrailRun {
    fn eq(&self, other: &Self) -> bool {
        self.trail == other.trail && self.started_at == other.started_at
    }
}

impl PartialEq for TrailStepRun {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

// ============================================================================
// Trail helper methods
// ============================================================================

impl Trail {
    /// Get the first job (V2 only executes one job).
    pub fn first_job(&self) -> Option<(&String, &TrailJob)> {
        self.jobs.iter().next()
    }

    /// Get push trigger branches, if any.
    pub fn push_branches(&self) -> Option<&Vec<String>> {
        self.on.as_ref()?.push.as_ref()?.branches.as_ref()
    }

    /// Get poll interval in seconds (default 30).
    pub fn poll_interval(&self) -> u64 {
        self.on.as_ref()
            .and_then(|t| t.push.as_ref())
            .and_then(|p| p.poll_interval)
            .unwrap_or(30)
    }

    /// Whether this trail has an on:push trigger.
    pub fn has_push_trigger(&self) -> bool {
        self.on.as_ref()
            .and_then(|t| t.push.as_ref())
            .is_some()
    }
}
