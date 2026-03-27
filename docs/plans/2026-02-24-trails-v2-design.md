# Trails v2: Livestock Integration & Unified Trail Detail

**Date:** 2026-02-24
**Status:** Approved

## Problem

The `[t]` hotkey on the livestock detail page does nothing because `livestock.trails` is empty — there's no way to associate trails with livestock from the UI. Trail definitions exist in `~/.yeehaw/trails/` but are disconnected from livestock. The trails feature needs first-class integration into the livestock workflow.

## Design Overview

Three changes:

1. **Livestock detail page** gets a Trails panel alongside Sessions (50/50 horizontal split)
2. **Trail detail page** becomes a unified 3-panel view (Runs / Steps / Output)
3. **Trail creation wizard** lets users create and link trails inline from the Trails panel

## 1. Livestock Detail — Dual Panel Layout

Replace the single Sessions panel with two horizontal panels, Tab-switchable.

```
┌─ 🐄 rust-cli ──────────────────────────────────────────────────┐
│  (ASCII livestock header with metadata)                         │
│  Path: ~/Sites/Yeehaw/rust-cli  Branch: main  Barn: local      │
└─────────────────────────────────────────────────────────────────┘
┌─ Sessions ──── [c] claude [s] shell ─┐┌─ Trails ──────── [n] new ─┐
│  [1] Yeehaw-cli-claude    ● active   ││  › deploy-basic  ✓✓✓ 2m ago│
│  [2] Yeehaw-cli-shell     ○ idle     ││    test-suite    ✓✓✗ 1d ago│
│                                      ││    db-migration            │
└──────────────────────────────────────┘└────────────────────────────┘
```

### Trail List Item Rendering

Each trail row shows inline status from the most recent run:

- **Active run:** `▒ deploy-basic  ✓✓▒░ 2m14s` — per-step status icons + elapsed
- **Last run success:** `deploy-basic  ✓✓✓ 2m ago`
- **Last run failed:** `deploy-basic  ✓✓✗ 1d ago`
- **Never run:** `deploy-basic` (no metadata)

### Panel-Level Hotkeys

| Panel | Hints (shown in header when focused) |
|-------|--------------------------------------|
| Sessions | `[c] claude  [s] shell` |
| Trails | `[n] new  [d] unlink` |

Page-level bottom bar: `[l] logs  [e] edit  Tab switch  Esc back`

### Interaction

- **Tab** switches focus between Sessions and Trails
- **Enter** on a session: switches to that tmux window
- **Enter** on a trail: navigates to Trail Detail page
- **[n]** in Trails panel: opens trail creation wizard
- **[d]** in Trails panel: unlinks trail from livestock (does not delete the trail definition)

## 2. Trail Detail — Unified 3-Panel View

Consolidates the existing `trail_view` (execution) and `trail_history` (history) into one page. Accessed by pressing Enter on a trail in the livestock Trails panel.

```
┌─ Trail: deploy-basic ─────────────────────────────────────────────────┐
│ Basic deployment - pull latest and restart  │  Provider: native       │
│ Steps: 3  │  Used by: cli, web, rust-cli                             │
└───────────────────────────────────────────────────────────────────────┘
┌─ Runs ──────────────┐┌─ Steps ─────────────┐┌─ Output ──────────────┐
│ › ▒ 2/24 3:45p 2m14s││  ✓ Pull latest      ││ > Already up to date. │
│   ✓ 2/24 3:42p   3m ││  ✓ Install deps     ││ > npm: added 0 pkgs   │
│   ✗ 2/23 1:15p   2m ││  ▒ Restart svc      ││ > Restarting app...   │
│   ✓ 2/22 9:00a   4m ││                     ││ > Service started     │
│                      ││                     ││ > Listening on :3000  │
└──────────────────────┘└─────────────────────┘└───────────────────────┘
```

### Panel Layout

- **Runs** (25%): Chronological list, most recent first
- **Steps** (25%): Steps for the selected run
- **Output** (50%): Log output for the selected step

Tab cycles: Runs → Steps → Output → Runs

### Runs Panel

- Active run: `▒` icon + timestamp + elapsed time (auto-updating)
- Completed runs: `✓`/`✗` icon + timestamp + total duration
- Selecting a run updates Steps and Output panels

### Steps Panel

- Shows steps for the selected run with status indicators
- Status icons: `░` pending, `▒` running, `✓` success, `✗` failed
- For active runs, auto-follows the currently running step
- Selecting a step updates the Output panel

### Output Panel

- Shows log output for the selected step of the selected run
- Live-streams for active runs (auto-scroll)
- Scrollable with `j/k` and `g/G` when focused

### Trail Detail Hotkeys

| Key | Action | Level |
|-----|--------|-------|
| `r` | Start new trail run | Page |
| `x` | Cancel active run (shown only when running) | Page |
| `e` | Edit trail config | Page |
| `Tab` | Cycle panel focus | Page |
| `j/k` | Navigate within focused panel | Panel |
| `g/G` | Jump to top/bottom (output) | Panel |
| `Esc` | Back to livestock detail | Page |

### Header

Shows trail name, description, provider, step count, and which livestock use this trail.

## 3. Trail Creation Wizard

Triggered by `[n]` in the Trails panel on livestock detail.

### Flow

1. **Trail Name** — text input, validates uniqueness
2. **Description** — text input (optional, Enter to skip)
3. **Step loop** (repeats):
   - **Step Name** — text input
   - **Command** — text input (can use `$REPO_PATH`, `$BRANCH`, etc.)
   - **Timeout** — text input (default: 60s)
   - After each step: `[Enter] Add another step` / `[Esc] Save trail`

### On Save

1. Write trail definition to `~/.yeehaw/trails/{name}.yaml`
2. Add trail name to `livestock.trails` in the project YAML
3. Reload state, stay on livestock detail page

### Validation

- Trail name must not conflict with existing trail YAML files
- At least one step required
- Step name and command can't be empty

## 4. Runner Changes — Env Var Injection

Replace `{{template}}` syntax with environment variable injection for new trails. Keep template resolution as backward-compatible fallback.

### Auto-Injected Environment Variables

| Env Var | Source | Default |
|---------|--------|---------|
| `$NAME` | livestock.name | — |
| `$REPO_PATH` | livestock.path (expanded) | — |
| `$BRANCH` | livestock.branch | `"main"` |
| `$BARN` | barn.host | `"localhost"` |
| `$BARN_USER` | barn.user | `"root"` |

### Execution Order

1. Resolve any `{{var}}` templates in the command (backward compat)
2. Set environment variables on the spawned process
3. Execute the command

## 5. Navigation & AppView Changes

### Navigation Flow

```
Global → Project → Livestock Detail
                      ├── Sessions panel (Enter → switch tmux window)
                      └── Trails panel
                            ├── [n] → creation wizard (inline form)
                            ├── [d] → unlink trail
                            └── Enter → Trail Detail (3-panel)
                                          ├── [r] → run trail
                                          ├── [x] → cancel run
                                          ├── [e] → edit trail config
                                          └── Esc → back to Livestock Detail
```

### AppView Enum Changes

- `AppView::Trail` stays, represents the unified 3-panel view
- `AppView::TrailHistory` **removed** — consolidated into Trail view

### Data Model

No changes to the `Livestock` or `Trail` structs. The `trails: Vec<String>` field on Livestock is already the correct model (global trail definitions linked per-livestock).

## 6. Trail Scope

Trail definitions are **global and reusable**:

- Stored in `~/.yeehaw/trails/{name}.yaml`
- Linked to livestock via `livestock.trails: Vec<String>`
- Multiple livestock can share the same trail definition
- `[d]` on trails panel **unlinks** (does not delete the definition)
- `[e]` on trail detail page edits the global definition (shows "Used by: X, Y, Z" in header as a visibility aid)

## Files to Modify

| File | Change |
|------|--------|
| `src/views/livestock_detail.rs` | Add Trails panel, Tab switching, [n]/[d] handlers, FocusedPanel enum |
| `src/views/trail_view.rs` | Rewrite as unified 3-panel view (Runs/Steps/Output) |
| `src/views/trail_history.rs` | Remove (consolidated into trail_view) |
| `src/app.rs` | Update livestock detail handler, remove TrailHistory handling, update bottom bar hints |
| `src/types.rs` | Remove `AppView::TrailHistory` variant |
| `src/trails/runner.rs` | Add env var injection alongside template resolution |
| `src/trails/native.rs` | Pass env vars to spawned SSH commands |
| `src/config.rs` | Add helper to link/unlink trail from livestock |
