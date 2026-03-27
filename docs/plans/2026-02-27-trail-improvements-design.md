# Trail Improvements Design

Three changes to the trail system: local execution support, command visibility in the TUI, and a Claude Code skill for trail authoring.

## 1. Local Barn Trail Execution

### Problem
Trail execution always SSHs to a barn. Livestock on the local machine (no barn configured) can't run trails, blocking use cases like iOS build-and-archive workflows.

### Design
The `local` barn already exists as a virtual entity: `config::local_barn()` auto-injects it into `load_barns()`, `is_local_barn()` detects it, and livestock with `barn: None` maps to it.

The missing piece is the execution path. `NativeProvider::execute()` in `trails/native.rs` unconditionally builds an SSH command. It needs a branch:

**Remote (existing):**
```
ssh -p PORT -i KEY user@host "export VAR='val'; command 2>&1"
```

**Local (new):**
```
sh -c "cd $REPO_PATH && export VAR='val'; command 2>&1"
```

Same `Stdio::piped()`, same line-by-line streaming, same timeout logic, same cancellation. The only difference is `Command::new("sh")` with `["-c", full_command]` instead of `Command::new("ssh")` with SSH args. The `cd $REPO_PATH` sets the working directory to the livestock's path.

**Callers to fix:**
- `app.rs` trail run handler (~line 1140): falls back to `config::local_barn()` instead of erroring "No barn found"
- `app.rs` MCP trigger handler (~line 2040): same fallback
- `mcp_server.rs` `run_trail` tool: same fallback

## 2. Command Visibility in Output Panel

### Problem
The TUI steps panel only shows step names. There's no way to see what command a step actually runs without reading the trail YAML file.

### Design
The output panel (right 50% of trail view) shows the step's `run:` command pinned at the top, with output/logs below.

**Layout:**
```
┌─ Output ─────────────────────────┐
│  $ npm run build --release       │  <- command (dimmed, $ prefix)
│  ─────────────────────────────── │  <- separator
│  Building for production...      │  <- stdout/stderr
│  ✓ Compiled 142 modules          │
│  ✓ Bundle size: 2.1MB            │
└──────────────────────────────────┘
```

Works in all states:
- **Before run:** Just the command (review what will execute)
- **During run:** Command + live streaming output
- **Historical run:** Command + saved log

**Implementation:** Store a `Vec<String>` of step `run` commands on `TrailView` during `enter()`. In `render_output_panel()`, prepend the command for the selected step index before the output lines. Style it with `Color::DarkGray` and a `$ ` prefix.

## 3. Trail Authoring Skill

### Problem
When users ask Claude to create trails, Claude has no knowledge of the trail YAML schema, available env vars, or intended use patterns. Trails get malformed or use incorrect keys.

### Design
A Claude Code skill file (`.md`) that teaches Claude how to author trails. Installed alongside the yeehaw MCP tools so it's available in any session.

**Contents:**
1. **Ethos** — Trails are lightweight CI/CD for operational tasks scoped to a livestock: build & tag, deploy, restart, migrate, archive. Not a GitHub Actions replacement.
2. **YAML schema** — Every key with types and defaults: `on.push.branches`, `on.push.poll-interval`, `env` at trail/job/step levels, `jobs.<name>.runs-on`, `steps[].name`, `steps[].run`, `steps[].env`, `steps[].timeout-minutes`.
3. **Auto-injected env vars** — `NAME`, `REPO_PATH`, `BRANCH`, `BARN`, `BARN_USER`, `PROJECT`, `TRAIL`, `RUN_ID`, `RUN_NUMBER`, `STEP_NAME` with descriptions.
4. **Execution model** — Sequential steps, fail-fast, per-step timeouts (default 1 min), stdout/stderr merged, local vs remote execution.
5. **Example trails** — Build & tag (iOS), deploy (pull/install/restart), database migration, health check.
6. **Anti-patterns** — No secrets in YAML, don't over-step what a shell script handles better, respect timeouts.

**Location:** Skill file in the yeehaw project, referenced in Claude config.
