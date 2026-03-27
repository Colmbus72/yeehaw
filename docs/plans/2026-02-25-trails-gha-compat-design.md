# Trails v2: GHA-Compatible Format, Git Polling, MCP Exposure

## Overview

Trails switch to a GitHub Actions-compatible YAML format, gain automated `on: push` triggering via git polling worms, expose a full set of MCP tools for Claude-assisted trail management, and clearly document all available environment variables.

## Motivation

- Users shouldn't need to learn a custom YAML format when GitHub Actions syntax is already familiar
- Automated triggers eliminate manual TUI-only execution for deploy-on-push workflows
- MCP exposure lets Claude create, manage, and run trails conversationally
- Environment variables need clear documentation so trail authors know what's available

## Design Decisions

- **GHA-compatible YAML syntax.** `on:`, `jobs:`, `steps:`, `run:`, `env:`, `timeout-minutes:` all work the same way. A simple GitHub Action can be copied over with minimal edits (swap `runs-on: ubuntu-latest` for `runs-on: native`, remove `uses:` steps).
- **Clean break from v1 format.** No backward compatibility with `{{handlebars}}` templates or flat `steps:` list. Trails are brand new (days old), so no migration burden.
- **Jobs layer supported, single-job execution in v2.** The `jobs:` key is parsed for format compatibility. Only the first job executes. Multi-job (parallel barn execution) is a natural v3 extension via `needs:` dependencies.
- **Git polling via regular worms.** `on: push` auto-creates a visible, toggleable poll worm that runs `git ls-remote` on a schedule. Standard worm lifecycle (run history, logs, enable/disable).
- **Plain env vars, no expression parser.** `$NAME` instead of `${{ github.* }}`. Shell-native, no custom parser needed.
- **Full MCP CRUD + run.** 11 new MCP tools covering the complete trail lifecycle.

## Trail YAML Format

Trail files live in `~/.yeehaw/trails/*.yaml`:

```yaml
# ~/.yeehaw/trails/deploy.yaml
name: deploy

on:
  push:
    branches: [main]
    poll-interval: 30        # seconds, default 30

env:
  NODE_ENV: production       # top-level env, available to all steps

jobs:
  deploy:
    runs-on: native          # "native" = SSH to livestock's barn
    env:
      DEPLOY_ENV: staging    # job-level env
    steps:
      - name: Pull latest code
        run: cd $REPO_PATH && git pull origin $BRANCH

      - name: Build release
        run: cd $REPO_PATH && cargo build --release
        timeout-minutes: 5

      - name: Restart service
        env:
          RESTART_TIMEOUT: "30"
        run: sudo systemctl restart $NAME

      - name: Verify running
        run: systemctl is-active $NAME
```

### Format differences from GitHub Actions

| GitHub Actions | Yeehaw Trails | Reason |
|---------------|---------------|--------|
| `runs-on: ubuntu-latest` | `runs-on: native` | Executes via SSH on barn, not a cloud runner |
| `uses: actions/checkout@v4` | (not supported) | No action marketplace, use `run:` with shell commands |
| `${{ github.ref_name }}` | `$BRANCH` | Plain env vars, no expression parser |
| `${{ secrets.TOKEN }}` | (not supported v2) | Future: secrets management |
| `needs: [build]` | (parsed, not executed v2) | Future: multi-job with dependencies |

### Trigger types

| Trigger | V2 Behavior |
|---------|-------------|
| `on: push` | Git polling via auto-created worm |
| `on: push: branches: [main, dev]` | Poll specific branches only |
| (no `on:` block) | Manual trigger only (TUI `[r]` key or MCP `run_trail`) |
| `on: pull_request` | Not supported (v3+) |
| `on: schedule` | Not supported (v3+, use worms directly) |

## Environment Variables

### Auto-injected variables

Always available in every step, no configuration needed:

| Variable | Description | Example |
|----------|-------------|---------|
| `$NAME` | Livestock name | `my-api` |
| `$REPO_PATH` | Path on barn | `/opt/apps/my-api` |
| `$BRANCH` | Git branch | `main` |
| `$BARN` | Barn hostname | `prod-server.example.com` |
| `$BARN_USER` | SSH user | `deploy` |
| `$PROJECT` | Project name | `my-project` |
| `$TRAIL` | Trail name | `deploy` |
| `$RUN_ID` | Unique run identifier | `2026-02-25T14-30-00` |
| `$RUN_NUMBER` | Incrementing run counter | `42` |
| `$STEP_NAME` | Current step name | `Build release` |

### User-defined `env:` blocks

Supported at three levels with merge semantics (later wins):

1. Auto-injected (lowest priority)
2. Top-level `env:` block
3. Job-level `env:` block
4. Step-level `env:` block (highest priority)

```yaml
env:
  FOO: from-top        # available to all steps

jobs:
  deploy:
    env:
      FOO: from-job    # overrides top-level for this job
    steps:
      - name: Step 1
        run: echo $FOO  # prints "from-job"
      - name: Step 2
        env:
          FOO: from-step
        run: echo $FOO  # prints "from-step"
```

## Git Polling & Trigger System

### Polling flow

```
Trail linked to livestock with `on: push`
    |
    v
Yeehaw auto-creates poll worm:
    name: poll:{livestock}:{trail}
    schedule: every {poll-interval} seconds (default 30)
    command: git ls-remote {repo_url} refs/heads/{branch}
    |
    v
Worm runs on barn via SSH -> gets remote SHA
    |
    v
Compare to stored SHA in ~/.yeehaw/poll-state/{livestock}--{branch}.sha
    |
    v
If different:
    1. Update stored SHA immediately (prevents double-trigger)
    2. Write trigger file to ~/.yeehaw/worm-triggers/
    3. Watcher detects -> starts trail execution

If same: no-op, wait for next poll cycle
```

### Poll state storage

```
~/.yeehaw/poll-state/
    my-api--main.sha          # contains: "abc123def456..."
    my-api--develop.sha
```

### Poll worm behavior

Poll worms are regular, visible worms. They appear in the worms list, can be toggled on/off by the user, and follow the exact same lifecycle as other worms (scheduling, run history, logs, enable/disable).

Auto-worm lifecycle:
- Created when a trail with `on: push` is linked to a livestock
- Removed when the trail is unlinked or the `on:` block is removed
- Naming convention: `poll:{livestock}:{trail}` (e.g., `poll:my-api:deploy`)

### Concurrency policy

When a push arrives while a trail is already running: **skip and note**. The running trail's `git pull` step will already grab the latest commits. A "skipped trigger" event is logged in the worm run history. No queueing — redundant deploys waste time.

## MCP Trail Tools

11 new tools following the established pattern (param struct + `#[tool]` macro + `ok_json`/`err_text` helpers):

| Tool | Description | Required Params |
|------|-------------|-----------------|
| `list_trails` | List all trail definitions | (none) |
| `get_trail` | Get trail YAML content and metadata | `name` |
| `create_trail` | Create a new trail from YAML | `name`, `content` |
| `update_trail` | Update an existing trail | `name`, `content` |
| `delete_trail` | Delete a trail definition | `name` |
| `link_trail` | Link trail to a livestock | `project`, `livestock`, `trail` |
| `unlink_trail` | Unlink trail from a livestock | `project`, `livestock`, `trail` |
| `run_trail` | Trigger a trail run | `project`, `livestock`, `trail` |
| `list_trail_runs` | List run history | `project`, `livestock`, `trail`, `limit?` |
| `get_trail_run` | Get run details (step statuses, timing) | `project`, `livestock`, `trail`, `run_timestamp` |
| `read_trail_step_log` | Read stdout/stderr for a step | `project`, `livestock`, `trail`, `run_timestamp`, `step` |

### `create_trail` behavior

Claude sends the full GHA-compatible YAML as a string. Yeehaw validates it (parses `on:`, `jobs:`, `steps:`, checks required fields) and writes to `~/.yeehaw/trails/{name}.yaml`. If the trail has `on: push` and gets linked to a livestock, the poll worm is auto-created.

### `run_trail` behavior

Triggers execution the same way the TUI `[r]` key does. Returns immediately with the run ID. Caller polls `get_trail_run` for status or `read_trail_step_log` for output.

## Data Model Changes

### New trail types (`src/trails/mod.rs`)

```rust
pub struct Trail {
    pub name: String,
    pub on: Option<TrailTrigger>,
    pub env: Option<HashMap<String, String>>,
    pub jobs: HashMap<String, TrailJob>,
}

pub struct TrailTrigger {
    pub push: Option<PushTrigger>,
}

pub struct PushTrigger {
    pub branches: Option<Vec<String>>,
    pub poll_interval: Option<u64>,      // seconds, default 30
}

pub struct TrailJob {
    pub runs_on: String,                 // "native" for SSH
    pub env: Option<HashMap<String, String>>,
    pub steps: Vec<TrailStep>,
}

pub struct TrailStep {
    pub name: String,
    pub run: String,
    pub env: Option<HashMap<String, String>>,
    pub timeout_minutes: Option<u64>,    // default 1
}
```

### Env var resolution in runner

Remove `{{handlebars}}` template resolution. Build env var map from 4-layer merge (auto-injected < top-level < job-level < step-level). Pass merged env vars via SSH `export` statements (same SSH mechanism as current).

### Config changes

- Parser updated to deserialize GHA-style YAML
- `save_trail` serializes new format
- New directory: `~/.yeehaw/poll-state/`

### TUI wizard changes

Trail creation wizard generates GHA-format YAML. Adds optional trigger selection step ("Trigger on push? [y/n]") and branch input ("Which branches? [main]").

## Scope Boundaries

### In scope (v2)

- GHA-compatible trail YAML format
- `on: push` trigger via git polling worms (visible, toggleable, regular worms)
- 10 auto-injected env vars with clear documentation
- `env:` blocks at top-level, job-level, step-level with merge semantics
- 11 MCP tools for full trail lifecycle
- Clean break from old format
- Updated trail creation wizard
- Poll state storage and SHA-based deduplication
- Skip policy when trail already running

### Out of scope (v3+)

- Multi-job execution (parallel jobs on different barns)
- `uses:` actions (marketplace equivalent)
- `${{ }}` expression syntax
- `on: pull_request`, `on: schedule`, `on: workflow_dispatch`
- `needs:` job dependency chains
- `if:` conditional steps
- Matrix builds (`strategy: matrix:`)
- Artifacts / caching between steps
- Secrets management
- Webhook listener (HTTP endpoint)
- `services:` containers
