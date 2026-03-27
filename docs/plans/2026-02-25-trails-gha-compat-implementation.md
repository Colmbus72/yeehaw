# Trails v2: GHA-Compatible Format Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the trail YAML format with GitHub Actions-compatible syntax, add git polling triggers via auto-created worms, expose 11 MCP tools for trail lifecycle management, and expand the auto-injected env vars.

**Architecture:** The trail data model (`src/trails/mod.rs`) gets restructured to match GHA's `on:`/`jobs:`/`steps:` hierarchy. The runner drops `{{handlebars}}` template resolution in favor of a 4-layer env var merge (auto-injected < top-level < job-level < step-level). Git polling is implemented as regular worms that compare remote SHA to stored state and write trigger files. The MCP server gets 11 new tools following the existing `#[tool]` macro pattern.

**Tech Stack:** Rust, serde_yaml (YAML parsing), rmcp (MCP server), tokio (async), ratatui (TUI). Follows existing patterns from worms, wiki, and barn SSH execution.

**Design doc:** `docs/plans/2026-02-25-trails-gha-compat-design.md`

---

### Task 1: Update Trail Data Model

**Files:**
- Modify: `src/trails/mod.rs:1-86` (replace entire Trail/TrailStep structs)

**Step 1: Replace the Trail and TrailStep structs with GHA-compatible types**

Replace lines 12-58 of `src/trails/mod.rs`. Keep `TrailRun` and `TrailStepRun` unchanged — they track execution, not definition.

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod provider;
pub mod native;
pub mod runner;
pub mod history;

// ============================================================================
// Trail Definition (loaded from ~/.yeehaw/trails/*.yaml)
// GHA-compatible: on:/jobs:/steps:/env: structure
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trail {
    pub name: String,
    #[serde(default)]
    pub on: Option<TrailTrigger>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    pub jobs: HashMap<String, TrailJob>,
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
```

Keep `TrailRun` and `TrailStepRun` exactly as they are (lines 41-58 become ~lines 95+). They don't change.

**Step 2: Verify it compiles**

Run: `cargo build 2>&1 | head -40`
Expected: Compilation errors in runner.rs, native.rs, config.rs, livestock_detail.rs, etc. because they reference the old struct fields. That's expected — we'll fix them in subsequent tasks.

**Step 3: Commit**

```bash
git add src/trails/mod.rs
git commit -m "Update trail data model to GHA-compatible format"
```

---

### Task 2: Update Config Parser

**Files:**
- Modify: `src/config.rs:30-31` (add poll_state_dir)
- Modify: `src/config.rs:377-470` (update trail load/save functions)

**Step 1: Add poll state directory helper**

After line 31 in `src/config.rs`, add:

```rust
pub fn poll_state_dir() -> PathBuf { yeehaw_dir().join("poll-state") }
```

**Step 2: Add poll state read/write functions**

After the existing trail functions (after `load_trail_step_log` around line 520), add:

```rust
/// Read the last known SHA for a livestock+branch polling pair.
pub fn read_poll_sha(livestock_name: &str, branch: &str) -> Option<String> {
    let path = poll_state_dir().join(format!("{}--{}.sha", livestock_name, branch));
    std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string())
}

/// Write the current SHA for a livestock+branch polling pair.
pub fn write_poll_sha(livestock_name: &str, branch: &str, sha: &str) -> Result<()> {
    let dir = poll_state_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}--{}.sha", livestock_name, branch));
    std::fs::write(&path, sha)?;
    Ok(())
}

/// Delete poll state for a livestock+branch pair.
pub fn delete_poll_sha(livestock_name: &str, branch: &str) -> Result<()> {
    let path = poll_state_dir().join(format!("{}--{}.sha", livestock_name, branch));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
```

**Step 3: Update `save_trail` (config.rs:452-470)**

The current `save_trail` serializes the old format. Update it to handle the new struct. Since serde handles the GHA-compatible struct directly, the function body likely needs no change — serde_yaml will serialize the new fields. But verify the function doesn't reference removed fields like `provider` or `description`.

Check `save_trail` at line 452. If it directly serializes the Trail struct via `serde_yaml::to_string(trail)?`, it should just work with the new struct. If it manually constructs YAML, rewrite it to use serde:

```rust
pub fn save_trail(trail: &crate::trails::Trail) -> Result<()> {
    let dir = trails_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.yaml", trail.name));
    let yaml = serde_yaml::to_string(trail)?;
    std::fs::write(&path, yaml)?;
    Ok(())
}
```

**Step 4: Add `load_trail_run_count` helper for $RUN_NUMBER env var**

After the poll state functions, add:

```rust
/// Count existing runs for a livestock+trail pair (for $RUN_NUMBER).
pub fn count_trail_runs(livestock_name: &str, trail_name: &str) -> u64 {
    load_trail_runs(livestock_name, trail_name).len() as u64
}
```

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "Add poll state helpers and update trail config functions"
```

---

### Task 3: Update Trail Runner

**Files:**
- Modify: `src/trails/runner.rs:1-105` (rewrite resolve_templates → env merge, update start_trail)

**Step 1: Replace `resolve_templates` with `build_env_vars`**

Remove the entire `resolve_templates` function (lines 14-47). Replace with a 4-layer env var merge function:

```rust
use std::collections::HashMap;
use crate::trails::{Trail, TrailJob, TrailStep};
use crate::types::{Livestock, Barn};

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
) -> Vec<(String, String)> {
    let mut env: HashMap<String, String> = HashMap::new();

    // Layer 1: Auto-injected
    env.insert("NAME".into(), livestock.name.clone());
    env.insert("REPO_PATH".into(), livestock.path.clone());
    env.insert("BRANCH".into(), livestock.branch.as_deref().unwrap_or("main").into());
    env.insert("BARN".into(), barn.host.as_deref().unwrap_or("localhost").into());
    env.insert("BARN_USER".into(), barn.user.as_deref().unwrap_or("root").into());
    env.insert("PROJECT".into(), "".into()); // filled by caller if available
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
```

**Step 2: Rewrite `start_trail` to use new data model**

Replace `start_trail` (lines 50-104) to extract the first job's steps and use the new env merge:

```rust
pub fn start_trail(
    trail: &Trail,
    livestock: &Livestock,
    barn: &Barn,
    project_name: Option<&str>,
) -> Result<(PathBuf, mpsc::Receiver<StepUpdate>, NativeProvider)> {
    // Extract first job (V2: single job only)
    let (job_name, job) = trail.first_job()
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

    // Build per-step env vars (layer merge happens per-step in the provider,
    // but we pass the shared layers here)
    let base_env = build_env_vars(
        trail, job, &job.steps[0], livestock, barn, &timestamp, run_number,
    );

    // Update PROJECT if provided
    let mut env_vars = base_env;
    if let Some(proj) = project_name {
        if let Some(entry) = env_vars.iter_mut().find(|(k, _)| k == "PROJECT") {
            entry.1 = proj.to_string();
        }
    }

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
```

**Step 3: Commit**

```bash
git add src/trails/runner.rs
git commit -m "Replace handlebars templates with 4-layer env var merge"
```

---

### Task 4: Update Provider Trait & Native Provider

**Files:**
- Modify: `src/trails/provider.rs:24-34` (update TrailContext struct)
- Modify: `src/trails/native.rs:41-170` (update step execution loop for per-step env)

**Step 1: Update TrailContext in provider.rs**

Replace the `TrailContext` struct (lines 24-34) to match the new data model. Remove `resolved_steps` (no longer pre-resolved). Add new fields:

```rust
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
```

**Step 2: Update NativeProvider to use job.steps and per-step env merge**

In `native.rs`, update the step execution loop (around lines 41-170):

1. Iterate over `ctx.job.steps` instead of `ctx.resolved_steps`
2. For each step, build the full env map by starting from `ctx.env_vars` and layering on the step's `env:` block
3. Update `STEP_NAME` in the env vars for each step
4. Convert `timeout_minutes` to seconds: `step.timeout_minutes.unwrap_or(1) * 60`

The SSH command construction (lines 58-84) stays mostly the same — it already builds `env_exports` from a `Vec<(String, String)>`. Just ensure it receives the per-step merged env.

Key change in the step loop:

```rust
for (i, step) in ctx.job.steps.iter().enumerate() {
    // Merge step-level env on top of base env
    let mut step_env: Vec<(String, String)> = ctx.env_vars.clone();

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

    // ... rest of SSH execution using step_env and timeout_secs ...
}
```

**Step 3: Verify it compiles**

Run: `cargo build 2>&1 | head -40`
Expected: Errors from app.rs and livestock_detail.rs that still reference old types. Fix in later tasks.

**Step 4: Commit**

```bash
git add src/trails/provider.rs src/trails/native.rs
git commit -m "Update provider trait and native executor for GHA-style steps"
```

---

### Task 5: Update App.rs Trail Execution Callsites

**Files:**
- Modify: `src/app.rs:1083-1098` (start_trail call)
- Modify: `src/app.rs:583-657` (trail update polling — should mostly work)

**Step 1: Update the start_trail call in app.rs**

At line 1085, the call is:
```rust
crate::trails::runner::start_trail(&trail, &livestock, &barn)
```

Add the project_name argument:
```rust
crate::trails::runner::start_trail(&trail, &livestock, &barn, Some(&project.name))
```

Where `project` is whatever the current project context is. Check how the project is available at this callsite — it's likely `app.current_project()` or similar. Read the surrounding context at line 1083 to determine the right accessor.

**Step 2: Verify it compiles**

Run: `cargo build 2>&1 | head -40`

**Step 3: Commit**

```bash
git add src/app.rs
git commit -m "Update trail execution callsite for new runner signature"
```

---

### Task 6: Add Git Polling Worm Auto-Creation

**Files:**
- Modify: `src/config.rs` (add trail-poll worm helpers)
- Modify: `src/app.rs` (extend handle_worm_trigger for trail polls)
- Create: `src/trails/polling.rs` (poll logic: check remote SHA, compare, trigger)

**Step 1: Create `src/trails/polling.rs`**

This module contains the logic that a poll worm executes. It's called by `yeehaw worm exec poll:livestock:trail` which runs the polling check:

```rust
use anyhow::Result;
use crate::config;

/// Check if the remote branch has new commits. Returns true if a trail should trigger.
/// Called by the poll worm's exec command.
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
        "worm": format!("poll:{}:{}", livestock_name, trail_name),
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
```

**Step 2: Add `pub mod polling;` to `src/trails/mod.rs`**

Add after the existing module declarations:
```rust
pub mod polling;
```

**Step 3: Add poll worm auto-creation helpers to config.rs**

Add functions to create/remove poll worms when trails are linked/unlinked:

```rust
/// Create a poll worm for a trail with on:push trigger.
/// Worm name: poll:{livestock}:{trail}
pub fn create_poll_worm(
    livestock_name: &str,
    trail_name: &str,
    poll_interval_secs: u64,
    project_name: Option<&str>,
) -> Result<()> {
    let worm_name = format!("poll:{}:{}", livestock_name, trail_name);

    // Build a cron expression from the poll interval.
    // For intervals < 60s, we use a "per-second" cron hack:
    // Run every minute, but the command itself loops for sub-minute intervals.
    // For simplicity, round to nearest minute for cron and use the yeehaw poll subcommand.
    let schedule = if poll_interval_secs < 60 {
        "* * * * *".to_string()  // every minute
    } else {
        let mins = poll_interval_secs / 60;
        format!("*/{} * * * *", mins)
    };

    let worm = crate::types::Worm {
        name: worm_name,
        command: format!("yeehaw trail poll {} {}", livestock_name, trail_name),
        schedule,
        worm_type: "shell".to_string(),
        enabled: true,
        project: project_name.map(|s| s.to_string()),
        working_dir: None,
    };

    save_worm(&worm)?;
    crate::crontab::sync_crontab()?;
    Ok(())
}

/// Remove a poll worm for a trail.
pub fn remove_poll_worm(livestock_name: &str, trail_name: &str) -> Result<()> {
    let worm_name = format!("poll:{}:{}", livestock_name, trail_name);
    delete_worm(&worm_name)?;
    crate::crontab::sync_crontab()?;
    Ok(())
}
```

**Step 4: Update `link_trail_to_livestock` (config.rs:416-433) to auto-create poll worm**

After the trail is linked, check if it has `on: push` and create the poll worm:

```rust
pub fn link_trail_to_livestock(project_name: &str, livestock_name: &str, trail_name: &str) -> Result<()> {
    // ... existing linking logic ...

    // Auto-create poll worm if trail has on:push trigger
    if let Some(trail) = load_trail(trail_name) {
        if trail.has_push_trigger() {
            let _ = create_poll_worm(
                livestock_name,
                trail_name,
                trail.poll_interval(),
                Some(project_name),
            );
        }
    }

    Ok(())
}
```

**Step 5: Update `unlink_trail_from_livestock` (config.rs:435-450) to remove poll worm**

After unlinking, remove the poll worm:

```rust
pub fn unlink_trail_from_livestock(project_name: &str, livestock_name: &str, trail_name: &str) -> Result<()> {
    // ... existing unlinking logic ...

    // Remove poll worm if it exists
    let _ = remove_poll_worm(livestock_name, trail_name);

    Ok(())
}
```

**Step 6: Extend `handle_worm_trigger` in app.rs to recognize trail poll triggers**

In `src/app.rs`, the `handle_worm_trigger` function (line 1771) reads trigger files and executes worms. For poll triggers (where `trigger == "poll"`), instead of running a worm command, start the trail:

Add after the existing worm trigger handling (around line 1822), a new branch:

```rust
// Check if this is a trail poll trigger
if trigger_type == "poll" {
    if let (Some(livestock_name), Some(trail_name)) = (
        trigger.get("livestock").and_then(|v| v.as_str()),
        trigger.get("trail").and_then(|v| v.as_str()),
    ) {
        // Find livestock, trail, barn and start execution
        // (similar to the manual trail run code at line 1083)
        // ... find project, livestock, trail, barn ...
        // ... call start_trail ...
        return;
    }
}
```

This is the wiring that connects poll detection to trail execution. Look at the existing manual trail run code at `app.rs:1083-1098` for the exact pattern to follow.

**Step 7: Add `trail poll` CLI subcommand to main.rs**

The poll worm's command is `yeehaw trail poll {livestock} {trail}`. Add this subcommand to `src/main.rs` so it can be executed by cron. It should:

1. Load the livestock and its barn config
2. Load the trail config
3. Call `trails::polling::check_and_trigger()` with the right params
4. Exit 0 on success, 1 on error

**Step 8: Commit**

```bash
git add src/trails/polling.rs src/trails/mod.rs src/config.rs src/app.rs src/main.rs
git commit -m "Add git polling infrastructure with auto-created poll worms"
```

---

### Task 7: Add MCP Trail Tools

**Files:**
- Modify: `src/mcp_server.rs:20-358` (add param structs)
- Modify: `src/mcp_server.rs:1373` (add tool methods before ServerHandler impl)

**Step 1: Add parameter structs**

Add these after the existing param structs (around line 358), before the `impl YeehawServer`:

```rust
#[derive(Deserialize, JsonSchema)]
struct GetTrailParams {
    /// Trail name
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateTrailParams {
    /// Trail name
    name: String,
    /// Full trail YAML content (GHA-compatible format)
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateTrailParams {
    /// Trail name to update
    name: String,
    /// New trail YAML content
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct LinkTrailParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
}

#[derive(Deserialize, JsonSchema)]
struct RunTrailParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
}

#[derive(Deserialize, JsonSchema)]
struct ListTrailRunsParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
    /// Max runs to return (default: 20)
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct GetTrailRunParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
    /// Run timestamp (from list_trail_runs)
    run_timestamp: String,
}

#[derive(Deserialize, JsonSchema)]
struct ReadTrailStepLogParams {
    /// Project name
    project: String,
    /// Livestock name
    livestock: String,
    /// Trail name
    trail: String,
    /// Run timestamp
    run_timestamp: String,
    /// Step index (0-based)
    step: usize,
}
```

**Step 2: Add the 11 tool methods**

Add these inside the `#[tool_router] impl YeehawServer` block, after the existing RanchHand tools (around line 1372):

```rust
// ========================================================================
// Trails
// ========================================================================

#[tool(description = "List all trail definitions")]
async fn list_trails(&self) -> Result<CallToolResult, McpError> {
    let trails = config::load_all_trails();
    let names: Vec<&str> = trails.iter().map(|t| t.name.as_str()).collect();
    ok_json(&names)
}

#[tool(description = "Get trail YAML content and metadata")]
async fn get_trail(&self, params: Parameters<GetTrailParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    match config::load_trail(&p.name) {
        Some(trail) => ok_json(&trail),
        None => err_text(&format!("Trail '{}' not found", p.name)),
    }
}

#[tool(description = "Create a new trail from GHA-compatible YAML content")]
async fn create_trail(&self, params: Parameters<CreateTrailParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    // Parse and validate the YAML
    let trail: crate::trails::Trail = match serde_yaml::from_str(&p.content) {
        Ok(t) => t,
        Err(e) => return err_text(&format!("Invalid trail YAML: {}", e)),
    };
    // Verify name matches
    if trail.name != p.name {
        return err_text(&format!("Trail name in YAML ('{}') doesn't match parameter ('{}')", trail.name, p.name));
    }
    // Check it has at least one job with steps
    if trail.jobs.is_empty() {
        return err_text("Trail must have at least one job");
    }
    match config::save_trail(&trail) {
        Ok(_) => ok_text(&format!("Trail '{}' created", p.name)),
        Err(e) => err_text(&format!("Failed to save trail: {}", e)),
    }
}

#[tool(description = "Update an existing trail with new YAML content")]
async fn update_trail(&self, params: Parameters<UpdateTrailParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    // Verify trail exists
    if config::load_trail(&p.name).is_none() {
        return err_text(&format!("Trail '{}' not found", p.name));
    }
    let trail: crate::trails::Trail = match serde_yaml::from_str(&p.content) {
        Ok(t) => t,
        Err(e) => return err_text(&format!("Invalid trail YAML: {}", e)),
    };
    match config::save_trail(&trail) {
        Ok(_) => ok_text(&format!("Trail '{}' updated", p.name)),
        Err(e) => err_text(&format!("Failed to update trail: {}", e)),
    }
}

#[tool(description = "Delete a trail definition")]
async fn delete_trail(&self, params: Parameters<NameParam>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    match config::delete_trail(&p.name) {
        Ok(true) => ok_text(&format!("Trail '{}' deleted", p.name)),
        Ok(false) => err_text(&format!("Trail '{}' not found", p.name)),
        Err(e) => err_text(&format!("Failed to delete trail: {}", e)),
    }
}

#[tool(description = "Link a trail to a livestock (attaches trail for execution)")]
async fn link_trail(&self, params: Parameters<LinkTrailParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    match config::link_trail_to_livestock(&p.project, &p.livestock, &p.trail) {
        Ok(_) => ok_text(&format!("Trail '{}' linked to '{}'", p.trail, p.livestock)),
        Err(e) => err_text(&format!("Failed to link trail: {}", e)),
    }
}

#[tool(description = "Unlink a trail from a livestock")]
async fn unlink_trail(&self, params: Parameters<LinkTrailParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    match config::unlink_trail_from_livestock(&p.project, &p.livestock, &p.trail) {
        Ok(_) => ok_text(&format!("Trail '{}' unlinked from '{}'", p.trail, p.livestock)),
        Err(e) => err_text(&format!("Failed to unlink trail: {}", e)),
    }
}

#[tool(description = "Trigger a trail run on a livestock (executes via SSH on the livestock's barn)")]
async fn run_trail(&self, params: Parameters<RunTrailParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    // Write a trigger file (same mechanism as manual TUI run)
    let now = chrono::Utc::now();
    let filename = format!("mcp-trail-{}--{}--{}.json", p.livestock, p.trail,
                           now.format("%Y-%m-%dT%H-%M-%S"));
    let trigger = serde_json::json!({
        "worm": format!("trail:{}:{}", p.livestock, p.trail),
        "triggered_at": now.to_rfc3339(),
        "trigger": "mcp",
        "livestock": p.livestock,
        "trail": p.trail,
        "project": p.project,
    });
    let trigger_path = config::worm_triggers_dir().join(&filename);
    std::fs::create_dir_all(config::worm_triggers_dir()).map_err(|e| McpError::internal(e.to_string()))?;
    std::fs::write(&trigger_path, serde_json::to_string_pretty(&trigger).unwrap())
        .map_err(|e| McpError::internal(e.to_string()))?;
    ok_text(&format!("Trail '{}' triggered on '{}'. Run ID: {}", p.trail, p.livestock, now.format("%Y-%m-%dT%H-%M-%S")))
}

#[tool(description = "List trail run history for a specific livestock and trail")]
async fn list_trail_runs(&self, params: Parameters<ListTrailRunsParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    let mut runs = config::load_trail_runs(&p.livestock, &p.trail);
    let limit = p.limit.unwrap_or(20) as usize;
    runs.truncate(limit);
    ok_json(&runs)
}

#[tool(description = "Get details of a specific trail run (step statuses, timing, exit codes)")]
async fn get_trail_run(&self, params: Parameters<GetTrailRunParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    let run_dir = config::trail_run_dir_for(&p.livestock, &p.trail, &p.run_timestamp);
    let run_path = run_dir.join("run.json");
    match std::fs::read_to_string(&run_path) {
        Ok(content) => {
            let run: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
            ok_json(&run)
        }
        Err(_) => err_text(&format!("Run not found: {}/{}/{}", p.livestock, p.trail, p.run_timestamp)),
    }
}

#[tool(description = "Read stdout/stderr log for a specific step in a trail run")]
async fn read_trail_step_log(&self, params: Parameters<ReadTrailStepLogParams>) -> Result<CallToolResult, McpError> {
    let p = params.0;
    let run_dir = config::trail_run_dir_for(&p.livestock, &p.trail, &p.run_timestamp);
    match config::load_trail_step_log(&run_dir, p.step) {
        Some(log) => ok_text(&log),
        None => err_text(&format!("Step log not found: step {} in {}/{}/{}", p.step, p.livestock, p.trail, p.run_timestamp)),
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo build 2>&1 | head -40`

**Step 4: Commit**

```bash
git add src/mcp_server.rs
git commit -m "Add 11 MCP trail tools for full lifecycle management"
```

---

### Task 8: Update TUI Trail Creation Wizard

**Files:**
- Modify: `src/views/livestock_detail.rs:44-617` (WizardMode enum + wizard methods)

**Step 1: Add new wizard stages for trigger configuration**

Update `WizardMode` enum (line 44) to add trigger stages:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum WizardMode {
    Inactive,
    TrailName,
    TrailDescription,      // kept for job name (maps to first job key)
    TriggerOnPush,         // NEW: "Trigger on push? [y/n]"
    TriggerBranches,       // NEW: "Which branches? [main]"
    StepName,
    StepCommand,
    StepTimeout,
}
```

**Step 2: Add wizard state fields for trigger config**

In the `LivestockDetailView` struct (lines 54-73), add:

```rust
    new_trail_trigger_push: bool,
    new_trail_branches: String,
```

**Step 3: Update `handle_wizard_input` to process trigger stages**

After `TrailDescription` stage, route to `TriggerOnPush`:

```rust
WizardMode::TrailDescription => {
    self.new_trail_description = self.text_input.value().to_string();
    self.wizard_mode = WizardMode::TriggerOnPush;
    self.text_input = TextInput::new("n");  // default no
}
WizardMode::TriggerOnPush => {
    let val = self.text_input.value().to_lowercase();
    self.new_trail_trigger_push = val == "y" || val == "yes";
    if self.new_trail_trigger_push {
        self.wizard_mode = WizardMode::TriggerBranches;
        self.text_input = TextInput::new("main");
    } else {
        self.wizard_mode = WizardMode::StepName;
        self.text_input = TextInput::new("");
    }
}
WizardMode::TriggerBranches => {
    self.new_trail_branches = self.text_input.value().to_string();
    self.wizard_mode = WizardMode::StepName;
    self.text_input = TextInput::new("");
}
```

**Step 4: Update `finish_trail_wizard` to emit GHA-format Trail**

Replace the `finish_trail_wizard` method (lines 494-507):

```rust
fn finish_trail_wizard(&mut self) -> LivestockAction {
    let mut steps = Vec::new();
    for old_step in &self.new_trail_steps {
        steps.push(crate::trails::TrailStep {
            name: old_step.name.clone(),
            run: old_step.run.clone(),
            env: None,
            timeout_minutes: Some(old_step.timeout_minutes.unwrap_or(1)),
        });
    }

    let job = crate::trails::TrailJob {
        runs_on: "native".to_string(),
        env: None,
        steps,
    };

    let on = if self.new_trail_trigger_push {
        let branches = if self.new_trail_branches.is_empty() {
            None
        } else {
            Some(self.new_trail_branches.split(',')
                .map(|s| s.trim().to_string())
                .collect())
        };
        Some(crate::trails::TrailTrigger {
            push: Some(crate::trails::PushTrigger {
                branches,
                poll_interval: None,
            }),
        })
    } else {
        None
    };

    let mut jobs = std::collections::HashMap::new();
    jobs.insert(self.new_trail_name.clone(), job);

    let trail = crate::trails::Trail {
        name: self.new_trail_name.clone(),
        on,
        env: None,
        jobs,
    };

    self.wizard_mode = WizardMode::Inactive;
    LivestockAction::SaveNewTrail(trail)
}
```

Note: The `new_trail_steps` Vec now holds the new `TrailStep` type. Update `handle_wizard_input` for the StepTimeout stage to create a step with the new fields (`timeout_minutes` instead of `timeout`).

**Step 5: Update wizard rendering to show trigger stages**

In `render_trail_wizard` (lines 509-617), add display cases for `TriggerOnPush` and `TriggerBranches` stages, showing the appropriate prompts ("Trigger on push to remote? (y/n)", "Which branches? (comma-separated)").

**Step 6: Verify it compiles and the wizard flow works**

Run: `cargo build`

**Step 7: Commit**

```bash
git add src/views/livestock_detail.rs
git commit -m "Update trail creation wizard for GHA-format with trigger configuration"
```

---

### Task 9: Wire MCP Trail Triggers into App Event Loop

**Files:**
- Modify: `src/app.rs:1771-1830` (extend handle_worm_trigger)

**Step 1: Extend handle_worm_trigger to handle trail triggers from MCP and polls**

The `handle_worm_trigger` function at line 1771 currently only handles worm execution. Extend it to recognize trail triggers (from polls and MCP):

After the trigger file is parsed (around line 1797), add:

```rust
// Check if this is a trail trigger (from poll or MCP)
if trigger_type == "poll" || trigger_type == "mcp" {
    if let (Some(livestock_name), Some(trail_name)) = (
        trigger.get("livestock").and_then(|v| v.as_str()),
        trigger.get("trail").and_then(|v| v.as_str()),
    ) {
        let project_name = trigger.get("project").and_then(|v| v.as_str());

        // Check if trail is already running (skip policy)
        if app.trail_run_receiver.is_some() {
            // Trail already running, skip
            return;
        }

        // Find the livestock, trail, and barn
        // Search across projects for the livestock
        let projects = config::load_projects();
        for project in &projects {
            if let Some(ls) = project.livestock.iter().find(|l| l.name == livestock_name) {
                if let Some(trail) = config::load_trail(trail_name) {
                    if let Some(barn_name) = &ls.barn {
                        if let Some(barn) = config::load_barns().into_iter().find(|b| &b.name == barn_name) {
                            match crate::trails::runner::start_trail(&trail, ls, &barn, Some(&project.name)) {
                                Ok((run_dir, rx, provider)) => {
                                    app.trail_view.enter(&trail, ls);
                                    app.trail_view.start_run(&trail);
                                    app.trail_run_receiver = Some(rx);
                                    app.trail_provider = Some(provider);
                                    app.trail_run_dir = Some(run_dir);
                                }
                                Err(e) => {
                                    app.error = Some(format!("Failed to start trail: {}", e));
                                }
                            }
                            return;
                        }
                    }
                }
            }
        }
        return;
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo build`

**Step 3: Commit**

```bash
git add src/app.rs
git commit -m "Wire poll and MCP trail triggers into app event loop"
```

---

### Task 10: Add `trail poll` CLI Subcommand

**Files:**
- Modify: `src/main.rs` (add trail poll subcommand)

**Step 1: Add the subcommand handler**

Find the CLI argument parsing section in main.rs. Add a `trail poll {livestock} {trail}` subcommand that:

1. Loads the livestock from config (search all projects)
2. Loads the trail from config
3. Finds the barn for the livestock
4. Gets the repo URL (from livestock.repo or detect from barn)
5. Gets the branch (from trail's `on.push.branches[0]` or livestock.branch or "main")
6. Calls `trails::polling::check_and_trigger()` with all params
7. Prints result and exits

```rust
// In the CLI match block:
("trail", Some(trail_matches)) => {
    match trail_matches.subcommand() {
        ("poll", Some(poll_matches)) => {
            let livestock_name = poll_matches.value_of("livestock").unwrap();
            let trail_name = poll_matches.value_of("trail").unwrap();

            // Find livestock across all projects
            let projects = config::load_projects();
            let mut found = None;
            for project in &projects {
                if let Some(ls) = project.livestock.iter().find(|l| l.name == livestock_name) {
                    if let Some(barn_name) = &ls.barn {
                        if let Some(barn) = config::load_barns().into_iter().find(|b| &b.name == barn_name) {
                            found = Some((ls.clone(), barn, project.name.clone()));
                            break;
                        }
                    }
                }
            }

            let (livestock, barn, _project) = found.unwrap_or_else(|| {
                eprintln!("Livestock '{}' not found or has no barn", livestock_name);
                std::process::exit(1);
            });

            let trail = config::load_trail(trail_name).unwrap_or_else(|| {
                eprintln!("Trail '{}' not found", trail_name);
                std::process::exit(1);
            });

            let repo_url = livestock.repo.as_deref().unwrap_or_else(|| {
                eprintln!("Livestock '{}' has no repo URL configured", livestock_name);
                std::process::exit(1);
            });

            let branch = trail.push_branches()
                .and_then(|b| b.first())
                .map(|s| s.as_str())
                .or(livestock.branch.as_deref())
                .unwrap_or("main");

            match crate::trails::polling::check_and_trigger(
                livestock_name,
                trail_name,
                repo_url,
                branch,
                barn.host.as_deref().unwrap_or(&barn.name),
                barn.user.as_deref().unwrap_or("root"),
                barn.port.unwrap_or(22),
                barn.identity_file.as_deref().unwrap_or(""),
            ) {
                Ok(true) => println!("New commits detected, trail triggered"),
                Ok(false) => println!("No new commits"),
                Err(e) => {
                    eprintln!("Poll error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        _ => {}
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo build`

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "Add 'trail poll' CLI subcommand for cron-based git polling"
```

---

### Task 11: Final Integration Test & Cleanup

**Files:**
- All modified files

**Step 1: Full build verification**

Run: `cargo build 2>&1`
Expected: Clean compilation with no errors.

**Step 2: Verify MCP server starts**

Run: `echo '{"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{}},"id":1}' | cargo run -- mcp 2>/dev/null | head -5`
Expected: JSON response with tool list including the new trail tools.

**Step 3: Verify trail YAML parsing**

Create a test trail file and verify it parses:
```bash
mkdir -p ~/.yeehaw/trails
cat > /tmp/test-trail.yaml << 'EOF'
name: test-deploy
on:
  push:
    branches: [main]
jobs:
  deploy:
    runs-on: native
    steps:
      - name: Pull code
        run: echo "pulling $BRANCH"
      - name: Build
        run: echo "building"
        timeout-minutes: 5
EOF
```

**Step 4: Clean up any TODO comments or dead code from old format**

Search for any remaining references to `{{handlebars}}`, `provider: native`, old `TrailStep.timeout` (seconds), or `resolve_templates` and remove them.

Run: `grep -rn '{{' src/trails/ src/views/trail_view.rs src/views/livestock_detail.rs`
Expected: No results (all handlebars removed).

Run: `grep -rn 'resolve_templates\|provider.*native\|default_provider\|default_timeout' src/`
Expected: No results from old code (some legitimate uses of "native" are fine in the new code).

**Step 5: Commit final cleanup**

```bash
git add -A
git commit -m "Clean up old trail format references and verify integration"
```
