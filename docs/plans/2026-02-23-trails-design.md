# Trails: CI/CD Integration for Yeehaw

## Overview

Trails are a new first-class concept in Yeehaw that automate deployment workflows for livestock. A trail is a reusable, step-based pipeline defined in YAML, attached to livestock, and executed via SSH on barns.

## Motivation

Currently, deploying livestock requires manually SSHing into barns and running commands. Trails automate this by defining ordered sequences of shell commands that Yeehaw executes on the livestock's barn, with real-time streaming output in the TUI.

## Design Decisions

- **Trails are attached to livestock.** A livestock (repo + barn + branch) can have multiple trails (deploy, rollback, etc.)
- **Manual trigger from TUI only (v1).** Designed for future automated triggers (webhooks, polling).
- **Provider trait with native-only implementation (v1).** `TrailProvider` trait abstracts execution so external CI/CD providers (GitHub Actions, GitLab CI) can be plugged in later.
- **TUI only, no MCP exposure (v1).** MCP tools for trails are a clean v2 addition.
- **Stop on first failure.** No retry logic or resume-from-step in v1.

## Configuration

### Trail Definition

Trail YAML files live in `~/.yeehaw/trails/`:

```yaml
# ~/.yeehaw/trails/deploy.yaml
name: deploy
description: Pull, build, and restart a service
provider: native

steps:
  - name: Pull latest code
    run: "cd {{repo_path}} && git pull origin {{branch}}"

  - name: Build release
    run: "cd {{repo_path}} && cargo build --release"
    timeout: 300

  - name: Restart service
    run: "sudo systemctl restart {{name}}"

  - name: Verify running
    run: "systemctl is-active {{name}}"
```

### Livestock Config (updated)

Livestock gain a `trails` field referencing trail files by name:

```yaml
livestock:
  - name: my-api
    repo: git@github.com:me/my-api.git
    path: /opt/apps/my-api
    barn: prod-server
    branch: main
    trails:
      - deploy
      - rollback
```

### Template Variables

Resolved at runtime from the livestock context:

| Variable | Source |
|----------|--------|
| `{{name}}` | Livestock name |
| `{{repo_path}}` | Repo path on the barn |
| `{{branch}}` | Livestock branch |
| `{{barn}}` | Barn hostname |
| `{{barn_user}}` | SSH user for the barn |

Simple string replacement only. No conditionals, loops, or expressions. Unrecognized `{{...}}` patterns produce a validation error before execution.

## Logs & Run History

Each trail execution gets its own directory in `~/.yeehaw/trail-runs/`:

```
~/.yeehaw/trail-runs/
  my-api--deploy--2026-02-23T14-30-00/
    run.json          # metadata, step statuses, timing, exit codes
    step-0.log        # full stdout/stderr for "Pull latest code"
    step-1.log        # full stdout/stderr for "Build release"
    step-2.log        # full stdout/stderr for "Restart service"
    step-3.log        # full stdout/stderr for "Verify running"
```

### run.json

```json
{
  "livestock": "my-api",
  "trail": "deploy",
  "started_at": "2026-02-23T14:30:00Z",
  "finished_at": "2026-02-23T14:30:45Z",
  "status": "success",
  "steps": [
    {
      "name": "Pull latest code",
      "status": "success",
      "exit_code": 0,
      "started_at": "2026-02-23T14:30:00Z",
      "duration_ms": 1200
    }
  ]
}
```

## Provider Trait

```rust
pub enum StepStatus {
    Pending,
    Running,
    Success,
    Failed { exit_code: i32 },
    Skipped,
}

pub struct StepUpdate {
    pub step_index: usize,
    pub status: StepStatus,
    pub output_line: Option<String>,
}

pub trait TrailProvider {
    fn name(&self) -> &str;

    fn execute(
        &self,
        trail: &Trail,
        livestock: &Livestock,
    ) -> Result<tokio::sync::mpsc::Receiver<StepUpdate>>;

    fn cancel(&self) -> Result<()>;
}
```

### V1: NativeProvider

SSHes into the livestock's barn and runs each step sequentially. Streams stdout/stderr line-by-line through the `StepUpdate` channel. Writes output to `step-N.log` files in real-time.

### Future providers (not built in v1)

- `GitHubActionsProvider` -- trigger workflows via GitHub API, poll status, stream logs
- `GitLabCIProvider` -- same via GitLab API

## Execution Flow

```
1. User presses 'r' on a trail
2. Confirmation dialog: "Run trail 'deploy' on my-api (prod-server)?"
3. Create run directory: ~/.yeehaw/trail-runs/my-api--deploy--<timestamp>/
4. Resolve all templates against livestock context
5. Validate: barn reachable (SSH check), all template variables resolved
6. For each step sequentially:
   a. Status -> Running (pulse animation starts)
   b. SSH into barn, execute command
   c. Stream stdout/stderr -> step-N.log AND TUI right panel
   d. On exit:
      - exit 0 -> Success
      - exit non-zero -> Failed, halt execution
      - timeout exceeded -> kill process, Failed, halt
7. Write final run.json
8. Remaining unexecuted steps stay Pending
```

### Error Handling

| Scenario | Behavior |
|----------|----------|
| SSH connection fails | Trail fails immediately, error shown in output panel |
| Step exits non-zero | Step marked failed, remaining steps stay pending, trail = failed |
| Step exceeds timeout | Process killed, step marked failed, same as above |
| User cancels (`c`) | Running process killed via SSH, step marked failed, trail = cancelled |
| Barn unreachable at start | Trail never starts, error dialog shown |
| Template variable missing | Trail never starts, error identifies the bad variable |

## TUI

### Navigation

Trails are accessed from the Livestock Detail view:

```
Livestock Detail: my-api
  -> Info
  -> Logs
  -> Trails    <-- new
       -> deploy
       -> rollback
```

### Trail View (split panel)

Left panel: step list with status indicators. Right panel: streaming output for selected step.

```
+-----------------------------------------------------+
|  Trail: deploy  .  my-api  .  prod-server            |
+--------------+--------------------------------------+
|              |                                       |
|  Steps       |  Output                               |
|              |                                       |
|  * Pull      |  $ cd /opt/apps/my-api && git pull    |
|  # Build     |  origin main                          |
|  . Restart   |  Compiling my-api v0.3.1              |
|  . Verify    |  Compiling serde v1.0.203             |
|              |  Compiling tokio v1.38.0              |
|              |  ...                                   |
|              |                                       |
+--------------+--------------------------------------+
|  [r] Run  [c] Cancel  [h] History  [q] Back         |
+-----------------------------------------------------+
```

### Step Status Indicators

| Character | State | Behavior |
|-----------|-------|----------|
| `░` | Pending | Light shade, faint, waiting |
| `░` / `▒` | Running | Pulses between light and medium shade (~500ms interval) |
| `✓` | Success | Checkmark, done |
| `✗` | Failed | Ballot x, something broke |

Skipped steps (after a failure) remain as the pending indicator -- they never ran.

### Trail History View

Pressing `h` shows past runs in the same split-panel layout:

```
+-----------------------------------------------------+
|  Trail History: deploy  .  my-api                    |
+--------------+--------------------------------------+
|              |                                       |
|  Runs        |  Run Detail                           |
|              |                                       |
|  ✓ Feb 23    |  Started: 2026-02-23 14:30:00        |
|    14:30     |  Duration: 45s                        |
|  ✗ Feb 22    |  Steps: 4/4 passed                   |
|    09:15     |                                       |
|  ✓ Feb 21    |  ✓ Pull latest code (1.2s)           |
|    16:45     |  ✓ Build release (38.1s)              |
|              |  ✓ Restart service (2.4s)             |
|              |  ✓ Verify running (0.8s)              |
|              |                                       |
+--------------+--------------------------------------+
|  [enter] View full logs  [q] Back                    |
+-----------------------------------------------------+
```

### Keybindings

| Key | Action |
|-----|--------|
| `r` | Run the trail |
| `c` | Cancel a running trail |
| `h` | Toggle history view |
| `Enter` | Expand selected step/run |
| `Up/Down` or `j/k` | Navigate step list |
| `q` | Back to livestock detail |

## Module Structure

```
src/
  trails/
    mod.rs            -- Trail, TrailStep, TrailRun types + module exports
    provider.rs       -- TrailProvider trait, StepStatus, StepUpdate
    native.rs         -- NativeProvider (SSH executor)
    runner.rs         -- Orchestrates execution: resolves templates,
                         calls provider, writes logs + run.json
    history.rs        -- Load/query past trail runs from ~/.yeehaw/trail-runs/

  views/
    trail_view.rs     -- Split-panel trail execution/monitoring view
    trail_history.rs  -- Trail run history browser

  components/
    trail_steps.rs    -- Step list widget with status indicators + pulse animation

  types.rs            -- Updated: Livestock gains trails: Vec<String> field
  config.rs           -- Updated: loads trail YAML files from ~/.yeehaw/trails/
  app.rs              -- Updated: new view routing for trail views
```

## Scope Boundaries (v1 vs later)

### In scope (v1)

- Trail YAML definition and loading
- Template variable resolution with validation
- TrailProvider trait
- NativeProvider (SSH execution)
- Trail runner with log persistence
- Trail view (split panel, streaming output, step indicators with pulse animation)
- Trail history view
- Livestock config updated with trails field
- Confirmation dialog before execution

### Out of scope (v2+)

- MCP tool exposure for trails
- Automated triggers (webhooks, git polling)
- External CI/CD providers (GitHub Actions, GitLab CI)
- Retry logic / resume from step N
- Custom template variables (trail_vars)
- Parallel step execution
- Step dependencies / conditional steps
