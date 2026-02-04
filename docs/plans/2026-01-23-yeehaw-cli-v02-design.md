# Yeehaw CLI v0.2 Design Document

**Date:** 2026-01-23
**Status:** Draft
**Author:** Claude + Cam

## Overview

This document describes UX and architectural improvements for Yeehaw CLI v0.2, building on the v0.1 foundation. The key changes are:

1. Two-tier navigation (Global Dashboard vs Project Context)
2. tmux window-based session management with branded UI
3. Refined data model separating Barns from Deployments

---

## 1. Two-Tier Navigation Model

### Global Dashboard (YEEHAW)

When the app launches, the user sees the YEEHAW ASCII header. This is the command center showing:

- **All projects** with health indicators (active sessions, last activity)
- **All barns** (servers) with connection status
- **All running sessions** across all projects
- **Activity feed** showing recent events

```
┌─────────────────────────────────────────────────────────────────────┐
│  ██╗   ██╗███████╗███████╗██╗  ██╗ █████╗ ██╗    ██╗               │
│  ╚██╗ ██╔╝██╔════╝██╔════╝██║  ██║██╔══██╗██║    ██║               │
│   ╚████╔╝ █████╗  █████╗  ███████║███████║██║ █╗ ██║               │
│    ╚██╔╝  ██╔══╝  ██╔══╝  ██╔══██║██╔══██║██║███╗██║               │
│     ██║   ███████╗███████╗██║  ██║██║  ██║╚███╔███╔╝               │
│     ╚═╝   ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝                │
├─────────────────────────────────────────────────────────────────────┤
│  ┌─ Projects ─────────────┐  ┌─ Sessions ─────────────────────────┐│
│  │ › acme-webapp    ● 2   │  │   [1] claude: acme / fixing auth   ││
│  │   personal-blog  ○ 0   │  │ › [2] claude: blog / new post      ││
│  │   api-service    ● 1   │  │   [3] shell: acme / logs           ││
│  └────────────────────────┘  └────────────────────────────────────┘│
│  ┌─ Barns ────────────────────────────────────────────────────────┐│
│  │   linode-prod ● online    staging-01 ● online    local ○      ││
│  └────────────────────────────────────────────────────────────────┘│
├─────────────────────────────────────────────────────────────────────┤
│ [p]rojects  [b]arns  [s]essions  [?]help                    q:quit │
└─────────────────────────────────────────────────────────────────────┘
```

**Actions from Global Dashboard:**
- Select a project → enters Project Context
- Quick-attach to any running session
- View barn health across all servers

### Project Context (PROJECT NAME)

When a project is selected, the ASCII header changes to the project name. The view narrows to project-specific concerns:

```
┌─────────────────────────────────────────────────────────────────────┐
│   █████╗  ██████╗███╗   ███╗███████╗                               │
│  ██╔══██╗██╔════╝████╗ ████║██╔════╝                               │
│  ███████║██║     ██╔████╔██║█████╗                                 │
│  ██╔══██║██║     ██║╚██╔╝██║██╔══╝                                 │
│  ██║  ██║╚██████╗██║ ╚═╝ ██║███████╗                               │
│  ╚═╝  ╚═╝ ╚═════╝╚═╝     ╚═╝╚══════╝  ~/Sites/acme-webapp         │
├─────────────────────────────────────────────────────────────────────┤
│  ┌─ Deployments ──────────────────┐  ┌─ Sessions ─────────────────┐│
│  │ › production (linode-prod)     │  │ › [1] claude: fixing auth  ││
│  │     /var/www/acme ● online     │  │   [3] shell: logs          ││
│  │   staging (staging-01)         │  │                            ││
│  │     /var/www/acme-stg ● online │  │   [c] new claude session   ││
│  └────────────────────────────────┘  └────────────────────────────┘│
│                                                                     │
├─────────────────────────────────────────────────────────────────────┤
│ [c]laude  [d]eploy  [Enter]ssh  [?]help            Esc:back q:quit │
└─────────────────────────────────────────────────────────────────────┘
```

**Actions from Project Context:**
- Start new Claude session (scoped to this project)
- SSH into a deployment (barn + project path)
- Attach to project's running sessions
- `Esc` or `q` returns to Global Dashboard

### Navigation Flow

```
YEEHAW (global dashboard)
    │
    ├─→ Select project ──→ PROJECT NAME (scoped context)
    │                           │
    │                           ├─→ [c] Start Claude session
    │                           ├─→ [Enter] SSH to deployment
    │                           ├─→ [Enter] Attach to session
    │                           └─→ [Esc/q] Back to YEEHAW
    │
    └─→ Quick-attach to any session (stays in global)
```

---

## 2. tmux Window-Based Session Management

### Architecture

Yeehaw runs inside tmux as window 0 of a master session. Claude and shell sessions are sibling windows:

```
tmux session: "yeehaw"
├── window 0: Yeehaw TUI (dashboard/project views)
├── window 1: Claude session (acme / fixing auth)
├── window 2: Claude session (blog / new post)
└── window 3: Shell (acme / logs)
```

### User Flow

1. User runs `yeehaw` command
2. Yeehaw creates or attaches to tmux session named `yeehaw`
3. Yeehaw TUI renders in window 0
4. User presses `c` → new Claude window created, focus switches to it
5. User presses `Ctrl-y` → returns to Yeehaw window instantly
6. User presses `Ctrl-b 2` → jumps to window 2 directly

### Branded tmux Experience

**Custom Keybinding:**
- `Ctrl-y` → select-window -t :0 (return to Yeehaw)

**When in Yeehaw Window (0):**
- tmux status bar is hidden (Yeehaw renders its own UI)
- Or minimal: `[3 sessions running]`

**When in Claude/Shell Window (1+):**
- Branded status bar in Yeehaw colors (yellow/brown, not default green)
- Shows context and escape hatch:

```
┌────────────────────────────────────────────────────────────────────┐
│                                                                    │
│  Claude Code session content...                                    │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│ YEEHAW │ acme-webapp │ fixing auth │       Ctrl-y: back to yeehaw │
└────────────────────────────────────────────────────────────────────┘
```

### tmux Configuration

Yeehaw manages its own tmux config (sourced on session creation):

```bash
# ~/.yeehaw/tmux.conf

# Yeehaw keybinding
bind-key -n C-y select-window -t :0

# Status bar styling (shown only in non-Yeehaw windows)
set -g status-style "bg=#b8860b,fg=#000000"
set -g status-left "#[bold] YEEHAW │ "
set -g status-right " Ctrl-y: dashboard "

# Hide status in window 0 (Yeehaw handles its own UI)
set-hook -g window-pane-changed 'if-shell "[ #{window_index} -eq 0 ]" "set status off" "set status on"'
```

---

## 3. Data Model: Barns vs Deployments

### Barn (Global Server Definition)

A barn is a server and how to connect to it. Barns are global resources, independent of any project.

```yaml
# ~/.yeehaw/barns/linode-prod.yaml
name: linode-prod
host: 45.33.12.94
user: deploy
port: 22
identity_file: ~/.ssh/deploy_key

# Optional metadata
provider: linode
region: us-east
```

```yaml
# ~/.yeehaw/barns/staging-01.yaml
name: staging-01
host: staging.example.com
user: deploy
port: 22
identity_file: ~/.ssh/deploy_key
```

### Project (With Deployments)

A project references barns via deployments, which include project-specific details like the remote path.

```yaml
# ~/.yeehaw/projects/acme-webapp.yaml
name: acme-webapp
path: ~/Sites/acme-webapp  # local working directory

deployments:
  - barn: linode-prod
    name: production        # display name
    path: /var/www/acme     # path ON the server
    branch: main

  - barn: staging-01
    name: staging
    path: /var/www/acme-staging
    branch: develop
```

### Relationships

```
┌─────────────┐         ┌─────────────┐
│   Barn      │         │   Project   │
│ (server)    │◄────────│             │
├─────────────┤    *    ├─────────────┤
│ host        │         │ name        │
│ user        │         │ path        │
│ port        │         │ deployments │
│ identity    │         └─────────────┘
└─────────────┘               │
                              │ has many
                              ▼
                    ┌─────────────────┐
                    │   Deployment    │
                    ├─────────────────┤
                    │ barn (ref)      │
                    │ name            │
                    │ path (remote)   │
                    │ branch          │
                    └─────────────────┘
```

### UI Mapping

| Context | Panel | Shows |
|---------|-------|-------|
| Global Dashboard | Barns | All servers with health status |
| Global Dashboard | Projects | All projects with session counts |
| Project Context | Deployments | This project's barn+path combos |
| Project Context | Sessions | Only this project's sessions |

---

## 4. Session Metadata

Sessions track which project and context they belong to:

```yaml
# ~/.yeehaw/sessions/yh-claude-1706012345.yaml
id: yh-claude-1706012345
type: claude
project: acme-webapp          # scoped to project
deployment: production        # optional: if started from deployment
tmux_window: 1                # window number in yeehaw session
started_at: 2026-01-23T10:30:00Z
working_directory: /Users/cam/Sites/acme-webapp
notes: "fixing auth"
status: active
```

---

## 5. Future: Daemon/Background Mode

The tmux architecture supports future headless operation:

```bash
yeehaw              # interactive TUI (default)
yeehaw --daemon     # start headless, manage sessions in background
yeehaw --attach     # attach to existing yeehaw tmux session
yeehaw --status     # print status without TUI
```

**Daemon Capabilities (Future):**
- Scheduled Claude sessions
- Health monitoring of barns
- Automated deployments
- Webhook triggers

**For Now:** We design with this in mind but don't implement daemon features in v0.2.

---

## 6. Implementation Phases

### Phase 1: Data Model Updates
- Update `types.ts` with Deployment type
- Update project YAML schema
- Update `useConfig` hook to load deployments

### Phase 2: tmux Integration
- Yeehaw runs inside tmux session
- Create/manage windows for Claude sessions
- Generate and source custom tmux.conf
- Implement `Ctrl-y` keybinding

### Phase 3: Two-Tier UI
- Refactor Header to accept dynamic ASCII text
- Create GlobalDashboard view
- Rename/refactor Home to ProjectContext view
- Update navigation flow (Esc returns to global)

### Phase 4: Branded tmux
- Custom status bar styling
- Context-aware status bar content
- Hide status bar in Yeehaw window

---

## 7. Success Criteria

v0.2 is successful if:

1. User launches `yeehaw` and sees global dashboard with all projects/barns/sessions
2. User selects project and sees scoped view with project name as ASCII header
3. User starts Claude session, it opens in new tmux window
4. User presses `Ctrl-y` from Claude session and returns to Yeehaw instantly
5. tmux status bar is branded with Yeehaw colors, not default green
6. `Esc` from project context returns to global dashboard
7. Deployments show barn + remote path correctly
