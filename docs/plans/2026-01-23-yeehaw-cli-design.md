# Yeehaw CLI Design Document

**Date:** 2026-01-23
**Status:** Draft
**Author:** Claude + Cam

## Overview

Yeehaw CLI is a full-screen terminal user interface (TUI) for managing development infrastructure using the "Infrastructure as Farm" metaphor. It provides keyboard-driven navigation for managing projects, barns (servers), and Claude Code sessions via tmux.

This is a standalone CLI application, separate from the existing desktop Electron app, designed for developers who prefer working entirely within the terminal.

## Goals

1. **Terminal-native experience** - Full-screen TUI like Vim, lazygit, or Midnight Commander
2. **Keyboard-driven** - All operations accessible via keyboard shortcuts
3. **Simple foundation** - Config files over databases, minimal dependencies
4. **tmux integration** - Manage Claude Code sessions as tmux sessions
5. **Familiar patterns** - Vim-style navigation, Unix conventions

## Non-Goals (v0.1)

- Sharing data with desktop app
- GitHub issue integration
- Terraform provisioning
- Critter/livestock installation
- Session recording (asciinema)
- Mouse support

---

## User Interface

### Screen Layout

```
┌─────────────────────────────────────────────────────────────────┐
│  ██╗   ██╗███████╗███████╗██╗  ██╗ █████╗ ██╗    ██╗           │
│  ╚██╗ ██╔╝██╔════╝██╔════╝██║  ██║██╔══██╗██║    ██║           │
│   ╚████╔╝ █████╗  █████╗  ███████║███████║██║ █╗ ██║           │
│    ╚██╔╝  ██╔══╝  ██╔══╝  ██╔══██║██╔══██║██║███╗██║           │
│     ██║   ███████╗███████╗██║  ██║██║  ██║╚███╔███╔╝           │
│     ╚═╝   ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝ ╚══╝╚══╝            │
│                                                                 │
│  Project: acme-webapp                              [P]rojects   │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─ Barns ──────────────────┐  ┌─ Sessions ──────────────────┐ │
│  │ › production  ● active   │  │   #1 claude: fixing auth    │ │
│  │   staging     ● active   │  │   #2 shell: logs            │ │
│  │   dev-local   ○ offline  │  │ › #3 claude: new feature    │ │
│  └──────────────────────────┘  └──────────────────────────────┘ │
│                                                                 │
│  ┌─ Recent Activity ────────────────────────────────────────┐  │
│  │ 2m ago   Session #3 started on production                │  │
│  │ 15m ago  Deployed v2.1.0 to staging                      │  │
│  │ 1h ago   Issue #42 closed                                │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ [b]arns  [s]essions  [i]ssues  [c]laude  [?]help      q:quit   │
└─────────────────────────────────────────────────────────────────┘
```

### Layout Zones

| Zone | Purpose |
|------|---------|
| **Header** | ASCII art "YEEHAW" logo + current project name |
| **Main content** | View-specific panels (barns, sessions, activity) |
| **Status bar** | Available keyboard shortcuts for current context |

### Views

1. **Home** - Dashboard with barns, sessions, recent activity
2. **Barns** - Full list of barns with details
3. **Sessions** - Active tmux sessions
4. **Projects** - Project switcher

---

## Keyboard Navigation

### Global Shortcuts

| Key | Action |
|-----|--------|
| `q` | Quit / Back to previous view |
| `?` | Toggle help overlay |
| `p` | Open project switcher |
| `b` | Go to barns view |
| `s` | Go to sessions view |
| `c` | Start new Claude session |
| `:` | Command mode (future) |
| `Esc` | Return to home / Cancel |

### List Navigation

| Key | Action |
|-----|--------|
| `j` / `↓` | Move selection down |
| `k` / `↑` | Move selection up |
| `g` | Jump to first item |
| `G` | Jump to last item |
| `Enter` | Select / Activate item |
| `Tab` | Move to next panel |
| `Shift+Tab` | Move to previous panel |

### Context-Specific

| Key | Context | Action |
|-----|---------|--------|
| `Enter` | Barn selected | SSH into barn |
| `Enter` | Session selected | Attach to tmux session |
| `n` | Barns view | Add new barn |
| `d` | Any item selected | Delete (with confirmation) |
| `e` | Any item selected | Edit in $EDITOR |

---

## Configuration

### Directory Structure

```
~/.yeehaw/
├── config.yaml                 # Global settings
├── projects/                   # Project definitions
│   ├── acme-webapp.yaml
│   └── personal-blog.yaml
├── barns/                      # Barn (server) definitions
│   ├── production.yaml
│   ├── staging.yaml
│   └── dev-local.yaml
└── sessions/                   # Session metadata (auto-generated)
    └── 2026-01-23-abc123.yaml
```

Optional project-local config:

```
<project-root>/.yeehaw/
├── project.yaml                # Project-specific settings
└── barns/                      # Project-scoped barns
```

### Config File Formats

#### `~/.yeehaw/config.yaml`

```yaml
version: 1

# Default project to load on startup
default_project: acme-webapp

# Preferred editor for config editing
editor: nvim

# UI settings
theme: dark
show_activity: true

# Claude Code settings
claude:
  model: claude-sonnet-4-20250514
  auto_attach: true         # Auto-attach when starting new session

# tmux settings
tmux:
  session_prefix: "yh-"     # Prefix for yeehaw-managed sessions
  default_shell: /bin/zsh
```

#### `~/.yeehaw/projects/<name>.yaml`

```yaml
name: acme-webapp
path: /Users/cam/Sites/acme-webapp

# Associated repositories
repositories:
  - url: git@github.com:acme/webapp.git
    path: .
  - url: git@github.com:acme/api.git
    path: ./api

# Barns associated with this project
barns:
  - production
  - staging

# GitHub integration (future)
github:
  repo: acme/webapp
  sync_issues: false
```

#### `~/.yeehaw/barns/<name>.yaml`

```yaml
name: production
host: 45.33.12.94
user: deploy
port: 22
identity_file: ~/.ssh/acme_deploy

# Optional: installed services (informational)
critters:
  - nginx
  - php-fpm
  - mysql

# Optional: deployed applications (informational)
livestock:
  - name: webapp
    type: laravel
    path: /var/www/webapp
```

#### `~/.yeehaw/sessions/<id>.yaml`

Auto-generated when sessions are created:

```yaml
id: yh-claude-1706012345
type: claude              # claude | shell
project: acme-webapp
barn: null                # or barn name if SSH-based
tmux_session: yh-claude-1706012345
started_at: 2026-01-23T10:30:00Z
working_directory: /Users/cam/Sites/acme-webapp
notes: "Working on auth refactor"
status: active            # active | detached | ended
```

---

## tmux Integration

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Yeehaw CLI (Ink TUI)                       │
│  - Renders UI, handles keyboard input                           │
│  - Manages config files                                         │
│  - Spawns/queries tmux sessions                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ spawn / attach / list
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        tmux server                              │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐            │
│  │ yh-claude-1  │ │ yh-claude-2  │ │ yh-shell-1   │            │
│  │ (claude cmd) │ │ (claude cmd) │ │ (bash)       │            │
│  └──────────────┘ └──────────────┘ └──────────────┘            │
└─────────────────────────────────────────────────────────────────┘
```

### Session Lifecycle

**Creating a Claude session:**

1. User presses `c` in Yeehaw
2. Yeehaw creates session metadata file
3. Yeehaw spawns tmux session:
   ```bash
   tmux new-session -d -s "yh-claude-$(timestamp)" \
     -c "/path/to/project" \
     "claude"
   ```
4. Yeehaw immediately attaches (if `auto_attach: true`)

**Listing sessions:**

```bash
tmux list-sessions -F "#{session_name}:#{session_created}:#{session_activity}:#{session_attached}"
```

Filter to `yh-*` prefix to show only Yeehaw-managed sessions.

**Attaching to a session:**

1. User selects session, presses `Enter`
2. Yeehaw "suspends" its Ink rendering
3. Yeehaw runs: `tmux attach-session -t "session-name"`
4. User works in tmux (full terminal control)
5. User detaches (`Ctrl-b d`)
6. Control returns to Yeehaw, Ink resumes rendering

**Ending a session:**

Sessions end naturally when the command exits. Yeehaw updates the session metadata file to `status: ended`.

---

## Technology Stack

| Component | Technology | Purpose |
|-----------|------------|---------|
| Runtime | Node.js 20+ | JavaScript runtime |
| Language | TypeScript 5.x | Type safety |
| TUI Framework | Ink 4.x | React components for terminal |
| React | React 18.x | Component model |
| ASCII Art | figlet | Big text headers |
| Colors | chalk 5.x | Terminal styling |
| Config | js-yaml | YAML parsing |
| File watching | chokidar | Config hot-reload |
| Child processes | execa | Spawning tmux/ssh |

### Dependencies

```json
{
  "dependencies": {
    "ink": "^4.4.1",
    "react": "^18.2.0",
    "figlet": "^1.7.0",
    "chalk": "^5.3.0",
    "js-yaml": "^4.1.0",
    "chokidar": "^3.5.3",
    "execa": "^8.0.1"
  },
  "devDependencies": {
    "typescript": "^5.3.0",
    "@types/react": "^18.2.0",
    "@types/figlet": "^1.5.8",
    "@types/js-yaml": "^4.0.9",
    "tsx": "^4.7.0"
  }
}
```

---

## Project Structure

```
yeehaw-cli/
├── src/
│   ├── index.tsx              # Entry point, CLI argument handling
│   ├── app.tsx                # Main App component
│   │
│   ├── components/
│   │   ├── Header.tsx         # ASCII art header + project context
│   │   ├── StatusBar.tsx      # Bottom keyboard hints
│   │   ├── Panel.tsx          # Bordered panel container
│   │   ├── List.tsx           # Navigable list with selection
│   │   ├── HelpOverlay.tsx    # Help modal (? key)
│   │   └── Confirm.tsx        # Confirmation dialog
│   │
│   ├── views/
│   │   ├── Home.tsx           # Dashboard view
│   │   ├── Barns.tsx          # Barn list/detail
│   │   ├── Sessions.tsx       # Session list
│   │   └── Projects.tsx       # Project switcher
│   │
│   ├── hooks/
│   │   ├── useConfig.ts       # Load and watch config files
│   │   ├── useProjects.ts     # Project CRUD
│   │   ├── useBarns.ts        # Barn CRUD
│   │   ├── useSessions.ts     # tmux session queries
│   │   └── useKeymap.ts       # Keyboard handling
│   │
│   ├── lib/
│   │   ├── config.ts          # Config file reading/writing
│   │   ├── tmux.ts            # tmux command wrappers
│   │   ├── ssh.ts             # SSH connection handling
│   │   └── paths.ts           # Config path resolution
│   │
│   └── types.ts               # TypeScript interfaces
│
├── package.json
├── tsconfig.json
├── .gitignore
└── README.md
```

---

## MVP Feature Set (v0.1)

### Must Have

| Feature | Description |
|---------|-------------|
| Full-screen TUI | Ink-based, takes over terminal |
| ASCII header | figlet "YEEHAW" with project name |
| Project switching | `p` key opens project list |
| Barn list | Show all barns with connection status |
| SSH into barn | `Enter` on barn starts SSH |
| Session list | Show tmux sessions (yh-* prefix) |
| Attach session | `Enter` on session attaches tmux |
| New Claude session | `c` key spawns claude in tmux |
| Vim navigation | `j/k/g/G` for list navigation |
| Help overlay | `?` shows keyboard shortcuts |
| Config reading | Load from `~/.yeehaw/` |
| Status bar | Show available commands |

### Deferred to v0.2+

- GitHub issue integration
- Critter/livestock management
- Barn provisioning (Terraform)
- Session recording
- Search/filter (`/` key)
- Themes/customization
- Mouse support
- Config editing within TUI
- Activity log persistence

---

## Error Handling

### Missing Config

On first run, if `~/.yeehaw/` doesn't exist:
1. Create directory structure
2. Create minimal `config.yaml` with defaults
3. Show welcome message with setup instructions

### tmux Not Installed

If tmux is not available:
1. Show error message
2. Provide installation instructions
3. Allow viewing (but not creating) sessions

### SSH Connection Failure

If barn connection fails:
1. Show error in status area
2. Offer to edit barn config (`e` key)
3. Don't crash the TUI

---

## Future Considerations

### v0.2 Possibilities

- **Issue tracking:** Sync GitHub issues, show in sidebar
- **Barn health:** Ping barns, show actual status
- **Session search:** Filter sessions by project/type
- **Quick actions:** Command palette (`:` key)

### v0.3+ Possibilities

- **Critter management:** Install services on barns
- **Livestock deployment:** Deploy applications
- **Terraform integration:** Provision new barns
- **MCP integration:** Expose Yeehaw tools to Claude sessions

---

## Open Questions

1. **Session persistence:** Should session metadata survive tmux session ending? (Current answer: yes, for history)

2. **Multi-project barns:** A barn can serve multiple projects. How to handle in UI? (Current answer: barns are global, projects reference them)

3. **Remote Yeehaw:** Should Yeehaw work over SSH? (Current answer: yes, it's just a Node CLI)

---

## Success Criteria

v0.1 is successful if:

1. User can launch `yeehaw` and see full-screen TUI
2. User can switch between projects
3. User can see list of barns and SSH into one
4. User can start a new Claude session (spawns tmux)
5. User can attach to existing tmux session
6. User can navigate entirely with keyboard
7. Config changes in `~/.yeehaw/` are reflected in TUI
