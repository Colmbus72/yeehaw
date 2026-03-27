# Trails v2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate trails as a first-class panel on the livestock detail page, consolidate trail execution and history into a unified 3-panel view, and add inline trail creation.

**Architecture:** The livestock detail view gains a side-by-side Sessions/Trails layout (matching herd_detail.rs pattern). The separate trail_view and trail_history views merge into one 3-panel view (Runs/Steps/Output). Trail creation uses an inline wizard triggered by [n]. The runner gains env var injection alongside backward-compatible template resolution.

**Tech Stack:** Rust, ratatui, crossterm, serde_yaml, tokio mpsc channels. Follows existing patterns from herd_detail.rs (dual-panel), project_context.rs (creation wizard), and trail_view.rs (streaming output).

**Design doc:** `docs/plans/2026-02-24-trails-v2-design.md`

---

### Task 1: Config Helpers for Trail Linking

Add utility functions to config.rs for linking/unlinking trails from livestock, and loading all available trails.

**Files:**
- Modify: `src/config.rs`

**Step 1: Add `link_trail_to_livestock` function**

Add after `load_trails_for_livestock` (line 414) in `src/config.rs`:

```rust
pub fn link_trail_to_livestock(project_name: &str, livestock_name: &str, trail_name: &str) -> Result<()> {
    validate_name(project_name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if let Some(ls) = project.livestock.iter_mut().find(|l| l.name == livestock_name) {
        if !ls.trails.contains(&trail_name.to_string()) {
            ls.trails.push(trail_name.to_string());
        }
    } else {
        anyhow::bail!("Livestock '{}' not found in project '{}'", livestock_name, project_name);
    }

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    fs::write(&path, new_content).context("Failed to write project file")?;
    Ok(())
}

pub fn unlink_trail_from_livestock(project_name: &str, livestock_name: &str, trail_name: &str) -> Result<()> {
    validate_name(project_name, "project")?;
    let path = projects_dir().join(format!("{}.yaml", project_name));
    let content = fs::read_to_string(&path).context("Failed to read project file")?;
    let mut project: Project = serde_yaml::from_str(&content).context("Failed to parse project")?;

    if let Some(ls) = project.livestock.iter_mut().find(|l| l.name == livestock_name) {
        ls.trails.retain(|t| t != trail_name);
    } else {
        anyhow::bail!("Livestock '{}' not found in project '{}'", livestock_name, project_name);
    }

    let new_content = serde_yaml::to_string(&project).context("Failed to serialize project")?;
    fs::write(&path, new_content).context("Failed to write project file")?;
    Ok(())
}
```

**Step 2: Verify compilation**

Run: `cargo check`
Expected: Compiles with no errors.

**Step 3: Commit**

```bash
git add src/config.rs
git commit -m "Add config helpers for linking/unlinking trails to livestock"
```

---

### Task 2: Livestock Detail Dual-Panel Layout

Transform the livestock detail page from a single Sessions panel into a side-by-side Sessions + Trails layout with Tab switching. This follows the herd_detail.rs pattern exactly.

**Files:**
- Modify: `src/views/livestock_detail.rs`
- Modify: `src/app.rs` (handler + rendering)

**Step 1: Add FocusedPanel enum and update struct**

In `src/views/livestock_detail.rs`, add `FocusedPanel` enum after the imports (before `LivestockAction`), and add new action variants. Add `trails_state` and `focused_panel` fields to the struct.

Replace the `LivestockAction` enum (lines 15-23) with:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Sessions,
    Trails,
}

pub enum LivestockAction {
    None,
    OpenLogs,
    OpenClaude,
    OpenShell,
    OpenTrail(usize),
    SelectWindow(usize),
    UpdateLivestock(Livestock),
    CreateTrail,
    UnlinkTrail(usize),
}
```

Replace the `LivestockDetailView` struct (lines 36-46) with:

```rust
pub struct LivestockDetailView {
    focused_panel: FocusedPanel,
    sessions_state: ListState,
    trails_state: ListState,
    edit_mode: EditMode,
    text_input: TextInput,
    edit_name: String,
    edit_path: String,
    edit_repo: String,
    edit_branch: String,
    edit_log_path: String,
}
```

Update `new()` (lines 49-60):

```rust
pub fn new() -> Self {
    Self {
        focused_panel: FocusedPanel::Sessions,
        sessions_state: ListState::new(),
        trails_state: ListState::new(),
        edit_mode: EditMode::Normal,
        text_input: TextInput::new(""),
        edit_name: String::new(),
        edit_path: String::new(),
        edit_repo: String::new(),
        edit_branch: String::new(),
        edit_log_path: String::new(),
    }
}
```

**Step 2: Update handle_input for dual-panel navigation**

Replace the `handle_input` method (lines 134-185). The new version needs to accept `trails_count` and route hotkeys based on `focused_panel`. Add Tab switching. Move session-specific and trail-specific hotkeys into per-panel match arms.

```rust
pub fn handle_input(
    &mut self,
    key: KeyCode,
    _project: &Project,
    livestock: &Livestock,
    session_count: usize,
    trails_count: usize,
) -> LivestockAction {
    if self.edit_mode != EditMode::Normal {
        if key == KeyCode::Esc {
            self.cancel_edit();
            return LivestockAction::None;
        }
        let submitted = self.text_input.handle_input(key);
        if submitted {
            if self.advance_field().is_none() {
                let updated = self.build_updated(livestock);
                self.edit_mode = EditMode::Normal;
                return LivestockAction::UpdateLivestock(updated);
            }
        }
        return LivestockAction::None;
    }

    // Tab switches panels
    if key == KeyCode::Tab {
        self.focused_panel = match self.focused_panel {
            FocusedPanel::Sessions => FocusedPanel::Trails,
            FocusedPanel::Trails => FocusedPanel::Sessions,
        };
        return LivestockAction::None;
    }

    // Page-level hotkeys (work regardless of panel)
    match key {
        KeyCode::Char('e') => {
            self.start_edit(livestock);
            return LivestockAction::None;
        }
        KeyCode::Char('l') => return LivestockAction::OpenLogs,
        _ => {}
    }

    // Panel-specific hotkeys
    match self.focused_panel {
        FocusedPanel::Sessions => match key {
            KeyCode::Char('c') => LivestockAction::OpenClaude,
            KeyCode::Char('s') => LivestockAction::OpenShell,
            KeyCode::Char('j') | KeyCode::Down => {
                self.sessions_state.select_next(session_count);
                LivestockAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sessions_state.select_prev();
                LivestockAction::None
            }
            KeyCode::Enter => LivestockAction::SelectWindow(self.sessions_state.selected),
            _ => LivestockAction::None,
        },
        FocusedPanel::Trails => match key {
            KeyCode::Char('n') => LivestockAction::CreateTrail,
            KeyCode::Char('d') => {
                if trails_count > 0 {
                    LivestockAction::UnlinkTrail(self.trails_state.selected)
                } else {
                    LivestockAction::None
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.trails_state.select_next(trails_count);
                LivestockAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.trails_state.select_prev();
                LivestockAction::None
            }
            KeyCode::Enter => {
                if trails_count > 0 {
                    LivestockAction::OpenTrail(self.trails_state.selected)
                } else {
                    LivestockAction::None
                }
            }
            _ => LivestockAction::None,
        },
    }
}
```

**Step 3: Update render method for dual-panel layout**

Replace the render method (lines 187-252). Use the herd_detail.rs pattern: header on top, two horizontal panels below (50/50 split).

The render method now needs `trails` and `trail_runs` as parameters to show inline status.

```rust
pub fn render(
    &mut self,
    frame: &mut Frame,
    area: Rect,
    project: &Project,
    livestock: &Livestock,
    windows: &[TmuxWindow],
    trails: &[crate::trails::Trail],
    trail_runs: &[(String, Option<crate::trails::TrailRun>)], // (trail_name, most_recent_run)
) {
    if self.edit_mode != EditMode::Normal {
        self.render_edit_form(frame, area, project, livestock);
        return;
    }

    // Filter windows for this livestock
    let pattern = format!("{}-{}", project.name, livestock.name);
    let session_windows: Vec<_> = windows
        .iter()
        .filter(|w| w.index > 0 && w.name.contains(&pattern))
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(14), // ASCII header
            Constraint::Min(1),     // panels
        ])
        .split(area);

    livestock_header::render_livestock_header(frame, chunks[0], livestock, &project.name);

    // Two panels side by side
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .margin(1)
        .split(chunks[1]);

    // Sessions panel (left)
    let sessions_panel = Panel {
        title: "Sessions",
        focused: self.focused_panel == FocusedPanel::Sessions,
        hints: if self.focused_panel == FocusedPanel::Sessions {
            Some("[c] claude  [s] shell")
        } else {
            None
        },
    };
    let sessions_inner = sessions_panel.render(frame, panels[0]);

    let session_items: Vec<ListItem> = session_windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let status_info = tmux::get_window_status(w);
            ListItem {
                id: w.index.to_string(),
                label: format!("[{}] {}", i + 1, w.name),
                status: Some(if w.active {
                    ItemStatus::Active
                } else {
                    ItemStatus::Inactive
                }),
                meta: Some(status_info.text),
                actions: vec![],
            }
        })
        .collect();

    list::render_list(
        frame,
        sessions_inner,
        &session_items,
        &mut self.sessions_state,
        self.focused_panel == FocusedPanel::Sessions,
        Some(10),
    );

    // Trails panel (right)
    let trails_panel = Panel {
        title: "Trails",
        focused: self.focused_panel == FocusedPanel::Trails,
        hints: if self.focused_panel == FocusedPanel::Trails {
            Some("[n] new  [d] unlink")
        } else {
            None
        },
    };
    let trails_inner = trails_panel.render(frame, panels[1]);

    let trail_items: Vec<ListItem> = trails.iter().map(|t| {
        // Find the most recent run for this trail
        let run_meta = trail_runs.iter()
            .find(|(name, _)| name == &t.name)
            .and_then(|(_, run)| run.as_ref())
            .map(|run| {
                let step_icons: String = run.steps.iter().map(|s| {
                    match s.status.as_str() {
                        "success" => "\u{2713}",
                        "failed" => "\u{2717}",
                        "running" => "\u{2592}",
                        _ => "\u{2591}",
                    }
                }).collect::<Vec<_>>().join("");

                let time_ago = format_time_ago(&run.started_at);
                format!("{} {}", step_icons, time_ago)
            })
            .unwrap_or_default();

        let status = trail_runs.iter()
            .find(|(name, _)| name == &t.name)
            .and_then(|(_, run)| run.as_ref())
            .map(|run| match run.status.as_str() {
                "success" => ItemStatus::Active,
                "failed" | "cancelled" => ItemStatus::Error,
                "running" => ItemStatus::Inactive,
                _ => ItemStatus::Inactive,
            });

        ListItem {
            id: t.name.clone(),
            label: t.name.clone(),
            status,
            meta: if run_meta.is_empty() { None } else { Some(run_meta) },
            actions: vec![],
        }
    }).collect();

    list::render_list(
        frame,
        trails_inner,
        &trail_items,
        &mut self.trails_state,
        self.focused_panel == FocusedPanel::Trails,
        Some(10),
    );
}
```

Also add this helper function at the bottom of the file (outside the impl block):

```rust
fn format_time_ago(timestamp: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        let now = chrono::Utc::now();
        let duration = now.signed_duration_since(dt.with_timezone(&chrono::Utc));
        if duration.num_minutes() < 1 {
            "just now".to_string()
        } else if duration.num_minutes() < 60 {
            format!("{}m ago", duration.num_minutes())
        } else if duration.num_hours() < 24 {
            format!("{}h ago", duration.num_hours())
        } else {
            format!("{}d ago", duration.num_days())
        }
    } else {
        String::new()
    }
}
```

**Step 4: Update app.rs — livestock detail handler**

In `src/app.rs`, update `handle_livestock_detail_input` (starts at line 1159). The `handle_input` call now needs `trails_count`:

At line 1168, after computing `session_count`, add:
```rust
let trails = config::load_trails_for_livestock(&livestock);
let trails_count = trails.len();
```

Update line 1170 to pass the new parameter:
```rust
let action = app.livestock_view.handle_input(key, &project, &livestock, session_count, trails_count);
```

Add new match arms after `LivestockAction::OpenTrail(idx)` (after line 1242):

```rust
LivestockAction::CreateTrail => {
    // Will be handled in Task 3 (trail creation wizard)
    // For now, just show a placeholder error
    app.error = Some("Trail creation not yet implemented".to_string());
}
LivestockAction::UnlinkTrail(idx) => {
    let trails = config::load_trails_for_livestock(&livestock);
    if let Some(trail) = trails.get(idx) {
        match config::unlink_trail_from_livestock(&project.name, &livestock.name, &trail.name) {
            Ok(()) => {
                app.reload();
                // Re-navigate to refresh livestock data
                if let Some(updated_project) = app.projects.iter().find(|p| p.name == project.name) {
                    if let Some(updated_livestock) = updated_project.livestock.iter().find(|l| l.name == livestock.name) {
                        app.navigate(AppView::Livestock {
                            project: updated_project.clone(),
                            livestock: updated_livestock.clone(),
                            source: source.clone(),
                            source_barn: source_barn.clone(),
                        });
                    }
                }
            }
            Err(e) => {
                app.error = Some(format!("Failed to unlink trail: {}", e));
            }
        }
    }
}
```

**Step 5: Update app.rs — livestock render call**

In `src/app.rs`, find the `render_view` function's `AppView::Livestock` arm (around line 1560). Update to pass trails and trail_runs:

```rust
AppView::Livestock { project, livestock, .. } => {
    let project = project.clone();
    let livestock = livestock.clone();
    let trails = config::load_trails_for_livestock(&livestock);
    let trail_runs: Vec<(String, Option<crate::trails::TrailRun>)> = trails.iter().map(|t| {
        let runs = config::load_trail_runs(&livestock.name, &t.name);
        (t.name.clone(), runs.into_iter().next())
    }).collect();
    app.livestock_view.render(frame, area, &project, &livestock, &app.windows, &trails, &trail_runs);
}
```

**Step 6: Update bottom bar hints for Livestock view**

In `src/app.rs`, update the `AppView::Livestock` bottom bar hints (lines 1701-1709):

```rust
AppView::Livestock { .. } => vec![
    ("l", "logs"),
    ("e", "edit"),
    ("Tab", "switch panel"),
    ("Esc", "back"),
    ("?", "help"),
],
```

(Removed `c`, `s`, `t` from page-level since they're now panel-level hints.)

**Step 7: Verify compilation**

Run: `cargo check`
Expected: Compiles. Fix any import issues (add `use crate::trails;` etc. if needed).

**Step 8: Commit**

```bash
git add src/views/livestock_detail.rs src/app.rs
git commit -m "Add dual-panel layout to livestock detail (Sessions + Trails)"
```

---

### Task 3: Trail Creation Wizard

Add an inline multi-step form for creating trails from the Trails panel.

**Files:**
- Modify: `src/views/livestock_detail.rs`
- Modify: `src/app.rs`

**Step 1: Add InputMode enum and wizard state to livestock_detail.rs**

Add a new enum after `EditMode` and extend the struct:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum WizardMode {
    Inactive,
    TrailName,
    TrailDescription,
    StepName,
    StepCommand,
    StepTimeout,
}
```

Add wizard fields to `LivestockDetailView`:

```rust
pub struct LivestockDetailView {
    focused_panel: FocusedPanel,
    sessions_state: ListState,
    trails_state: ListState,
    edit_mode: EditMode,
    text_input: TextInput,
    edit_name: String,
    edit_path: String,
    edit_repo: String,
    edit_branch: String,
    edit_log_path: String,
    // Trail creation wizard
    wizard_mode: WizardMode,
    new_trail_name: String,
    new_trail_description: String,
    new_trail_steps: Vec<crate::trails::TrailStep>,
    new_step_name: String,
    new_step_command: String,
}
```

Initialize in `new()`:
```rust
wizard_mode: WizardMode::Inactive,
new_trail_name: String::new(),
new_trail_description: String::new(),
new_trail_steps: Vec::new(),
new_step_name: String::new(),
new_step_command: String::new(),
```

**Step 2: Add `is_in_wizard` method and wizard input handling**

```rust
pub fn is_in_wizard(&self) -> bool {
    self.wizard_mode != WizardMode::Inactive
}
```

In `handle_input`, add a wizard mode check at the top (after the edit_mode check):

```rust
if self.wizard_mode != WizardMode::Inactive {
    return self.handle_wizard_input(key);
}
```

Add the wizard handler method:

```rust
fn handle_wizard_input(&mut self, key: KeyCode) -> LivestockAction {
    if key == KeyCode::Esc {
        if self.wizard_mode == WizardMode::StepName && !self.new_trail_steps.is_empty() {
            // Esc during step entry with existing steps = save trail
            self.wizard_mode = WizardMode::Inactive;
            return self.finish_trail_wizard();
        }
        // Esc at any other point = cancel
        self.wizard_mode = WizardMode::Inactive;
        return LivestockAction::None;
    }

    let submitted = self.text_input.handle_input(key);
    if !submitted {
        return LivestockAction::None;
    }

    let value = self.text_input.value.trim().to_string();

    match self.wizard_mode {
        WizardMode::TrailName => {
            if value.is_empty() {
                return LivestockAction::None;
            }
            self.new_trail_name = value;
            self.wizard_mode = WizardMode::TrailDescription;
            self.text_input = TextInput::new("");
        }
        WizardMode::TrailDescription => {
            self.new_trail_description = value;
            self.wizard_mode = WizardMode::StepName;
            self.text_input = TextInput::new("");
        }
        WizardMode::StepName => {
            if value.is_empty() {
                return LivestockAction::None;
            }
            self.new_step_name = value;
            self.wizard_mode = WizardMode::StepCommand;
            self.text_input = TextInput::new("");
        }
        WizardMode::StepCommand => {
            if value.is_empty() {
                return LivestockAction::None;
            }
            self.new_step_command = value;
            self.wizard_mode = WizardMode::StepTimeout;
            self.text_input = TextInput::new("60");
        }
        WizardMode::StepTimeout => {
            let timeout: u64 = value.parse().unwrap_or(60);
            self.new_trail_steps.push(crate::trails::TrailStep {
                name: self.new_step_name.clone(),
                run: self.new_step_command.clone(),
                timeout,
            });
            // Loop back for another step
            self.wizard_mode = WizardMode::StepName;
            self.text_input = TextInput::new("");
        }
        WizardMode::Inactive => {}
    }

    LivestockAction::None
}

fn start_trail_wizard(&mut self) {
    self.wizard_mode = WizardMode::TrailName;
    self.new_trail_name.clear();
    self.new_trail_description.clear();
    self.new_trail_steps.clear();
    self.new_step_name.clear();
    self.new_step_command.clear();
    self.text_input = TextInput::new("");
}

fn finish_trail_wizard(&mut self) -> LivestockAction {
    if self.new_trail_steps.is_empty() || self.new_trail_name.is_empty() {
        return LivestockAction::None;
    }
    let trail = crate::trails::Trail {
        name: self.new_trail_name.clone(),
        description: if self.new_trail_description.is_empty() {
            None
        } else {
            Some(self.new_trail_description.clone())
        },
        provider: "native".to_string(),
        steps: self.new_trail_steps.clone(),
    };
    LivestockAction::SaveNewTrail(trail)
}
```

**Step 3: Add SaveNewTrail action variant**

Add to `LivestockAction`:
```rust
SaveNewTrail(crate::trails::Trail),
```

Update the `CreateTrail` handler in `handle_input`'s Trails panel match:
```rust
KeyCode::Char('n') => {
    self.start_trail_wizard();
    LivestockAction::None
}
```

**Step 4: Add wizard rendering**

Add a `render_trail_wizard` method and call it from `render` when wizard is active:

In `render`, after the edit mode check, add:
```rust
if self.wizard_mode != WizardMode::Inactive {
    self.render_trail_wizard(frame, area, project, livestock);
    return;
}
```

```rust
fn render_trail_wizard(
    &self,
    frame: &mut Frame,
    area: Rect,
    project: &Project,
    livestock: &Livestock,
) {
    let (label, step_info) = match self.wizard_mode {
        WizardMode::TrailName => ("Trail Name:", "Step 1: Name"),
        WizardMode::TrailDescription => ("Description (optional):", "Step 2: Description"),
        WizardMode::StepName => {
            let n = self.new_trail_steps.len() + 1;
            // Can't use format! in const, handle inline
            ("Step Name:", "Adding Steps")
        }
        WizardMode::StepCommand => ("Command:", "Adding Steps"),
        WizardMode::StepTimeout => ("Timeout (seconds):", "Adding Steps"),
        WizardMode::Inactive => return,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // header
            Constraint::Length(2),  // title
            Constraint::Length(self.new_trail_steps.len() as u16 + 3), // completed fields
            Constraint::Length(1),  // label
            Constraint::Length(1),  // input
            Constraint::Length(2),  // hints
            Constraint::Min(1),
        ])
        .split(area);

    use crate::components::header;
    header::render_simple_header(
        frame,
        chunks[0],
        &format!("{} / {} / New Trail", project.name, livestock.name),
        Some("creating"),
    );

    let title = Paragraph::new(format!("  {}", step_info))
        .style(Style::default().fg(BRAND_COLOR).add_modifier(Modifier::BOLD));
    frame.render_widget(title, chunks[1]);

    // Show completed fields
    let mut completed: Vec<Line> = Vec::new();
    if !self.new_trail_name.is_empty() && self.wizard_mode != WizardMode::TrailName {
        completed.push(Line::from(vec![
            Span::styled("  Name: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&self.new_trail_name),
        ]));
    }
    if self.wizard_mode != WizardMode::TrailName && self.wizard_mode != WizardMode::TrailDescription {
        if !self.new_trail_description.is_empty() {
            completed.push(Line::from(vec![
                Span::styled("  Desc: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&self.new_trail_description),
            ]));
        }
    }
    if !self.new_trail_steps.is_empty() {
        completed.push(Line::from(""));
        for (i, step) in self.new_trail_steps.iter().enumerate() {
            completed.push(Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), Style::default().fg(Color::DarkGray)),
                Span::raw(&step.name),
                Span::styled(format!(" ({}s)", step.timeout), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    if !completed.is_empty() {
        frame.render_widget(Paragraph::new(completed), chunks[2]);
    }

    let label_text = Paragraph::new(format!("  {}", label))
        .style(Style::default().fg(Color::White));
    frame.render_widget(label_text, chunks[3]);

    let input_area = Rect {
        x: chunks[4].x + 4,
        y: chunks[4].y,
        width: chunks[4].width.saturating_sub(4),
        height: 1,
    };
    self.text_input.render(frame, input_area);

    let hint = if self.wizard_mode == WizardMode::StepName && !self.new_trail_steps.is_empty() {
        "  Enter: next field  Esc: save trail"
    } else {
        "  Enter: next field  Esc: cancel"
    };
    let hints = Paragraph::new(hint)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hints, chunks[5]);
}
```

**Step 5: Wire SaveNewTrail in app.rs**

Replace the `CreateTrail` placeholder in app.rs with real handling, and add `SaveNewTrail`:

```rust
LivestockAction::CreateTrail => {
    // Handled inside view (starts wizard)
}
LivestockAction::SaveNewTrail(trail) => {
    // Save trail definition
    match config::save_trail(&trail) {
        Ok(()) => {
            // Link to livestock
            match config::link_trail_to_livestock(&project.name, &livestock.name, &trail.name) {
                Ok(()) => {
                    app.reload();
                    // Re-navigate to refresh
                    if let Some(updated_project) = app.projects.iter().find(|p| p.name == project.name) {
                        if let Some(updated_livestock) = updated_project.livestock.iter().find(|l| l.name == livestock.name) {
                            app.navigate(AppView::Livestock {
                                project: updated_project.clone(),
                                livestock: updated_livestock.clone(),
                                source: source.clone(),
                                source_barn: source_barn.clone(),
                            });
                        }
                    }
                }
                Err(e) => {
                    app.error = Some(format!("Failed to link trail: {}", e));
                }
            }
        }
        Err(e) => {
            app.error = Some(format!("Failed to save trail: {}", e));
        }
    }
}
```

Also update the Esc guard in the Livestock view dispatch (around line 530-534) to respect wizard mode:

```rust
AppView::Livestock { .. } => {
    if !app.livestock_view.is_editing() && !app.livestock_view.is_in_wizard() {
        if key.code == KeyCode::Esc { app.go_back(); continue; }
    }
    handle_livestock_detail_input(&mut app, key.code);
}
```

**Step 6: Verify compilation**

Run: `cargo check`

**Step 7: Commit**

```bash
git add src/views/livestock_detail.rs src/app.rs
git commit -m "Add trail creation wizard ([n] new in Trails panel)"
```

---

### Task 4: Unified Trail Detail View (3-Panel)

Rewrite trail_view.rs as a consolidated 3-panel view (Runs / Steps / Output). This absorbs functionality from trail_history.rs.

**Files:**
- Modify: `src/views/trail_view.rs`

**Step 1: Rewrite trail_view.rs with 3-panel layout**

Replace the entire `src/views/trail_view.rs` file. Key changes:
- `FocusedPanel` becomes `{ Runs, Steps, Output }` (3 panels)
- Remove `ShowHistory` action (history is inline)
- Add `runs_state` for the Runs panel
- `enter()` loads run history
- Render 3 horizontal panels: 25% / 25% / 50%

```rust
use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::panel::Panel;
use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::trail_header;
use crate::components::trail_steps::{self, StepItem, StepListState};
use crate::config;
use crate::trails::{Trail, TrailRun};
use crate::trails::provider::StepStatus;
use crate::types::Livestock;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FocusedPanel {
    Runs,
    Steps,
    Output,
}

pub enum TrailViewAction {
    None,
    Back,
    RunTrail,
    CancelTrail,
}

pub struct TrailView {
    focused_panel: FocusedPanel,
    runs_state: ListState,
    step_state: StepListState,
    output_scroll: usize,
    tick: u64,
    /// Cached run history (reloaded on enter and after run completes)
    pub runs: Vec<TrailRun>,
    /// Which run is selected (index into self.runs)
    pub selected_run: usize,
    /// Live step statuses for the active run (index 0 if running)
    pub step_statuses: Vec<StepStatus>,
    /// Live output lines per step for the active run
    pub step_outputs: Vec<Vec<String>>,
    /// Whether a trail is currently running
    pub running: bool,
    pub auto_follow: bool,
}

impl TrailView {
    pub fn new() -> Self {
        Self {
            focused_panel: FocusedPanel::Runs,
            runs_state: ListState::new(),
            step_state: StepListState::new(),
            output_scroll: 0,
            tick: 0,
            runs: vec![],
            selected_run: 0,
            step_statuses: vec![],
            step_outputs: vec![],
            running: false,
            auto_follow: true,
        }
    }

    /// Initialize view for a trail + livestock context. Loads run history.
    pub fn enter(&mut self, trail: &Trail, livestock: &Livestock) {
        self.runs = config::load_trail_runs(&livestock.name, &trail.name);
        self.runs_state = ListState::new();
        self.selected_run = 0;
        self.step_state = StepListState::new();
        self.output_scroll = 0;
        self.running = false;
        self.auto_follow = true;
        self.step_statuses = vec![StepStatus::Pending; trail.steps.len()];
        self.step_outputs = vec![vec![]; trail.steps.len()];
    }

    /// Call when starting a new run. Inserts an "active" placeholder at the top of runs.
    pub fn start_run(&mut self, trail: &Trail) {
        self.step_statuses = vec![StepStatus::Pending; trail.steps.len()];
        self.step_outputs = vec![vec![]; trail.steps.len()];
        self.running = true;
        self.auto_follow = true;
        self.selected_run = 0;
        self.runs_state = ListState::new();
        self.step_state = StepListState::new();
        self.output_scroll = 0;
    }

    /// Called when the active run finishes. Reloads history.
    pub fn finish_run(&mut self, livestock_name: &str, trail_name: &str) {
        self.running = false;
        self.runs = config::load_trail_runs(livestock_name, trail_name);
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.auto_follow && self.running && self.selected_run == 0 {
            for (i, status) in self.step_statuses.iter().enumerate() {
                if *status == StepStatus::Running {
                    self.step_state.selected = i;
                    if let Some(lines) = self.step_outputs.get(i) {
                        self.output_scroll = lines.len().saturating_sub(20);
                    }
                    break;
                }
            }
        }
    }

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
            KeyCode::Char('x') if self.running => return TrailViewAction::CancelTrail,
            KeyCode::Tab => {
                self.focused_panel = match self.focused_panel {
                    FocusedPanel::Runs => FocusedPanel::Steps,
                    FocusedPanel::Steps => FocusedPanel::Output,
                    FocusedPanel::Output => FocusedPanel::Runs,
                };
                if self.focused_panel != FocusedPanel::Output {
                    self.auto_follow = false;
                }
            }
            _ => {}
        }

        match self.focused_panel {
            FocusedPanel::Runs => match key {
                KeyCode::Char('j') | KeyCode::Down => {
                    let count = if self.running { self.runs.len() + 1 } else { self.runs.len() };
                    self.runs_state.select_next(count);
                    self.selected_run = self.runs_state.selected;
                    self.step_state = StepListState::new();
                    self.output_scroll = 0;
                    self.auto_follow = false;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.runs_state.select_prev();
                    self.selected_run = self.runs_state.selected;
                    self.step_state = StepListState::new();
                    self.output_scroll = 0;
                    self.auto_follow = false;
                }
                _ => {}
            },
            FocusedPanel::Steps => match key {
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
            },
            FocusedPanel::Output => match key {
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
                    let lines = self.get_selected_output();
                    self.output_scroll = lines.len().saturating_sub(20);
                    self.auto_follow = false;
                }
                _ => {}
            },
        }

        TrailViewAction::None
    }

    /// Get step data for the currently selected run.
    fn get_selected_steps(&self, trail: &Trail) -> Vec<StepItem> {
        if self.running && self.selected_run == 0 {
            // Active run — use live statuses
            trail.steps.iter().enumerate().map(|(i, s)| {
                StepItem {
                    name: s.name.clone(),
                    status: self.step_statuses.get(i).cloned().unwrap_or(StepStatus::Pending),
                }
            }).collect()
        } else {
            // Historical run
            let run_idx = if self.running { self.selected_run - 1 } else { self.selected_run };
            if let Some(run) = self.runs.get(run_idx) {
                run.steps.iter().map(|s| {
                    StepItem {
                        name: s.name.clone(),
                        status: match s.status.as_str() {
                            "success" => StepStatus::Success,
                            "failed" => StepStatus::Failed { exit_code: s.exit_code.unwrap_or(-1) },
                            "running" => StepStatus::Running,
                            _ => StepStatus::Pending,
                        },
                    }
                }).collect()
            } else {
                vec![]
            }
        }
    }

    /// Get output lines for the currently selected step of the selected run.
    fn get_selected_output(&self) -> Vec<String> {
        if self.running && self.selected_run == 0 {
            // Active run — use live output
            self.step_outputs.get(self.step_state.selected)
                .cloned()
                .unwrap_or_default()
        } else {
            // Historical run — load from log file
            let run_idx = if self.running { self.selected_run - 1 } else { self.selected_run };
            if let Some(run) = self.runs.get(run_idx) {
                // Construct the run directory path from run metadata
                let timestamp = run.started_at.replace(':', "-").replace('.', "-");
                let run_dir = config::trail_run_dir_for(&run.livestock, &run.trail, &timestamp);
                config::load_trail_step_log(&run_dir, self.step_state.selected)
                    .map(|log| log.lines().map(String::from).collect())
                    .unwrap_or_default()
            } else {
                vec![]
            }
        }
    }

    pub fn render(
        &mut self,
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

        trail_header::render_trail_header(frame, chunks[0], trail, livestock);

        // Three panels: Runs (25%) / Steps (25%) / Output (50%)
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(50),
            ])
            .margin(1)
            .split(chunks[1]);

        self.render_runs_panel(frame, panels[0], trail);
        self.render_steps_panel(frame, panels[1], trail);
        self.render_output_panel(frame, panels[2]);
    }

    fn render_runs_panel(&mut self, frame: &mut Frame, area: Rect, trail: &Trail) {
        let panel = Panel {
            title: "Runs",
            focused: self.focused_panel == FocusedPanel::Runs,
            hints: if self.focused_panel == FocusedPanel::Runs {
                Some("[r] run")
            } else {
                None
            },
        };
        let inner = panel.render(frame, area);

        let mut items: Vec<ListItem> = Vec::new();

        // If running, show active run as first item
        if self.running {
            let step_icons: String = self.step_statuses.iter().map(|s| {
                match s {
                    StepStatus::Success => "\u{2713}",
                    StepStatus::Failed { .. } => "\u{2717}",
                    StepStatus::Running => "\u{2592}",
                    _ => "\u{2591}",
                }
            }).collect::<Vec<_>>().join("");

            items.push(ListItem {
                id: "active".to_string(),
                label: format!("\u{2592} running {}", step_icons),
                status: Some(ItemStatus::Inactive),
                meta: None,
                actions: vec![],
            });
        }

        // Historical runs
        for run in &self.runs {
            let status_icon = match run.status.as_str() {
                "success" => "\u{2713}",
                "failed" | "cancelled" => "\u{2717}",
                "running" => "\u{2592}",
                _ => "?",
            };

            let date_display = format_run_timestamp(&run.started_at);

            let step_icons: String = run.steps.iter().map(|s| {
                match s.status.as_str() {
                    "success" => "\u{2713}",
                    "failed" => "\u{2717}",
                    "running" => "\u{2592}",
                    _ => "\u{2591}",
                }
            }).collect::<Vec<_>>().join("");

            items.push(ListItem {
                id: run.started_at.clone(),
                label: format!("{} {}", status_icon, date_display),
                status: Some(match run.status.as_str() {
                    "success" => ItemStatus::Active,
                    "failed" | "cancelled" => ItemStatus::Error,
                    _ => ItemStatus::Inactive,
                }),
                meta: Some(step_icons),
                actions: vec![],
            });
        }

        list::render_list(frame, inner, &items, &mut self.runs_state, self.focused_panel == FocusedPanel::Runs, Some(20));
    }

    fn render_steps_panel(&mut self, frame: &mut Frame, area: Rect, trail: &Trail) {
        let panel = Panel {
            title: "Steps",
            focused: self.focused_panel == FocusedPanel::Steps,
            hints: None,
        };
        let inner = panel.render(frame, area);

        let step_items = self.get_selected_steps(trail);

        trail_steps::render_step_list(
            frame,
            inner,
            &step_items,
            &self.step_state,
            self.focused_panel == FocusedPanel::Steps,
            self.tick,
        );
    }

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

        let lines = self.get_selected_output();

        if lines.is_empty() {
            let msg = if self.runs.is_empty() && !self.running {
                "  No runs yet. Press [r] to run."
            } else {
                "  No output for this step."
            };
            let p = Paragraph::new(msg).style(Style::default().fg(Color::DarkGray));
            frame.render_widget(p, inner);
        } else {
            let visible_height = inner.height as usize;
            let max_scroll = lines.len().saturating_sub(visible_height);
            let scroll = self.output_scroll.min(max_scroll);

            let visible_lines: Vec<Line> = lines
                .iter()
                .skip(scroll)
                .take(visible_height)
                .map(|l| Line::from(format!("  {}", l)))
                .collect();
            let p = Paragraph::new(visible_lines);
            frame.render_widget(p, inner);
        }
    }
}

fn format_run_timestamp(timestamp: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        dt.format("%-m/%d %-I:%M%P").to_string()
    } else if timestamp.len() >= 16 {
        timestamp[..16].to_string()
    } else {
        timestamp.to_string()
    }
}
```

**Step 2: Verify compilation**

Run: `cargo check`
Expected: May have compilation errors due to changed API (enter() signature, removed ShowHistory). Those will be fixed in Task 5.

**Step 3: Commit**

```bash
git add src/views/trail_view.rs
git commit -m "Rewrite trail view as unified 3-panel (Runs/Steps/Output)"
```

---

### Task 5: Remove TrailHistory & Wire App.rs

Remove the separate TrailHistory view and update all references in app.rs and types.rs.

**Files:**
- Delete: `src/views/trail_history.rs`
- Modify: `src/types.rs`
- Modify: `src/app.rs`
- Modify: `src/views/mod.rs` (if it exists)

**Step 1: Delete trail_history.rs**

```bash
rm src/views/trail_history.rs
```

**Step 2: Remove trail_history module declaration**

Find and remove `pub mod trail_history;` from wherever views are declared (likely `src/views/mod.rs` or `src/main.rs`).

**Step 3: Remove TrailHistory from AppView enum in types.rs**

Delete the `TrailHistory` variant (lines 384-390 in `src/types.rs`):

```rust
// DELETE THIS ENTIRE BLOCK:
TrailHistory {
    project: Project,
    livestock: Livestock,
    trail: crate::trails::Trail,
    source: String,
    source_barn: Option<Barn>,
},
```

**Step 4: Update app.rs — remove imports**

Remove the `TrailHistoryView` and `TrailHistoryAction` imports from line 33:

```rust
// DELETE:
use crate::views::trail_history::{TrailHistoryView, TrailHistoryAction};
```

**Step 5: Update app.rs — remove struct field**

Remove from the App struct (line 71):

```rust
// DELETE:
pub trail_history_view: TrailHistoryView,
```

Also remove the initialization in `App::new()` or wherever the struct is constructed.

**Step 6: Update app.rs — remove TrailHistory input handler**

Delete the entire `handle_trail_history_input` function (lines 1138-1157).

Remove the dispatch in the main event loop that calls it (search for `AppView::TrailHistory`).

**Step 7: Update app.rs — remove from go_back()**

Delete the `AppView::TrailHistory` arm in `go_back()` (lines 276-285).

**Step 8: Update app.rs — remove from navigate()**

Delete the `AppView::TrailHistory` arm in `navigate()` (lines 203-205).

**Step 9: Update app.rs — remove from render_view()**

Delete the `AppView::TrailHistory` rendering arm (lines 1609-1614).

**Step 10: Update app.rs — remove from get_bottom_bar_items()**

Delete the `AppView::TrailHistory` arm (lines 1745-1750).

**Step 11: Update app.rs — update trail_view.enter() call**

The `enter()` method signature changed: now takes `(trail, livestock)` instead of just `(trail)`.

Find all `app.trail_view.enter(` calls and update:

In `handle_trail_input` (around line 1105):
```rust
// Change from:
app.trail_view.enter(&trail);
// To:
app.trail_view.enter(&trail, &livestock);
```

In `handle_livestock_detail_input` `OpenTrail` handler (around line 1230):
```rust
// Change from:
app.trail_view.enter(trail);
// To:
app.trail_view.enter(trail, &livestock);
```

**Step 12: Update app.rs — remove ShowHistory handling**

In `handle_trail_input`, remove the `TrailViewAction::ShowHistory` arm (lines 1124-1133).

**Step 13: Update app.rs — update run completion to use new API**

In the trail execution polling section (lines 604-673), update the run completion logic:

Replace the block that sets `app.trail_view.running = false` (around line 624) to also call `finish_run`:

```rust
if all_done {
    if let AppView::Trail { ref livestock, ref trail, .. } = app.view {
        // Save final run state (existing code)
        // ...
        // Then refresh runs list
        app.trail_view.finish_run(&livestock.name, &trail.name);
    }
    trail_finished = true;
}
```

**Step 14: Update app.rs — update RunTrail handler**

In `handle_trail_input`, update the `RunTrail` handler to use `start_run` instead of `enter`:

```rust
TrailViewAction::RunTrail => {
    let barn = source_barn.as_ref().or_else(|| {
        livestock.barn.as_ref().and_then(|bn| app.barns.iter().find(|b| b.name == *bn))
    }).cloned();

    if let Some(barn) = barn {
        match crate::trails::runner::start_trail(&trail, &livestock, &barn) {
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
    } else {
        app.error = Some("No barn found for this livestock".to_string());
    }
}
```

**Step 15: Update Trail bottom bar hints**

```rust
AppView::Trail { .. } => vec![
    ("r", "run"),
    ("x", "cancel"),
    ("Tab", "switch panel"),
    ("Esc", "back"),
],
```

**Step 16: Verify compilation**

Run: `cargo check`
Fix all remaining compilation errors (there may be a few — follow the compiler).

**Step 17: Commit**

```bash
git add -A
git commit -m "Remove TrailHistory view, wire unified trail view into app"
```

---

### Task 6: Runner Env Var Injection

Add environment variable injection to the native provider alongside backward-compatible template resolution.

**Files:**
- Modify: `src/trails/runner.rs`
- Modify: `src/trails/native.rs`
- Modify: `src/trails/provider.rs`

**Step 1: Add env vars to TrailContext**

In `src/trails/provider.rs`, add an `env_vars` field to `TrailContext`:

```rust
pub struct TrailContext {
    pub livestock: Livestock,
    pub barn: Barn,
    pub trail: Trail,
    pub resolved_steps: Vec<TrailStep>,
    pub run_dir: std::path::PathBuf,
    pub env_vars: Vec<(String, String)>,
}
```

**Step 2: Build env vars in runner.rs**

In `src/trails/runner.rs`, in `start_trail()` (line 50), build env vars before creating the context:

```rust
// Build environment variables for injection
let env_vars = vec![
    ("NAME".to_string(), livestock.name.clone()),
    ("REPO_PATH".to_string(), livestock.path.clone()),
    ("BRANCH".to_string(), livestock.branch.as_deref().unwrap_or("main").to_string()),
    ("BARN".to_string(), barn.host.as_deref().unwrap_or("localhost").to_string()),
    ("BARN_USER".to_string(), barn.user.as_deref().unwrap_or("root").to_string()),
];
```

Pass it into `TrailContext`:
```rust
let ctx = TrailContext {
    livestock: livestock.clone(),
    barn: barn.clone(),
    trail: trail.clone(),
    resolved_steps,
    run_dir: run_dir.clone(),
    env_vars,
};
```

**Step 3: Inject env vars in native.rs**

In `src/trails/native.rs`, in the `execute` method, extract env_vars from context and set them on the SSH command. After line 37 (`let run_dir = ctx.run_dir;`), add:

```rust
let env_vars = ctx.env_vars;
```

Before the `cmd.arg(&step.run)` line (line 72), wrap the command to export env vars:

```rust
// Build command with env var exports prepended
let env_exports: String = env_vars.iter()
    .map(|(k, v)| format!("export {}='{}'", k, v.replace('\'', "'\\''")))
    .collect::<Vec<_>>()
    .join("; ");
let full_command = if env_exports.is_empty() {
    step.run.clone()
} else {
    format!("{}; {}", env_exports, step.run)
};
cmd.arg(&full_command);
```

Replace the existing `cmd.arg(&step.run);` with the above.

**Step 4: Verify compilation**

Run: `cargo check`

**Step 5: Commit**

```bash
git add src/trails/runner.rs src/trails/native.rs src/trails/provider.rs
git commit -m "Add env var injection to trail runner ($NAME, $REPO_PATH, $BRANCH, etc.)"
```

---

### Task 7: Final Polish & Build Verification

Ensure everything compiles, the binary builds, and run a quick smoke test.

**Files:**
- Various (fixups only)

**Step 1: Full build**

Run: `cargo build`
Expected: Clean build with no errors.

**Step 2: Fix any warnings**

Run: `cargo build 2>&1 | grep warning`
Fix any unused import or dead code warnings.

**Step 3: Run the binary**

Run: `cargo run`
Verify:
- Navigate to a project → livestock detail
- See both Sessions and Trails panels
- Tab switches between them
- Press [n] in Trails panel to open creation wizard
- Create a test trail (name it "test", add one step)
- See the trail appear in the Trails panel
- Press Enter on the trail to open Trail Detail (3-panel view)
- Press Esc to go back

**Step 4: Final commit**

```bash
git add -A
git commit -m "Trails v2: final polish and build verification"
```
