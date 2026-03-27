# Trails Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a "Trails" feature to Yeehaw that lets users define reusable step-based deployment pipelines in YAML, attach them to livestock, and execute them via SSH on barns with real-time streaming output in the TUI.

**Architecture:** Trails are a new `src/trails/` module with types, a provider trait, a native SSH executor, a runner orchestrator, and a history loader. The TUI gets two new views (trail execution + trail history) following the existing split-panel pattern from wiki/issues. Livestock gains a `trails: Vec<String>` field. Trail YAML files live in `~/.yeehaw/trails/`, run history in `~/.yeehaw/trail-runs/`.

**Tech Stack:** Rust, ratatui (TUI), serde_yaml (config), tokio (async SSH streaming), crossterm (terminal events). Follows existing patterns from worms, wiki_view, and barn SSH execution.

**Design doc:** `docs/plans/2026-02-23-trails-design.md`

---

### Task 1: Trail Types & Data Structures

**Files:**
- Create: `src/trails/mod.rs`
- Modify: `src/types.rs:131-142` (Livestock struct)
- Modify: `src/types.rs:350-376` (AppView enum)
- Modify: `src/types.rs:397-404` (PartialEq impls)

**Step 1: Create `src/trails/mod.rs` with core types**

```rust
use serde::{Deserialize, Serialize};

pub mod provider;
pub mod native;
pub mod runner;
pub mod history;

// ============================================================================
// Trail Definition (loaded from ~/.yeehaw/trails/*.yaml)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trail {
    pub name: String,
    pub description: Option<String>,
    #[serde(default = "default_provider")]
    pub provider: String,
    pub steps: Vec<TrailStep>,
}

fn default_provider() -> String {
    "native".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailStep {
    pub name: String,
    pub run: String,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    60
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
    pub status: String,   // "running", "success", "failed", "cancelled"
    pub steps: Vec<TrailStepRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailStepRun {
    pub name: String,
    pub status: String,   // "pending", "running", "success", "failed"
    pub exit_code: Option<i32>,
    pub started_at: Option<String>,
    pub duration_ms: Option<u64>,
}

impl PartialEq for Trail {
    fn eq(&self, other: &Self) -> bool { self.name == other.name }
}

impl PartialEq for TrailRun {
    fn eq(&self, other: &Self) -> bool {
        self.trail == other.trail && self.started_at == other.started_at
    }
}
```

**Step 2: Add `trails` field to Livestock struct**

In `src/types.rs`, add to the `Livestock` struct (after `k8s_metadata` field, line 141):

```rust
    #[serde(default)]
    pub trails: Vec<String>,
```

**Step 3: Add Trail and TrailRun AppView variants**

In `src/types.rs`, add to the `AppView` enum (after `WormRunLog` variant, line 374):

```rust
    Trail {
        project: Project,
        livestock: Livestock,
        trail: crate::trails::Trail,
        source: String,
        source_barn: Option<Barn>,
    },
    TrailHistory {
        project: Project,
        livestock: Livestock,
        trail: crate::trails::Trail,
        source: String,
        source_barn: Option<Barn>,
    },
```

**Step 4: Register the trails module**

In `src/main.rs`, add at the top with other `mod` declarations:

```rust
mod trails;
```

**Step 5: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: Compiles (with warnings about unused modules, which is fine).

**Step 6: Commit**

```bash
git add src/trails/mod.rs src/types.rs src/main.rs
git commit -m "feat(trails): add core Trail types and data structures"
```

---

### Task 2: Config Loading & Directory Setup

**Files:**
- Modify: `src/config.rs:18-29` (path functions)
- Modify: `src/config.rs:46-66` (ensure_config_dirs)
- Add new functions to: `src/config.rs` (after worm run functions, ~line 365)

**Step 1: Add trail path functions**

In `src/config.rs`, after `bin_dir()` (line 29), add:

```rust
pub fn trails_dir() -> PathBuf { yeehaw_dir().join("trails") }
pub fn trail_runs_dir() -> PathBuf { yeehaw_dir().join("trail-runs") }

pub fn trail_run_dir_for(livestock_name: &str, trail_name: &str, timestamp: &str) -> PathBuf {
    trail_runs_dir().join(format!("{}--{}--{}", livestock_name, trail_name, timestamp))
}
```

**Step 2: Add trails dirs to ensure_config_dirs**

In `src/config.rs`, inside the `ensure_config_dirs()` array (after `bin_dir()`, line 59), add:

```rust
        trails_dir(),
        trail_runs_dir(),
```

**Step 3: Add trail YAML loading functions**

In `src/config.rs`, after the worm run functions (after `save_worm_run`), add:

```rust
// ============================================================================
// Trails
// ============================================================================

pub fn load_trail(name: &str) -> Option<crate::trails::Trail> {
    let path = trails_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_yaml::from_str(&content).ok()
}

pub fn load_all_trails() -> Vec<crate::trails::Trail> {
    ensure_config_dirs();
    let dir = trails_dir();
    if !dir.exists() {
        return vec![];
    }

    let mut trails = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(trail) = serde_yaml::from_str::<crate::trails::Trail>(&content) {
                        trails.push(trail);
                    }
                }
            }
        }
    }
    trails.sort_by(|a, b| a.name.cmp(&b.name));
    trails
}

pub fn load_trails_for_livestock(livestock: &Livestock) -> Vec<crate::trails::Trail> {
    livestock.trails.iter()
        .filter_map(|name| load_trail(name))
        .collect()
}

pub fn save_trail(trail: &crate::trails::Trail) -> Result<()> {
    ensure_config_dirs();
    validate_name(&trail.name, "trail")?;
    let path = trails_dir().join(format!("{}.yaml", trail.name));
    let content = serde_yaml::to_string(trail).context("Failed to serialize trail")?;
    fs::write(&path, content).context("Failed to write trail file")?;
    Ok(())
}

pub fn delete_trail(name: &str) -> Result<bool> {
    validate_name(name, "trail")?;
    let path = trails_dir().join(format!("{}.yaml", name));
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).context("Failed to delete trail")?;
    Ok(true)
}
```

**Step 4: Add trail run persistence functions**

```rust
// ============================================================================
// Trail Runs
// ============================================================================

pub fn save_trail_run(run: &crate::trails::TrailRun, run_dir: &std::path::Path) -> Result<()> {
    if !run_dir.exists() {
        fs::create_dir_all(run_dir).context("Failed to create trail run directory")?;
    }
    let path = run_dir.join("run.json");
    let content = serde_json::to_string_pretty(run).context("Failed to serialize trail run")?;
    fs::write(&path, content).context("Failed to write trail run")?;
    Ok(())
}

pub fn load_trail_runs(livestock_name: &str, trail_name: &str) -> Vec<crate::trails::TrailRun> {
    let dir = trail_runs_dir();
    if !dir.exists() {
        return vec![];
    }

    let prefix = format!("{}--{}--", livestock_name, trail_name);
    let mut runs = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) && entry.path().is_dir() {
                let run_path = entry.path().join("run.json");
                if let Ok(content) = fs::read_to_string(&run_path) {
                    if let Ok(run) = serde_json::from_str::<crate::trails::TrailRun>(&content) {
                        runs.push(run);
                    }
                }
            }
        }
    }

    // Most recent first
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    runs
}

pub fn load_trail_step_log(run_dir: &std::path::Path, step_index: usize) -> Option<String> {
    let log_path = run_dir.join(format!("step-{}.log", step_index));
    fs::read_to_string(&log_path).ok()
}
```

**Step 5: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: Compiles. Note: `serde_json` is already a dependency in Cargo.toml.

**Step 6: Commit**

```bash
git add src/config.rs
git commit -m "feat(trails): add config loading and trail run persistence"
```

---

### Task 3: Provider Trait & Template Engine

**Files:**
- Create: `src/trails/provider.rs`
- Add template resolution to: `src/trails/runner.rs` (partial, runner fully built in Task 4)

**Step 1: Create the provider trait**

Create `src/trails/provider.rs`:

```rust
use anyhow::Result;
use tokio::sync::mpsc;

use super::{Trail, TrailStep};
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
    /// Steps with templates already resolved.
    pub resolved_steps: Vec<TrailStep>,
    /// Directory where run artifacts (run.json, step-N.log) are written.
    pub run_dir: std::path::PathBuf,
}

pub trait TrailProvider: Send + Sync {
    /// Human-readable provider name (e.g., "native", "github-actions").
    fn name(&self) -> &str;

    /// Execute a trail. Returns a receiver that streams step updates in real-time.
    /// The provider owns the execution; the caller reads updates and persists them.
    fn execute(
        &self,
        ctx: TrailContext,
    ) -> Result<mpsc::Receiver<StepUpdate>>;

    /// Request cancellation of the currently running trail.
    fn cancel(&self) -> Result<()>;
}
```

**Step 2: Create `src/trails/runner.rs` with template resolution**

```rust
use anyhow::{bail, Result};
use regex::Regex;

use super::{Trail, TrailStep};
use crate::types::{Barn, Livestock};

/// Resolve template variables in a trail's steps.
pub fn resolve_templates(
    trail: &Trail,
    livestock: &Livestock,
    barn: &Barn,
) -> Result<Vec<TrailStep>> {
    let mut resolved = Vec::new();

    for step in &trail.steps {
        let run = step.run
            .replace("{{name}}", &livestock.name)
            .replace("{{repo_path}}", &livestock.path)
            .replace("{{branch}}", livestock.branch.as_deref().unwrap_or("main"))
            .replace("{{barn}}", &barn.name)
            .replace("{{barn_user}}", barn.user.as_deref().unwrap_or("root"));

        // Check for unresolved variables
        let re = Regex::new(r"\{\{(\w+)\}\}").unwrap();
        if let Some(cap) = re.captures(&run) {
            bail!(
                "Unresolved template variable '{{{{{}}}}}' in step '{}'",
                &cap[1],
                step.name,
            );
        }

        resolved.push(TrailStep {
            name: step.name.clone(),
            run,
            timeout: step.timeout,
        });
    }

    Ok(resolved)
}
```

**Step 3: Add `regex` dependency**

Run: `cargo add regex` (or add `regex = "1"` to Cargo.toml under `[dependencies]`).

**Step 4: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: Compiles.

**Step 5: Commit**

```bash
git add src/trails/provider.rs src/trails/runner.rs Cargo.toml Cargo.lock
git commit -m "feat(trails): add provider trait and template resolution"
```

---

### Task 4: NativeProvider (SSH Executor)

**Files:**
- Create: `src/trails/native.rs`
- Modify: `src/trails/runner.rs` (add full orchestration)

**Step 1: Create the NativeProvider**

Create `src/trails/native.rs`:

```rust
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

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

    fn execute(
        &self,
        ctx: TrailContext,
    ) -> Result<mpsc::Receiver<StepUpdate>> {
        let (tx, rx) = mpsc::channel(256);
        let cancelled = self.cancelled.clone();

        // Reset cancellation flag
        cancelled.store(false, Ordering::SeqCst);

        let barn = ctx.barn;
        let steps = ctx.resolved_steps;
        let run_dir = ctx.run_dir;

        std::thread::spawn(move || {
            for (i, step) in steps.iter().enumerate() {
                if cancelled.load(Ordering::SeqCst) {
                    let _ = tx.blocking_send(StepUpdate {
                        step_index: i,
                        status: StepStatus::Failed { exit_code: -1 },
                        output_line: Some("Cancelled by user".to_string()),
                    });
                    break;
                }

                // Signal step is running
                let _ = tx.blocking_send(StepUpdate {
                    step_index: i,
                    status: StepStatus::Running,
                    output_line: None,
                });

                // Build SSH command
                let host = barn.host.as_deref().unwrap_or(&barn.name);
                let user = barn.user.as_deref().unwrap_or("root");
                let port = barn.port.unwrap_or(22);

                let mut cmd = Command::new("ssh");
                cmd.arg("-p").arg(port.to_string());

                if let Some(ref key) = barn.identity_file {
                    cmd.arg("-i").arg(key);
                }

                cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");
                cmd.arg("-o").arg(format!("ConnectTimeout={}", 10));
                cmd.arg(format!("{}@{}", user, host));
                cmd.arg(&step.run);

                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());

                // Open log file for this step
                let log_path = run_dir.join(format!("step-{}.log", i));
                let mut log_file = std::fs::File::create(&log_path).ok();

                match cmd.spawn() {
                    Ok(mut child) => {
                        // Stream stdout
                        if let Some(stdout) = child.stdout.take() {
                            let reader = BufReader::new(stdout);
                            for line in reader.lines() {
                                if cancelled.load(Ordering::SeqCst) {
                                    let _ = child.kill();
                                    break;
                                }
                                if let Ok(line) = line {
                                    // Write to log file
                                    if let Some(ref mut f) = log_file {
                                        use std::io::Write;
                                        let _ = writeln!(f, "{}", line);
                                    }
                                    // Send to TUI
                                    let _ = tx.blocking_send(StepUpdate {
                                        step_index: i,
                                        status: StepStatus::Running,
                                        output_line: Some(line),
                                    });
                                }
                            }
                        }

                        // Also capture stderr
                        if let Some(stderr) = child.stderr.take() {
                            let reader = BufReader::new(stderr);
                            for line in reader.lines() {
                                if let Ok(line) = line {
                                    if let Some(ref mut f) = log_file {
                                        use std::io::Write;
                                        let _ = writeln!(f, "[stderr] {}", line);
                                    }
                                    let _ = tx.blocking_send(StepUpdate {
                                        step_index: i,
                                        status: StepStatus::Running,
                                        output_line: Some(format!("[stderr] {}", line)),
                                    });
                                }
                            }
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
                            output_line: Some(format!("SSH failed: {}", e)),
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
```

**Step 2: Extend `src/trails/runner.rs` with full orchestration**

Add to `src/trails/runner.rs` (after the existing `resolve_templates` function):

```rust
use std::path::PathBuf;

use super::{Trail, TrailRun, TrailStepRun};
use super::provider::{StepStatus, StepUpdate, TrailContext};
use super::native::NativeProvider;
use crate::config;
use crate::types::{Barn, Livestock};

/// Start a trail execution. Returns the run directory and a receiver for updates.
pub fn start_trail(
    trail: &Trail,
    livestock: &Livestock,
    barn: &Barn,
) -> Result<(PathBuf, tokio::sync::mpsc::Receiver<StepUpdate>, NativeProvider)> {
    // Resolve templates
    let resolved_steps = resolve_templates(trail, livestock, barn)?;

    // Create run directory
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let run_dir = config::trail_run_dir_for(&livestock.name, &trail.name, &timestamp);
    std::fs::create_dir_all(&run_dir)?;

    // Create initial run record
    let run = TrailRun {
        livestock: livestock.name.clone(),
        trail: trail.name.clone(),
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
        status: "running".to_string(),
        steps: trail.steps.iter().map(|s| TrailStepRun {
            name: s.name.clone(),
            status: "pending".to_string(),
            exit_code: None,
            started_at: None,
            duration_ms: None,
        }).collect(),
    };
    config::save_trail_run(&run, &run_dir)?;

    // Create context
    let ctx = TrailContext {
        livestock: livestock.clone(),
        barn: barn.clone(),
        trail: trail.clone(),
        resolved_steps,
        run_dir: run_dir.clone(),
    };

    // Execute via native provider
    let provider = NativeProvider::new();
    let rx = provider.execute(ctx)?;

    Ok((run_dir, rx, provider))
}
```

**Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`
Expected: Compiles.

**Step 4: Commit**

```bash
git add src/trails/native.rs src/trails/runner.rs
git commit -m "feat(trails): add NativeProvider SSH executor and runner orchestration"
```

---

### Task 5: Trail History Loading

**Files:**
- Create: `src/trails/history.rs`

**Step 1: Create `src/trails/history.rs`**

```rust
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
        .replace(':', "-")
        .replace('T', "T")
        .chars()
        .take(19) // "2026-02-23T14-30-00"
        .collect::<String>();
    config::trail_run_dir_for(&run.livestock, &run.trail, &timestamp)
}

/// Load the log content for a specific step of a trail run.
pub fn load_step_log(run: &TrailRun, step_index: usize) -> Option<String> {
    let dir = run_dir_for(run);
    config::load_trail_step_log(&dir, step_index)
}
```

**Step 2: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

**Step 3: Commit**

```bash
git add src/trails/history.rs
git commit -m "feat(trails): add trail run history loading"
```

---

### Task 6: Trail Header Component

**Files:**
- Create: `src/components/trail_header.rs`
- Modify: `src/components/mod.rs` (add module)

**Step 1: Create the trail header component**

Create `src/components/trail_header.rs`, following the pattern from `worm_header.rs` but with a trail/path-themed ASCII art:

```rust
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::trails::Trail;
use crate::types::Livestock;

const BRAND_COLOR: Color = Color::Rgb(212, 160, 32);

/// DJB2 hash for seeding the PRNG.
fn djb2_hash(s: &str) -> u32 {
    let mut hash: u32 = 5381;
    for b in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u32);
    }
    hash
}

/// Mulberry32 PRNG.
fn mulberry32(state: &mut u32) -> u32 {
    *state = state.wrapping_add(0x6D2B79F5);
    let mut z = *state;
    z = (z ^ (z >> 15)).wrapping_mul(z | 1);
    z ^= z.wrapping_add((z ^ (z >> 7)).wrapping_mul(z | 61));
    z ^ (z >> 14)
}

const TRAIL_CHARS: &[char] = &['.', ':', '~', '-', '='];

/// Generate a trail path art — a winding path across the grid.
fn generate_trail_art(name: &str, width: usize, height: usize) -> Vec<Vec<char>> {
    let mut grid = vec![vec![' '; width]; height];
    let mut state = djb2_hash(name);

    let mut y = (mulberry32(&mut state) as usize % height).clamp(1, height.saturating_sub(2));

    for x in 0..width {
        let ch = TRAIL_CHARS[mulberry32(&mut state) as usize % TRAIL_CHARS.len()];
        if y > 0 && y < height {
            grid[y][x] = ch;
        }

        // Occasionally shift y up or down for a winding effect
        if mulberry32(&mut state) % 3 == 0 {
            let direction: i32 = if mulberry32(&mut state) % 2 == 0 { 1 } else { -1 };
            y = (y as i32 + direction).clamp(0, (height as i32) - 1) as usize;
        }
    }

    grid
}

pub fn render_trail_header(
    frame: &mut Frame,
    area: Rect,
    trail: &Trail,
    livestock: &Livestock,
) {
    let art_width = 30usize.min(area.width as usize);
    let art_height = 4usize.min(area.height.saturating_sub(5) as usize);

    let grid = generate_trail_art(&trail.name, art_width, art_height);

    let mut lines: Vec<Line> = Vec::new();

    let trail_color = BRAND_COLOR;

    for row in &grid {
        let spans: Vec<Span> = row
            .iter()
            .map(|&ch| {
                if ch == ' ' {
                    Span::styled(" ", Style::default())
                } else {
                    Span::styled(ch.to_string(), Style::default().fg(trail_color))
                }
            })
            .collect();
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled(" Trail:     ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            trail.name.clone(),
            Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD),
        ),
    ]));

    if let Some(ref desc) = trail.description {
        lines.push(Line::from(vec![
            Span::styled(" Desc:      ", Style::default().fg(Color::DarkGray)),
            Span::styled(desc.clone(), Style::default().fg(Color::White)),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled(" Livestock:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(livestock.name.clone(), Style::default().fg(Color::White)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Steps:     ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", trail.steps.len()),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled(" Provider:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(trail.provider.clone(), Style::default().fg(Color::White)),
    ]));

    let total = lines.len().min(area.height as usize);
    let visible: Vec<Line> = lines.into_iter().take(total).collect();
    let paragraph = Paragraph::new(visible);
    frame.render_widget(paragraph, area);
}
```

**Step 2: Register the module**

In `src/components/mod.rs`, add:

```rust
pub mod trail_header;
```

**Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

**Step 4: Commit**

```bash
git add src/components/trail_header.rs src/components/mod.rs
git commit -m "feat(trails): add trail header component with path art"
```

---

### Task 7: Trail Step List Component (with pulse animation)

**Files:**
- Create: `src/components/trail_steps.rs`
- Modify: `src/components/mod.rs` (add module)

**Step 1: Create the step list component**

Create `src/components/trail_steps.rs`:

```rust
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::trails::provider::StepStatus;

pub struct StepItem {
    pub name: String,
    pub status: StepStatus,
}

pub struct StepListState {
    pub selected: usize,
}

impl StepListState {
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    pub fn select_next(&mut self, count: usize) {
        if count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

/// Render the step status indicator character.
/// `tick` is a frame counter used for the pulse animation.
fn status_indicator(status: &StepStatus, tick: u64) -> (char, Color) {
    match status {
        StepStatus::Pending => ('\u{2591}', Color::DarkGray),  // ░
        StepStatus::Running => {
            // Pulse between ░ and ▒ every ~500ms (assuming ~10 fps, toggle every 5 ticks)
            if (tick / 5) % 2 == 0 {
                ('\u{2591}', Color::Yellow)   // ░
            } else {
                ('\u{2592}', Color::Yellow)   // ▒
            }
        }
        StepStatus::Success => ('\u{2713}', Color::Green),     // ✓
        StepStatus::Failed { .. } => ('\u{2717}', Color::Red), // ✗
        StepStatus::Skipped => ('\u{2591}', Color::DarkGray),  // ░ (same as pending)
    }
}

/// Render the step list into the given area.
pub fn render_step_list(
    frame: &mut Frame,
    area: Rect,
    steps: &[StepItem],
    state: &StepListState,
    focused: bool,
    tick: u64,
) {
    let mut lines: Vec<Line> = Vec::new();

    for (i, step) in steps.iter().enumerate() {
        let is_selected = i == state.selected;
        let (indicator, indicator_color) = status_indicator(&step.status, tick);

        let selector = if is_selected && focused { "\u{203A} " } else { "  " }; // › or space
        let selector_color = if focused { Color::White } else { Color::DarkGray };

        let name_color = match step.status {
            StepStatus::Success => Color::Green,
            StepStatus::Failed { .. } => Color::Red,
            StepStatus::Running => Color::Yellow,
            _ => Color::DarkGray,
        };

        let name_style = if is_selected && focused {
            Style::default().fg(name_color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(name_color)
        };

        lines.push(Line::from(vec![
            Span::styled(selector, Style::default().fg(selector_color)),
            Span::styled(
                format!("{} ", indicator),
                Style::default().fg(indicator_color),
            ),
            Span::styled(step.name.clone(), name_style),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}
```

**Step 2: Register the module**

In `src/components/mod.rs`, add:

```rust
pub mod trail_steps;
```

**Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

**Step 4: Commit**

```bash
git add src/components/trail_steps.rs src/components/mod.rs
git commit -m "feat(trails): add step list component with pulse animation"
```

---

### Task 8: Trail View (Split Panel TUI)

**Files:**
- Create: `src/views/trail_view.rs`
- Modify: `src/views/mod.rs` (add module)

**Step 1: Create the trail view**

Create `src/views/trail_view.rs`:

```rust
use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::panel::Panel;
use crate::components::trail_header;
use crate::components::trail_steps::{self, StepItem, StepListState};
use crate::trails::{Trail, TrailRun, TrailStepRun};
use crate::trails::provider::StepStatus;
use crate::types::{Livestock, Barn};

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Steps,
    Output,
}

pub enum TrailViewAction {
    None,
    Back,
    RunTrail,
    CancelTrail,
    ShowHistory,
}

pub struct TrailView {
    focused_panel: FocusedPanel,
    step_state: StepListState,
    output_scroll: usize,
    tick: u64,
    /// Live step statuses, updated from the execution receiver.
    pub step_statuses: Vec<StepStatus>,
    /// Live output lines per step.
    pub step_outputs: Vec<Vec<String>>,
    /// Whether a trail is currently running.
    pub running: bool,
    /// Auto-follow: track the currently running step.
    pub auto_follow: bool,
}

impl TrailView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Steps,
            step_state: StepListState::new(),
            output_scroll: 0,
            tick: 0,
            step_statuses: vec![],
            step_outputs: vec![],
            running: false,
            auto_follow: true,
        }
    }

    /// Initialize for a trail (reset state).
    pub fn enter(&mut self, trail: &Trail) {
        self.step_statuses = vec![StepStatus::Pending; trail.steps.len()];
        self.step_outputs = vec![vec![]; trail.steps.len()];
        self.step_state = StepListState::new();
        self.output_scroll = 0;
        self.running = false;
        self.auto_follow = true;
    }

    /// Call this on each TUI tick to advance animation.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        // Auto-follow: select the currently running step
        if self.auto_follow && self.running {
            for (i, status) in self.step_statuses.iter().enumerate() {
                if *status == StepStatus::Running {
                    self.step_state.selected = i;
                    // Auto-scroll output to bottom
                    if let Some(lines) = self.step_outputs.get(i) {
                        self.output_scroll = lines.len().saturating_sub(20);
                    }
                    break;
                }
            }
        }
    }

    /// Process a step update from the execution receiver.
    pub fn apply_update(&mut self, step_index: usize, status: StepStatus, output_line: Option<String>) {
        if step_index < self.step_statuses.len() {
            self.step_statuses[step_index] = status;
        }
        if let Some(line) = output_line {
            if step_index < self.step_outputs.len() {
                self.step_outputs[step_index].push(line);
            }
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, trail: &Trail) -> TrailViewAction {
        match key {
            KeyCode::Esc | KeyCode::Char('q') => return TrailViewAction::Back,
            KeyCode::Char('r') if !self.running => return TrailViewAction::RunTrail,
            KeyCode::Char('c') if self.running => return TrailViewAction::CancelTrail,
            KeyCode::Char('h') => return TrailViewAction::ShowHistory,
            KeyCode::Tab => {
                self.focused_panel = match self.focused_panel {
                    FocusedPanel::Steps => FocusedPanel::Output,
                    FocusedPanel::Output => FocusedPanel::Steps,
                };
                self.auto_follow = false;
            }
            _ => {}
        }

        match self.focused_panel {
            FocusedPanel::Steps => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.step_state.select_next(trail.steps.len());
                        self.output_scroll = 0;
                        self.auto_follow = false;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.step_state.select_prev();
                        self.output_scroll = 0;
                        self.auto_follow = false;
                    }
                    _ => {}
                }
            }
            FocusedPanel::Output => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.output_scroll += 1;
                        self.auto_follow = false;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.output_scroll = self.output_scroll.saturating_sub(1);
                        self.auto_follow = false;
                    }
                    KeyCode::Char('g') => {
                        self.output_scroll = 0;
                        self.auto_follow = false;
                    }
                    KeyCode::Char('G') => {
                        if let Some(lines) = self.step_outputs.get(self.step_state.selected) {
                            self.output_scroll = lines.len().saturating_sub(20);
                        }
                        self.auto_follow = false;
                    }
                    _ => {}
                }
            }
        }

        TrailViewAction::None
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        trail: &Trail,
        livestock: &Livestock,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // header
                Constraint::Min(1),     // panels
            ])
            .split(area);

        // Header
        trail_header::render_trail_header(frame, chunks[0], trail, livestock);

        // Split panels: steps (30%) + output (70%)
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(70),
            ])
            .margin(1)
            .split(chunks[1]);

        // Steps panel (left)
        let steps_panel = Panel {
            title: "Steps",
            focused: self.focused_panel == FocusedPanel::Steps,
            hints: None,
        };
        let steps_inner = steps_panel.render(frame, panels[0]);

        let step_items: Vec<StepItem> = trail.steps.iter().enumerate().map(|(i, s)| {
            StepItem {
                name: s.name.clone(),
                status: self.step_statuses.get(i).cloned().unwrap_or(StepStatus::Pending),
            }
        }).collect();

        trail_steps::render_step_list(
            frame,
            steps_inner,
            &step_items,
            &self.step_state,
            self.focused_panel == FocusedPanel::Steps,
            self.tick,
        );

        // Output panel (right)
        let output_panel = Panel {
            title: "Output",
            focused: self.focused_panel == FocusedPanel::Output,
            hints: if self.focused_panel == FocusedPanel::Output {
                Some("[j/k] scroll  [g/G] top/bottom")
            } else {
                None
            },
        };
        let output_inner = output_panel.render(frame, panels[1]);

        // Render output for selected step
        let selected = self.step_state.selected;
        if let Some(lines) = self.step_outputs.get(selected) {
            if lines.is_empty() {
                let status_text = match self.step_statuses.get(selected) {
                    Some(StepStatus::Pending) => "Waiting...",
                    Some(StepStatus::Running) => "Running...",
                    Some(StepStatus::Success) => "Completed successfully.",
                    Some(StepStatus::Failed { exit_code }) => "Failed.",
                    Some(StepStatus::Skipped) => "Skipped.",
                    None => "No data.",
                };
                let p = Paragraph::new(format!("  {}", status_text))
                    .style(Style::default().fg(Color::DarkGray));
                frame.render_widget(p, output_inner);
            } else {
                let visible_height = output_inner.height as usize;
                let max_scroll = lines.len().saturating_sub(visible_height);
                let scroll = self.output_scroll.min(max_scroll);

                let visible_lines: Vec<Line> = lines
                    .iter()
                    .skip(scroll)
                    .take(visible_height)
                    .map(|l| Line::from(format!("  {}", l)))
                    .collect();
                let p = Paragraph::new(visible_lines);
                frame.render_widget(p, output_inner);
            }
        } else {
            let p = Paragraph::new("  No step selected")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(p, output_inner);
        }
    }
}
```

**Step 2: Register the module**

In `src/views/mod.rs`, add:

```rust
pub mod trail_view;
```

**Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

**Step 4: Commit**

```bash
git add src/views/trail_view.rs src/views/mod.rs
git commit -m "feat(trails): add split-panel trail execution view"
```

---

### Task 9: Trail History View

**Files:**
- Create: `src/views/trail_history.rs`
- Modify: `src/views/mod.rs` (add module)

**Step 1: Create the trail history view**

Create `src/views/trail_history.rs`:

```rust
use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::header;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::trails::{TrailRun, Trail};
use crate::trails::history;
use crate::types::Livestock;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Runs,
    Detail,
}

pub enum TrailHistoryAction {
    None,
    Back,
    ViewRunLog(usize),
}

pub struct TrailHistoryView {
    focused_panel: FocusedPanel,
    runs_state: ListState,
    detail_scroll: usize,
}

impl TrailHistoryView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Runs,
            runs_state: ListState::new(),
            detail_scroll: 0,
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, runs: &[TrailRun]) -> TrailHistoryAction {
        match key {
            KeyCode::Esc | KeyCode::Char('q') => return TrailHistoryAction::Back,
            KeyCode::Tab => {
                self.focused_panel = match self.focused_panel {
                    FocusedPanel::Runs => FocusedPanel::Detail,
                    FocusedPanel::Detail => FocusedPanel::Runs,
                };
            }
            KeyCode::Enter => {
                if self.focused_panel == FocusedPanel::Runs && !runs.is_empty() {
                    return TrailHistoryAction::ViewRunLog(self.runs_state.selected);
                }
            }
            _ => {}
        }

        match self.focused_panel {
            FocusedPanel::Runs => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.runs_state.select_next(runs.len());
                        self.detail_scroll = 0;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.runs_state.select_prev();
                        self.detail_scroll = 0;
                    }
                    _ => {}
                }
            }
            FocusedPanel::Detail => {
                match key {
                    KeyCode::Char('j') | KeyCode::Down => self.detail_scroll += 1,
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.detail_scroll = self.detail_scroll.saturating_sub(1);
                    }
                    _ => {}
                }
            }
        }

        TrailHistoryAction::None
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        trail: &Trail,
        livestock: &Livestock,
        runs: &[TrailRun],
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Min(1),    // panels
            ])
            .split(area);

        header::render_simple_header(
            frame,
            chunks[0],
            &format!("Trail History: {} . {}", trail.name, livestock.name),
            None,
        );

        // Split panels: runs (30%) + detail (70%)
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(70),
            ])
            .margin(1)
            .split(chunks[1]);

        // Runs panel (left)
        let runs_panel = Panel {
            title: "Runs",
            focused: self.focused_panel == FocusedPanel::Runs,
            hints: Some("[Enter] view logs"),
        };
        let runs_inner = runs_panel.render(frame, panels[0]);

        let run_items: Vec<ListItem> = runs.iter().map(|r| {
            let status_icon = match r.status.as_str() {
                "success" => "\u{2713}",     // ✓
                "failed" => "\u{2717}",      // ✗
                "cancelled" => "\u{2717}",   // ✗
                "running" => "\u{2592}",     // ▒
                _ => "?",
            };

            // Parse date for display
            let date_display = if r.started_at.len() >= 16 {
                format!("{}", &r.started_at[..16])
            } else {
                r.started_at.clone()
            };

            ListItem {
                id: r.started_at.clone(),
                label: format!("{} {}", status_icon, date_display),
                status: Some(match r.status.as_str() {
                    "success" => ItemStatus::Active,
                    "failed" | "cancelled" => ItemStatus::Error,
                    _ => ItemStatus::Inactive,
                }),
                meta: None,
                actions: vec![],
            }
        }).collect();

        list::render_list(
            frame,
            runs_inner,
            &run_items,
            &mut self.runs_state,
            self.focused_panel == FocusedPanel::Runs,
            Some(20),
        );

        // Detail panel (right)
        let detail_panel = Panel {
            title: "Run Detail",
            focused: self.focused_panel == FocusedPanel::Detail,
            hints: if self.focused_panel == FocusedPanel::Detail {
                Some("[j/k] scroll")
            } else {
                None
            },
        };
        let detail_inner = detail_panel.render(frame, panels[1]);

        if let Some(run) = runs.get(self.runs_state.selected) {
            let mut lines: Vec<Line> = Vec::new();

            lines.push(Line::from(vec![
                Span::styled("  Started:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(&run.started_at, Style::default().fg(Color::White)),
            ]));

            if let Some(ref finished) = run.finished_at {
                lines.push(Line::from(vec![
                    Span::styled("  Finished: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(finished, Style::default().fg(Color::White)),
                ]));
            }

            let status_color = match run.status.as_str() {
                "success" => Color::Green,
                "failed" | "cancelled" => Color::Red,
                "running" => Color::Yellow,
                _ => Color::DarkGray,
            };
            lines.push(Line::from(vec![
                Span::styled("  Status:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(&run.status, Style::default().fg(status_color)),
            ]));

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Steps:",
                Style::default().fg(Color::DarkGray),
            )));

            for step in &run.steps {
                let (icon, color) = match step.status.as_str() {
                    "success" => ("\u{2713}", Color::Green),
                    "failed" => ("\u{2717}", Color::Red),
                    "running" => ("\u{2592}", Color::Yellow),
                    _ => ("\u{2591}", Color::DarkGray),
                };

                let duration = step.duration_ms
                    .map(|ms| {
                        if ms >= 1000 {
                            format!(" ({:.1}s)", ms as f64 / 1000.0)
                        } else {
                            format!(" ({}ms)", ms)
                        }
                    })
                    .unwrap_or_default();

                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                    Span::styled(&step.name, Style::default().fg(Color::White)),
                    Span::styled(duration, Style::default().fg(Color::DarkGray)),
                ]));
            }

            let visible_height = detail_inner.height as usize;
            let visible_lines: Vec<Line> = lines
                .into_iter()
                .skip(self.detail_scroll)
                .take(visible_height)
                .collect();

            let p = Paragraph::new(visible_lines);
            frame.render_widget(p, detail_inner);
        } else {
            let p = Paragraph::new("  No runs yet")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(p, detail_inner);
        }
    }
}
```

**Step 2: Register the module**

In `src/views/mod.rs`, add:

```rust
pub mod trail_history;
```

**Step 3: Verify it compiles**

Run: `cargo check 2>&1 | head -20`

**Step 4: Commit**

```bash
git add src/views/trail_history.rs src/views/mod.rs
git commit -m "feat(trails): add trail history view"
```

---

### Task 10: Wire Trails into App Routing & Livestock Detail

This is the integration task that connects everything together.

**Files:**
- Modify: `src/app.rs` (App struct, navigate, go_back, render, input handling, bottom bar)
- Modify: `src/views/livestock_detail.rs` (add trail action)

**Step 1: Add trail view fields to App struct**

In `src/app.rs`, find the `App` struct and add (after `worm_run_log_view` or similar):

```rust
    pub trail_view: views::trail_view::TrailView,
    pub trail_history_view: views::trail_history::TrailHistoryView,
    pub trail_provider: Option<crate::trails::native::NativeProvider>,
    pub trail_run_receiver: Option<tokio::sync::mpsc::Receiver<crate::trails::provider::StepUpdate>>,
    pub trail_run_dir: Option<std::path::PathBuf>,
```

Initialize them in the App constructor:

```rust
    trail_view: views::trail_view::TrailView::new(),
    trail_history_view: views::trail_history::TrailHistoryView::new(),
    trail_provider: None,
    trail_run_receiver: None,
    trail_run_dir: None,
```

**Step 2: Add navigate cases**

In `src/app.rs`, in the `navigate()` method, add:

```rust
AppView::Trail { ref trail, .. } => {
    tmux::update_status_bar(Some(&format!("Trail: {}", trail.name)));
}
AppView::TrailHistory { ref trail, ref livestock, .. } => {
    tmux::update_status_bar(Some(&format!("Trail History: {} . {}", trail.name, livestock.name)));
}
```

**Step 3: Add go_back cases**

In `src/app.rs`, in the `go_back()` method, add:

```rust
AppView::Trail { project, livestock, source, source_barn, .. } => {
    let project = project.clone();
    let livestock = livestock.clone();
    let source = source.clone();
    let source_barn = source_barn.clone();
    self.navigate(AppView::Livestock { project, livestock, source, source_barn });
}
AppView::TrailHistory { project, livestock, trail, source, source_barn } => {
    let project = project.clone();
    let livestock = livestock.clone();
    let trail = trail.clone();
    let source = source.clone();
    let source_barn = source_barn.clone();
    self.navigate(AppView::Trail { project, livestock, trail, source, source_barn });
}
```

**Step 4: Add render cases**

In `src/app.rs`, in the `render_view()` function, add:

```rust
AppView::Trail { ref project, ref livestock, ref trail, .. } => {
    let trail = trail.clone();
    let livestock = livestock.clone();
    app.trail_view.render(frame, area, &trail, &livestock);
}
AppView::TrailHistory { ref project, ref livestock, ref trail, .. } => {
    let trail = trail.clone();
    let livestock = livestock.clone();
    let runs = config::load_trail_runs(&livestock.name, &trail.name);
    app.trail_history_view.render(frame, area, &trail, &livestock, &runs);
}
```

**Step 5: Add input handling**

In `src/app.rs`, in the main input match, add cases:

```rust
AppView::Trail { .. } => {
    handle_trail_input(&mut app, key.code);
}
AppView::TrailHistory { .. } => {
    handle_trail_history_input(&mut app, key.code);
}
```

Add the handler functions:

```rust
fn handle_trail_input(app: &mut App, key: KeyCode) {
    if let AppView::Trail {
        ref project, ref livestock, ref trail, ref source, ref source_barn
    } = app.view {
        let trail = trail.clone();
        let livestock = livestock.clone();
        let project = project.clone();
        let source = source.clone();
        let source_barn = source_barn.clone();

        let action = app.trail_view.handle_input(key, &trail);
        match action {
            views::trail_view::TrailViewAction::None => {}
            views::trail_view::TrailViewAction::Back => {
                app.go_back();
            }
            views::trail_view::TrailViewAction::RunTrail => {
                // Find the barn for this livestock
                let barn = source_barn.as_ref().or_else(|| {
                    livestock.barn.as_ref().and_then(|bn| app.barns.iter().find(|b| b.name == *bn))
                }).cloned();

                if let Some(barn) = barn {
                    match crate::trails::runner::start_trail(&trail, &livestock, &barn) {
                        Ok((run_dir, rx, provider)) => {
                            app.trail_view.running = true;
                            app.trail_view.enter(&trail);
                            app.trail_view.running = true;
                            app.trail_run_receiver = Some(rx);
                            app.trail_provider = Some(provider);
                            app.trail_run_dir = Some(run_dir);
                        }
                        Err(e) => {
                            app.error = Some(format!("Failed to start trail: {}", e));
                        }
                    }
                } else {
                    app.error = Some("No barn found for this livestock".to_string());
                }
            }
            views::trail_view::TrailViewAction::CancelTrail => {
                if let Some(ref provider) = app.trail_provider {
                    let _ = provider.cancel();
                }
            }
            views::trail_view::TrailViewAction::ShowHistory => {
                app.trail_history_view = views::trail_history::TrailHistoryView::new();
                app.navigate(AppView::TrailHistory {
                    project,
                    livestock,
                    trail,
                    source,
                    source_barn,
                });
            }
        }
    }
}

fn handle_trail_history_input(app: &mut App, key: KeyCode) {
    if let AppView::TrailHistory {
        ref project, ref livestock, ref trail, ..
    } = app.view {
        let trail = trail.clone();
        let livestock = livestock.clone();
        let runs = config::load_trail_runs(&livestock.name, &trail.name);

        let action = app.trail_history_view.handle_input(key, &runs);
        match action {
            views::trail_history::TrailHistoryAction::None => {}
            views::trail_history::TrailHistoryAction::Back => {
                app.go_back();
            }
            views::trail_history::TrailHistoryAction::ViewRunLog(idx) => {
                // Future: navigate to a full log view for the selected run
                // For now, this is a no-op until we build the log drill-down
            }
        }
    }
}
```

**Step 6: Add trail update polling to event loop**

In the main event loop (where tick events are processed), add:

```rust
// Poll trail execution updates
if let Some(ref mut rx) = app.trail_run_receiver {
    while let Ok(update) = rx.try_recv() {
        let is_terminal = matches!(
            update.status,
            crate::trails::provider::StepStatus::Failed { .. } |
            crate::trails::provider::StepStatus::Success
        );
        app.trail_view.apply_update(update.step_index, update.status, update.output_line);

        // Check if trail finished (last step succeeded or any step failed)
        if is_terminal {
            // Check if this was the last step
            let all_done = app.trail_view.step_statuses.iter().all(|s| {
                matches!(s,
                    crate::trails::provider::StepStatus::Success |
                    crate::trails::provider::StepStatus::Failed { .. } |
                    crate::trails::provider::StepStatus::Pending
                )
            }) && app.trail_view.step_statuses.iter().any(|s| {
                matches!(s, crate::trails::provider::StepStatus::Failed { .. })
            }) || app.trail_view.step_statuses.iter().all(|s| {
                matches!(s, crate::trails::provider::StepStatus::Success)
            });

            if all_done {
                app.trail_view.running = false;
                // Save final run state
                if let Some(ref run_dir) = app.trail_run_dir {
                    if let AppView::Trail { ref livestock, ref trail, .. } = app.view {
                        let final_status = if app.trail_view.step_statuses.iter().all(|s| {
                            matches!(s, crate::trails::provider::StepStatus::Success)
                        }) {
                            "success"
                        } else {
                            "failed"
                        };

                        let run = crate::trails::TrailRun {
                            livestock: livestock.name.clone(),
                            trail: trail.name.clone(),
                            started_at: chrono::Utc::now().to_rfc3339(),
                            finished_at: Some(chrono::Utc::now().to_rfc3339()),
                            status: final_status.to_string(),
                            steps: trail.steps.iter().enumerate().map(|(i, s)| {
                                crate::trails::TrailStepRun {
                                    name: s.name.clone(),
                                    status: match app.trail_view.step_statuses.get(i) {
                                        Some(crate::trails::provider::StepStatus::Success) => "success".to_string(),
                                        Some(crate::trails::provider::StepStatus::Failed { .. }) => "failed".to_string(),
                                        _ => "pending".to_string(),
                                    },
                                    exit_code: match app.trail_view.step_statuses.get(i) {
                                        Some(crate::trails::provider::StepStatus::Failed { exit_code }) => Some(*exit_code),
                                        Some(crate::trails::provider::StepStatus::Success) => Some(0),
                                        _ => None,
                                    },
                                    started_at: None,
                                    duration_ms: None,
                                }
                            }).collect(),
                        };
                        let _ = config::save_trail_run(&run, run_dir);
                    }
                }
                app.trail_run_receiver = None;
                app.trail_provider = None;
                app.trail_run_dir = None;
            }
        }
    }
}

// Tick trail view animation
app.trail_view.tick();
```

**Step 7: Add bottom bar items**

In `src/app.rs`, in the `get_bottom_bar_items()` function, add:

```rust
AppView::Trail { .. } => vec![
    ("r", "run"),
    ("c", "cancel"),
    ("h", "history"),
    ("Tab", "switch panel"),
    ("j/k", "navigate"),
    ("Esc", "back"),
],
AppView::TrailHistory { .. } => vec![
    ("Tab", "switch panel"),
    ("Enter", "view logs"),
    ("j/k", "navigate"),
    ("Esc", "back"),
],
```

**Step 8: Add OpenTrail action to LivestockDetailView**

In `src/views/livestock_detail.rs`, add to the `LivestockAction` enum:

```rust
    OpenTrail(String),  // trail name
```

And add keybinding in the handle_input for livestock detail (e.g., `t` for trails), and the corresponding handler in `app.rs` that navigates to the trail view:

```rust
LivestockAction::OpenTrail(trail_name) => {
    if let Some(trail) = config::load_trail(trail_name.as_str()) {
        app.trail_view = views::trail_view::TrailView::new();
        app.trail_view.enter(&trail);
        app.navigate(AppView::Trail {
            project: project.clone(),
            livestock: livestock.clone(),
            trail,
            source: source.clone(),
            source_barn: source_barn.clone(),
        });
    }
}
```

**Step 9: Verify it compiles**

Run: `cargo check 2>&1 | head -40`

Fix any compilation errors (likely minor type mismatches or missing imports). This is the largest integration task and may need iterative fixes.

**Step 10: Commit**

```bash
git add src/app.rs src/views/livestock_detail.rs
git commit -m "feat(trails): wire trail views into app routing and event loop"
```

---

### Task 11: End-to-End Test with Sample Trail

**Files:**
- No new files; manual verification

**Step 1: Create a sample trail YAML**

```bash
mkdir -p ~/.yeehaw/trails
cat > ~/.yeehaw/trails/deploy.yaml << 'EOF'
name: deploy
description: Pull and restart service
provider: native
steps:
  - name: Pull latest code
    run: "cd {{repo_path}} && git pull origin {{branch}}"
  - name: Check status
    run: "cd {{repo_path}} && git log --oneline -3"
EOF
```

**Step 2: Build and run**

Run: `cargo build 2>&1 | tail -5`
Expected: Compiles successfully.

**Step 3: Manually test in the TUI**

1. Launch yeehaw
2. Navigate to a project with livestock that has `trails: [deploy]` in its config
3. Select the livestock, press `t` to open trails
4. Verify the trail view renders with steps
5. Press `r` to run (if a barn is configured)
6. Verify streaming output appears
7. Press `h` to check history after run completes

**Step 4: Verify trail run artifacts**

```bash
ls ~/.yeehaw/trail-runs/
```
Expected: A directory matching the pattern `{livestock}--{trail}--{timestamp}/` with `run.json` and `step-N.log` files.

**Step 5: Commit any fixes**

```bash
git add -A
git commit -m "fix(trails): end-to-end testing fixes"
```

---

### Task 12: Final Cleanup & Compilation Verification

**Step 1: Run full build**

Run: `cargo build --release 2>&1 | tail -10`
Expected: Compiles with no errors.

**Step 2: Run clippy**

Run: `cargo clippy 2>&1 | tail -20`
Expected: No errors. Fix any warnings that indicate real issues.

**Step 3: Clean up any dead code warnings**

Address any `unused` warnings for the new modules.

**Step 4: Final commit**

```bash
git add -A
git commit -m "chore(trails): cleanup and final compilation fixes"
```
