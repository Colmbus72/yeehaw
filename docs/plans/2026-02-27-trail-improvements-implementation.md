# Trail Improvements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable local trail execution, show step commands in the TUI output panel, and create a Claude Code skill for trail authoring.

**Architecture:** Three independent changes: (1) branch in `NativeProvider::execute()` to spawn `sh -c` for local barns instead of SSH, with fallback to `local_barn()` in all callsites; (2) store step `run` commands in `TrailView` and prepend them in the output panel; (3) a new `.md` skill file for trail authoring knowledge.

**Tech Stack:** Rust (ratatui TUI, tokio, std::process), Claude Code skills (.md)

---

### Task 1: Local Barn — Add local execution path to NativeProvider

**Files:**
- Modify: `src/trails/native.rs`

**Step 1: Add `use crate::config` import**

Add at top of file after existing imports:

```rust
use crate::config;
```

**Step 2: Extract the command-building and process-spawning into a local vs remote branch**

In `execute()`, after the env exports are built (line 100), replace the SSH command construction (lines 80-106) with a branch:

```rust
                // Build the full command with env exports
                let env_exports: String = step_env.iter()
                    .map(|(k, v)| format!("export {}='{}'", k, v.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join("; ");
                let full_command = if env_exports.is_empty() {
                    format!("{} 2>&1", step.run)
                } else {
                    format!("{}; {} 2>&1", env_exports, step.run)
                };

                let is_local = config::is_local_barn(&barn);

                let mut cmd = if is_local {
                    // Local execution: spawn shell directly
                    let repo_path = base_env.iter()
                        .find(|(k, _)| k == "REPO_PATH")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or(".");
                    let local_cmd = format!("cd '{}' && {}", repo_path, full_command);
                    let mut c = Command::new("sh");
                    c.arg("-c").arg(&local_cmd);
                    c
                } else {
                    // Remote execution: SSH to barn
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
                cmd.stderr(Stdio::null());
```

**Step 3: Update the spawn error message to be generic**

Change line 187 from:
```rust
output_line: Some(format!("SSH failed: {}", e)),
```
to:
```rust
output_line: Some(format!("Failed to spawn: {}", e)),
```

**Step 4: Run `cargo check` to verify compilation**

Run: `cargo check 2>&1`
Expected: compiles without errors (warnings OK)

**Step 5: Commit**

```bash
git add src/trails/native.rs
git commit -m "feat(trails): add local execution path for local barn"
```

---

### Task 2: Local Barn — Fix callsites to fall back to local barn

**Files:**
- Modify: `src/app.rs:1139-1159` (TUI run handler)
- Modify: `src/app.rs:2035-2068` (MCP trigger handler)

**Step 1: Fix the TUI trail run handler (~line 1139)**

Replace the barn lookup + error block:

```rust
            TrailViewAction::RunTrail => {
                // Find the barn for this livestock (fall back to local barn)
                let barn = source_barn.as_ref().or_else(|| {
                    livestock.barn.as_ref().and_then(|bn| app.barns.iter().find(|b| b.name == *bn))
                }).cloned().unwrap_or_else(config::local_barn);

                match crate::trails::runner::start_trail(&trail, &livestock, &barn, Some(&project.name)) {
                    Ok((run_dir, rx, provider)) => {
                        app.trail_view.start_run(&trail);
                        app.trail_run_receiver = Some(rx);
                        app.trail_provider = Some(provider);
                        app.trail_run_dir = Some(run_dir);
                    }
                    Err(e) => {
                        app.error = Some(format!("Failed to start trail: {}", e));
                    }
                }
            }
```

This removes the `if let Some(barn)` / `else` error branch entirely — no barn just means local.

**Step 2: Fix the MCP trigger handler (~line 2035)**

Replace the barn lookup logic. Currently it requires `ls.barn` to be `Some` and bails silently if not. Change to:

```rust
            // Find the livestock, trail, and barn across all projects
            let projects = config::load_projects();
            for project in &projects {
                if let Some(ls) = project.livestock.iter().find(|l| l.name == livestock_name) {
                    if let Some(trail) = config::load_trail(trail_name) {
                        let barn = ls.barn.as_ref()
                            .and_then(|bn| config::load_barns().into_iter().find(|b| &b.name == bn))
                            .unwrap_or_else(config::local_barn);
                        let proj_name = project_name_str.as_deref().unwrap_or(&project.name);
                        match crate::trails::runner::start_trail(&trail, ls, &barn, Some(proj_name)) {
                            Ok((run_dir, rx, provider)) => {
                                app.trail_view = TrailView::new();
                                app.trail_view.enter(&trail, ls);
                                app.trail_view.start_run(&trail);
                                app.trail_run_receiver = Some(rx);
                                app.trail_provider = Some(provider);
                                app.trail_run_dir = Some(run_dir);
                                app.navigate(AppView::Trail {
                                    project: project.clone(),
                                    livestock: ls.clone(),
                                    trail: trail.clone(),
                                    source: "project".to_string(),
                                    source_barn: Some(barn.clone()),
                                });
                            }
                            Err(e) => {
                                app.error = Some(format!("Failed to start trail: {}", e));
                            }
                        }
                        return;
                    }
                }
            }
```

**Step 3: Run `cargo check` to verify compilation**

Run: `cargo check 2>&1`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add src/app.rs
git commit -m "feat(trails): fall back to local barn when livestock has no barn"
```

---

### Task 3: Command Visibility — Store step commands on TrailView

**Files:**
- Modify: `src/views/trail_view.rs`

**Step 1: Add `step_commands` field to `TrailView` struct**

Add after the `step_outputs` field (line 52):

```rust
    /// The `run:` command for each step, cached from the trail definition.
    pub step_commands: Vec<String>,
```

**Step 2: Initialize in `new()`**

Add to the `Self` block in `new()`:

```rust
            step_commands: vec![],
```

**Step 3: Populate in `enter()`**

After line 84 (`self.step_statuses = ...`), add:

```rust
        self.step_commands = trail.first_job()
            .map(|(_, j)| j.steps.iter().map(|s| s.run.clone()).collect())
            .unwrap_or_default();
```

**Step 4: Repopulate in `start_run()`**

After line 96 (`self.step_outputs = ...`), add:

```rust
        self.step_commands = trail.first_job()
            .map(|(_, j)| j.steps.iter().map(|s| s.run.clone()).collect())
            .unwrap_or_default();
```

**Step 5: Run `cargo check`**

Run: `cargo check 2>&1`
Expected: compiles (warning about unused field is fine for now)

**Step 6: Commit**

```bash
git add src/views/trail_view.rs
git commit -m "feat(trails): cache step commands on TrailView"
```

---

### Task 4: Command Visibility — Render command in output panel

**Files:**
- Modify: `src/views/trail_view.rs` (the `render_output_panel` method)

**Step 1: Rewrite `render_output_panel` to prepend the command**

Replace the entire `render_output_panel` method (lines 351-395):

```rust
    fn render_output_panel(&self, frame: &mut Frame, area: Rect) {
        let panel = Panel {
            title: "Output",
            focused: self.focused_panel == FocusedPanel::Output,
            hints: if self.focused_panel == FocusedPanel::Output {
                Some("[j/k] scroll  [g/G] top/bottom")
            } else {
                None
            },
        };
        let inner = panel.render(frame, area);

        // Build combined lines: command header + separator + output
        let mut all_lines: Vec<Line> = Vec::new();

        // Pinned command at top
        if let Some(cmd) = self.step_commands.get(self.step_state.selected) {
            // Wrap long commands across multiple lines
            let prefix = "  $ ";
            let max_width = inner.width as usize;
            let available = max_width.saturating_sub(prefix.len());

            if available > 0 {
                let mut remaining = cmd.as_str();
                let mut first = true;
                while !remaining.is_empty() {
                    let (chunk, rest) = if remaining.len() > available {
                        (&remaining[..available], &remaining[available..])
                    } else {
                        (remaining, "")
                    };
                    let line_prefix = if first { "  $ " } else { "    " };
                    all_lines.push(Line::from(Span::styled(
                        format!("{}{}", line_prefix, chunk),
                        Style::default().fg(Color::DarkGray),
                    )));
                    remaining = rest;
                    first = false;
                }
            }

            // Separator
            let sep = "\u{2500}".repeat(max_width.min(60));
            all_lines.push(Line::from(Span::styled(
                format!("  {}", sep),
                Style::default().fg(Color::DarkGray),
            )));
        }

        let output_lines = self.get_selected_output();

        if output_lines.is_empty() && all_lines.len() <= 2 {
            // Command header + separator + status message
            let status_text = if self.is_viewing_active_run() {
                match self.step_statuses.get(self.step_state.selected) {
                    Some(StepStatus::Pending) => "Waiting...",
                    Some(StepStatus::Running) => "Running...",
                    Some(StepStatus::Success) => "Completed successfully.",
                    Some(StepStatus::Failed { .. }) => "Failed.",
                    Some(StepStatus::Skipped) => "Skipped.",
                    None => "No data.",
                }
            } else if self.runs.is_empty() && !self.running {
                "No runs yet. Press [r] to run."
            } else {
                "No output."
            };
            all_lines.push(Line::from(Span::styled(
                format!("  {}", status_text),
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for line in &output_lines {
                all_lines.push(Line::from(format!("  {}", line)));
            }
        }

        let visible_height = inner.height as usize;
        let max_scroll = all_lines.len().saturating_sub(visible_height);
        let scroll = self.output_scroll.min(max_scroll);

        let visible_lines: Vec<Line> = all_lines
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .collect();
        let p = Paragraph::new(visible_lines);
        frame.render_widget(p, inner);
    }
```

**Step 2: Run `cargo check`**

Run: `cargo check 2>&1`
Expected: compiles without errors

**Step 3: Build and do a quick visual test**

Run: `cargo build --release 2>&1`
Launch the app manually and navigate to a trail to verify the command shows in the output panel.

**Step 4: Commit**

```bash
git add src/views/trail_view.rs
git commit -m "feat(trails): show step command in output panel header"
```

---

### Task 5: Trail Authoring Skill — Create the skill file

**Files:**
- Create: `/Users/kev/.claude/skills/yeehaw-trail-authoring/SKILL.md`

**Step 1: Create the skill directory and file**

```bash
mkdir -p /Users/kev/.claude/skills/yeehaw-trail-authoring
```

**Step 2: Write the skill file**

Create `/Users/kev/.claude/skills/yeehaw-trail-authoring/SKILL.md` with the following content:

```markdown
---
name: yeehaw-trail-authoring
description: Create and edit Yeehaw trail definitions. Use when a user asks to create a trail, build pipeline, deploy workflow, or CI/CD automation for a Yeehaw livestock. Triggers on requests like "create a trail", "add a build trail", "set up a deploy pipeline", "make a trail for X".
---

# Yeehaw Trail Authoring

Create properly formatted trail YAML definitions for Yeehaw livestock deployments.

## What Trails Are

Trails are lightweight CI/CD pipelines scoped to a single livestock (deployed app instance). They run on the barn (server) where the livestock lives, or locally if no barn is configured.

Trails are for **operational tasks you'd otherwise run manually**: build and tag a release, deploy code, run migrations, restart services, run health checks, sync data. They are not a replacement for full CI systems like GitHub Actions — they're the "last mile" automation that runs where your code actually lives.

## Trail YAML Schema

Trails are stored as YAML in `~/.yeehaw/trails/<name>.yaml` and follow a GHA-compatible structure.

### Full Schema

```yaml
# Optional: automatic triggers
on:
  push:
    branches:            # List of branch names to watch (optional, watches all if omitted)
      - main
      - production
    poll-interval: 30    # Seconds between git polls (default: 30)

# Optional: trail-level environment variables
env:
  SOME_VAR: "value"      # Available to all jobs and steps

# Required: jobs map (currently only the first job executes)
jobs:
  build:                 # Job name (any string)
    runs-on: native      # Provider (only "native" currently supported)
    env:                 # Optional: job-level env vars (override trail-level)
      JOB_VAR: "value"
    steps:
      - name: Step name  # Required: human-readable step name
        run: |           # Required: shell command(s) to execute
          echo "hello"
          npm install
        env:             # Optional: step-level env vars (override job-level)
          STEP_VAR: "value"
        timeout-minutes: 5  # Optional: max runtime in minutes (default: 1)
```

### Key Rules
- **jobs**: Only the first job in the map executes (V2 limitation). Name it descriptively.
- **runs-on**: Always `native`. This means the command runs directly on the barn via SSH (or locally via `sh -c` for the local barn).
- **steps**: Execute sequentially. First failure stops the entire run (fail-fast).
- **run**: Shell commands. Multi-line supported with YAML `|` or `>`. `stdout` and `stderr` are merged.
- **timeout-minutes**: Default is 1 minute. Set higher for builds, deploys, or migrations.

## Auto-Injected Environment Variables

Every step automatically receives these variables. Do NOT define them in `env:` — they're injected by the runner.

| Variable | Value | Example |
|----------|-------|---------|
| `NAME` | Livestock name | `myapp-prod` |
| `REPO_PATH` | Livestock path (local or remote) | `/var/www/myapp` |
| `BRANCH` | Git branch (default: "main") | `main` |
| `BARN` | Barn hostname (or "localhost") | `prod-server.example.com` |
| `BARN_USER` | SSH user for the barn | `deploy` |
| `PROJECT` | Project name | `myapp` |
| `TRAIL` | Trail name | `deploy-prod` |
| `RUN_ID` | Unique run timestamp | `2024-01-15T10-30-45` |
| `RUN_NUMBER` | Sequential run counter | `42` |
| `STEP_NAME` | Current step's name | `Install dependencies` |

### Environment Variable Layering

Priority order (later overrides earlier):
1. Auto-injected (above)
2. Trail-level `env:`
3. Job-level `env:`
4. Step-level `env:`

## Execution Model

- **Local barn**: Commands run via `sh -c "cd $REPO_PATH && <command>"` directly on the machine running Yeehaw.
- **Remote barn**: Commands run via `ssh user@host "<command>"` on the barn server.
- **Streaming**: Output streams line-by-line to the TUI in real-time and is saved to log files.
- **Fail-fast**: First step that exits non-zero stops the entire run.
- **Cancellation**: User can cancel a running trail with `x` in the TUI or via MCP.

## Creating a Trail

Use the `create_trail` MCP tool:

```
mcp__yeehaw__create_trail(
  name: "trail-name",
  content: "<full YAML content>"
)
```

Then link it to a livestock:

```
mcp__yeehaw__link_trail(
  project: "project-name",
  livestock: "livestock-name",
  trail: "trail-name"
)
```

## Example Trails

### Build & Tag (iOS / Mobile)
```yaml
jobs:
  build:
    runs-on: native
    steps:
      - name: Increment version
        run: |
          cd $REPO_PATH
          # Bump patch version in Info.plist or similar
          agvtool next-version -all
        timeout-minutes: 1

      - name: Build archive
        run: |
          cd $REPO_PATH
          xcodebuild -scheme MyApp -configuration Release archive -archivePath build/MyApp.xcarchive
        timeout-minutes: 15

      - name: Tag release
        run: |
          cd $REPO_PATH
          VERSION=$(agvtool what-version -terse)
          git add .
          git commit -m "Bump to v$VERSION"
          git tag "v$VERSION"
          git push origin $BRANCH --tags
        timeout-minutes: 2
```

### Deploy (Pull, Install, Restart)
```yaml
env:
  SERVICE_NAME: myapp

jobs:
  deploy:
    runs-on: native
    steps:
      - name: Pull latest code
        run: |
          cd $REPO_PATH
          git fetch origin $BRANCH
          git reset --hard origin/$BRANCH
        timeout-minutes: 2

      - name: Install dependencies
        run: |
          cd $REPO_PATH
          npm ci --production
        timeout-minutes: 5

      - name: Run migrations
        run: |
          cd $REPO_PATH
          npx prisma migrate deploy
        timeout-minutes: 3

      - name: Restart service
        run: sudo systemctl restart $SERVICE_NAME
        timeout-minutes: 1
```

### Database Backup
```yaml
env:
  DB_NAME: myapp_production
  BACKUP_DIR: /var/backups/db

jobs:
  backup:
    runs-on: native
    steps:
      - name: Create backup
        run: |
          TIMESTAMP=$(date +%Y%m%d-%H%M%S)
          pg_dump $DB_NAME | gzip > $BACKUP_DIR/$DB_NAME-$TIMESTAMP.sql.gz
        timeout-minutes: 10

      - name: Prune old backups
        run: |
          find $BACKUP_DIR -name "*.sql.gz" -mtime +30 -delete
        timeout-minutes: 1
```

### Health Check / Smoke Test
```yaml
jobs:
  health:
    runs-on: native
    steps:
      - name: Check HTTP response
        run: |
          STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/health)
          if [ "$STATUS" != "200" ]; then
            echo "Health check failed with status $STATUS"
            exit 1
          fi
          echo "Health check passed (200 OK)"
        timeout-minutes: 1

      - name: Check disk space
        run: |
          USAGE=$(df -h / | awk 'NR==2{print $5}' | tr -d '%')
          if [ "$USAGE" -gt 90 ]; then
            echo "WARNING: Disk usage at ${USAGE}%"
            exit 1
          fi
          echo "Disk usage OK (${USAGE}%)"
        timeout-minutes: 1
```

## Anti-Patterns

- **Don't put secrets in trail YAML.** Use livestock env files (`env_path` on the livestock) or environment variables already configured on the barn.
- **Don't set `timeout-minutes: 0`.** There's no "no timeout" — use a high value like 30 or 60 for long operations.
- **Don't chain 20 steps when a shell script would do.** If a trail has more than 5-6 steps, consider whether some should be a single script file that the trail calls.
- **Don't use `cd $REPO_PATH` in every step for remote barns.** The SSH session starts in the user's home directory — `cd $REPO_PATH` at the start of multi-command steps is fine, but for local barns the runner already `cd`s to `REPO_PATH` automatically.
- **Don't use interactive commands.** Steps run non-interactively. No `vim`, no `read`, no prompts. Use `-y` flags where needed.

## Trigger Configuration

To auto-trigger a trail when new commits are pushed:

```yaml
on:
  push:
    branches:
      - main
    poll-interval: 60    # Check every 60 seconds
```

This creates a poll worm that runs `git ls-remote` at the configured interval. When new commits are detected on the specified branch, the trail runs automatically.

- If `branches` is omitted, all branches are watched.
- The poll worm is auto-created when the trail is linked and auto-removed when unlinked.
```

**Step 3: Commit**

```bash
git add /Users/kev/.claude/skills/yeehaw-trail-authoring/SKILL.md
git commit -m "feat: add yeehaw trail authoring skill for Claude Code"
```

---

### Task 6: Final build and verify

**Files:** None (verification only)

**Step 1: Full release build**

Run: `cargo build --release 2>&1`
Expected: compiles successfully

**Step 2: Verify the skill is discoverable**

Start a new Claude Code session in the Yeehaw directory and confirm `yeehaw-trail-authoring` appears in the available skills list.

**Step 3: Commit the design/plan docs if not already committed**

```bash
git add docs/plans/2026-02-27-trail-improvements-design.md docs/plans/2026-02-27-trail-improvements-implementation.md
git commit -m "docs: add trail improvements design and implementation plan"
```
