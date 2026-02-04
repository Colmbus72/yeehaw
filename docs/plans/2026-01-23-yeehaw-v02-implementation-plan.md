# Yeehaw CLI v0.2 Implementation Plan

**Date:** 2026-01-23
**Design Doc:** `docs/plans/2026-01-23-yeehaw-cli-v02-design.md`

---

## Phase 1: Data Model Updates

### Task 1.1: Add Deployment Type to types.ts

**File:** `/Users/cam/Sites/Yeehaw/cli/src/types.ts`

Add new `Deployment` interface:

```typescript
export interface Deployment {
  barn: string;          // reference to barn name
  name: string;          // display name (e.g., "production", "staging")
  path: string;          // path on the remote server
  branch?: string;       // optional git branch
}
```

**Verification:** TypeScript compiles without errors.

---

### Task 1.2: Update Project Type to Include Deployments

**File:** `/Users/cam/Sites/Yeehaw/cli/src/types.ts`

Modify `Project` interface:

```typescript
export interface Project {
  name: string;
  path: string;
  repositories: Repository[];
  barns: string[];           // DEPRECATED: keep for backwards compat, remove in v0.3
  deployments: Deployment[]; // NEW: replaces barns array
  github?: GitHubConfig;
}
```

**Verification:** TypeScript compiles without errors.

---

### Task 1.3: Update Session Type with tmux_window

**File:** `/Users/cam/Sites/Yeehaw/cli/src/types.ts`

Modify `Session` interface:

```typescript
export interface Session {
  id: string;
  type: 'claude' | 'shell';
  project: string | null;
  deployment: string | null;   // NEW: optional deployment name
  barn: string | null;
  tmux_session: string;
  tmux_window: number | null;  // NEW: window index in yeehaw session
  started_at: string;
  working_directory: string;
  notes: string;
  status: 'active' | 'detached' | 'ended';
}
```

**Verification:** TypeScript compiles without errors.

---

### Task 1.4: Update useConfig Hook to Load Deployments

**File:** `/Users/cam/Sites/Yeehaw/cli/src/hooks/useConfig.ts`

Ensure `loadProjects()` properly loads the `deployments` array from YAML. Add backwards compatibility to convert old `barns` array to `deployments` if needed:

```typescript
// In loadProjects or after loading:
function normalizeProject(project: Project): Project {
  if (!project.deployments && project.barns?.length) {
    // Backwards compat: convert barns array to deployments
    project.deployments = project.barns.map(barnName => ({
      barn: barnName,
      name: barnName,
      path: project.path, // default to project path
    }));
  }
  project.deployments = project.deployments || [];
  return project;
}
```

**Verification:**
1. Create test project YAML with deployments array
2. Run app, confirm deployments load correctly
3. Test backwards compat with old barns-only YAML

---

## Phase 2: tmux Integration Overhaul

### Task 2.1: Create Yeehaw tmux.conf Template

**New File:** `/Users/cam/Sites/Yeehaw/cli/src/lib/tmux-config.ts`

```typescript
export function generateTmuxConfig(projectName?: string): string {
  return `
# Yeehaw tmux configuration
# Auto-generated - do not edit manually

# Yeehaw keybinding: Ctrl-y returns to dashboard (window 0)
bind-key -n C-y select-window -t :0

# Status bar styling (Yeehaw brand colors)
set -g status-style "bg=#b8860b,fg=#1a1a1a"
set -g status-left "#[bold] YEEHAW "
set -g status-left-length 20
set -g status-right " Ctrl-y: dashboard "
set -g status-right-length 30

# Window status format
set -g window-status-format " #I:#W "
set -g window-status-current-format "#[bg=#daa520,fg=#1a1a1a,bold] #I:#W "

# Pane border styling
set -g pane-border-style "fg=#b8860b"
set -g pane-active-border-style "fg=#daa520"

# Message styling
set -g message-style "bg=#b8860b,fg=#1a1a1a"
`.trim();
}

export function writeTmuxConfig(): string {
  const configPath = join(homedir(), '.yeehaw', 'tmux.conf');
  const content = generateTmuxConfig();
  writeFileSync(configPath, content);
  return configPath;
}
```

**Verification:** Run `writeTmuxConfig()`, check file exists at `~/.yeehaw/tmux.conf`.

---

### Task 2.2: Refactor tmux.ts for Window-Based Sessions

**File:** `/Users/cam/Sites/Yeehaw/cli/src/lib/tmux.ts`

Add new functions for window management:

```typescript
const YEEHAW_SESSION = 'yeehaw';

export function ensureYeehawSession(): boolean {
  // Check if yeehaw session exists
  const result = execSync(`tmux has-session -t ${YEEHAW_SESSION} 2>/dev/null`, {
    encoding: 'utf-8',
    stdio: ['pipe', 'pipe', 'pipe']
  });
  return result.exitCode === 0;
}

export function createYeehawSession(): void {
  const configPath = writeTmuxConfig();
  execSync(`tmux new-session -d -s ${YEEHAW_SESSION} -n yeehaw`);
  execSync(`tmux source-file ${configPath}`);
}

export function createClaudeWindow(workingDir: string, windowName: string): number {
  // Create new window in yeehaw session, run claude
  const cmd = `tmux new-window -t ${YEEHAW_SESSION} -n "${windowName}" -c "${workingDir}" "claude"`;
  execSync(cmd);
  // Get the window index
  const index = execSync(`tmux display-message -t ${YEEHAW_SESSION} -p '#{window_index}'`);
  return parseInt(index.toString().trim(), 10);
}

export function createShellWindow(workingDir: string, windowName: string, shell: string = '/bin/zsh'): number {
  const cmd = `tmux new-window -t ${YEEHAW_SESSION} -n "${windowName}" -c "${workingDir}" "${shell}"`;
  execSync(cmd);
  const index = execSync(`tmux display-message -t ${YEEHAW_SESSION} -p '#{window_index}'`);
  return parseInt(index.toString().trim(), 10);
}

export function switchToWindow(windowIndex: number): void {
  execSync(`tmux select-window -t ${YEEHAW_SESSION}:${windowIndex}`);
}

export function listYeehawWindows(): TmuxWindow[] {
  const output = execSync(
    `tmux list-windows -t ${YEEHAW_SESSION} -F '#{window_index}:#{window_name}:#{window_active}'`,
    { encoding: 'utf-8' }
  );
  return output.trim().split('\n').filter(Boolean).map(line => {
    const [index, name, active] = line.split(':');
    return { index: parseInt(index, 10), name, active: active === '1' };
  });
}

export interface TmuxWindow {
  index: number;
  name: string;
  active: boolean;
}
```

**Verification:**
1. Manually test `createYeehawSession()` creates session
2. Test `createClaudeWindow()` adds window with claude
3. Test `switchToWindow()` changes focus
4. Test `listYeehawWindows()` returns correct list

---

### Task 2.3: Update Entry Point to Run Inside tmux

**File:** `/Users/cam/Sites/Yeehaw/cli/src/index.tsx`

Modify entry point to either:
- Create yeehaw tmux session and attach to window 0, OR
- If already inside yeehaw session, just run the TUI

```typescript
#!/usr/bin/env node
import { render } from 'ink';
import React from 'react';
import { App } from './app.js';
import { ensureYeehawSession, createYeehawSession, isInsideYeehawSession } from './lib/tmux.js';

function main() {
  // Check if we're already in the yeehaw tmux session
  if (isInsideYeehawSession()) {
    // Just render the TUI
    render(<App />);
    return;
  }

  // Not in yeehaw session - need to create/attach
  if (!ensureYeehawSession()) {
    createYeehawSession();
  }

  // Attach to yeehaw session, window 0
  // This replaces the current process
  attachToYeehawWindow(0);
}

main();
```

Add helper to tmux.ts:

```typescript
export function isInsideYeehawSession(): boolean {
  return process.env.TMUX_PANE !== undefined &&
         process.env.TMUX?.includes('yeehaw');
}

export function attachToYeehawWindow(windowIndex: number): never {
  const args = ['attach-session', '-t', `${YEEHAW_SESSION}:${windowIndex}`];
  execSync(`tmux ${args.join(' ')}`, { stdio: 'inherit' });
  process.exit(0);
}
```

**Verification:**
1. Run `yeehaw` from outside tmux - should create session and attach
2. Run `yeehaw` from inside yeehaw session - should just show TUI
3. Ctrl-y from any window should return to window 0

---

### Task 2.4: Update useSessions Hook for Windows

**File:** `/Users/cam/Sites/Yeehaw/cli/src/hooks/useSessions.ts`

Refactor to work with windows instead of separate sessions:

```typescript
export function useSessions() {
  const [windows, setWindows] = useState<TmuxWindow[]>([]);

  // Poll windows every 5 seconds
  useEffect(() => {
    const refresh = () => setWindows(listYeehawWindows());
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, []);

  const createClaude = useCallback((workingDir: string, name: string) => {
    const windowIndex = createClaudeWindow(workingDir, name);
    switchToWindow(windowIndex);
    // Refresh will happen on next poll
  }, []);

  const createShell = useCallback((workingDir: string, name: string) => {
    const windowIndex = createShellWindow(workingDir, name);
    switchToWindow(windowIndex);
  }, []);

  const attachToWindow = useCallback((index: number) => {
    switchToWindow(index);
  }, []);

  return { windows, createClaude, createShell, attachToWindow };
}
```

**Verification:** Sessions panel shows tmux windows correctly.

---

## Phase 3: Two-Tier UI

### Task 3.1: Create Dynamic ASCII Header Component

**File:** `/Users/cam/Sites/Yeehaw/cli/src/components/Header.tsx`

Refactor to accept dynamic text:

```typescript
interface HeaderProps {
  text: string;           // e.g., "YEEHAW" or "ACME-WEBAPP"
  subtitle?: string;      // e.g., "~/Sites/acme-webapp"
}

export function Header({ text, subtitle }: HeaderProps) {
  const [ascii, setAscii] = useState('');

  useEffect(() => {
    figlet.text(text.toUpperCase(), { font: 'ANSI Shadow' }, (err, result) => {
      if (!err && result) setAscii(result);
    });
  }, [text]);

  return (
    <Box flexDirection="column">
      <Text color="yellow">{ascii}</Text>
      {subtitle && <Text color="gray">{subtitle}</Text>}
    </Box>
  );
}
```

**Verification:** Header renders different text for "YEEHAW" vs project names.

---

### Task 3.2: Create GlobalDashboard View

**New File:** `/Users/cam/Sites/Yeehaw/cli/src/views/GlobalDashboard.tsx`

```typescript
interface GlobalDashboardProps {
  projects: Project[];
  barns: Barn[];
  windows: TmuxWindow[];
  onSelectProject: (project: Project) => void;
  onAttachWindow: (index: number) => void;
}

export function GlobalDashboard({
  projects, barns, windows, onSelectProject, onAttachWindow
}: GlobalDashboardProps) {
  const [focusedPanel, setFocusedPanel] = useState<'projects' | 'sessions' | 'barns'>('projects');

  // Render three panels:
  // - Projects list (with session count indicators)
  // - Sessions list (all windows across all projects)
  // - Barns status row (compact, at bottom)

  return (
    <Box flexDirection="column" flexGrow={1}>
      <Header text="YEEHAW" />

      <Box flexGrow={1}>
        {/* Left: Projects */}
        <Panel title="Projects" focused={focusedPanel === 'projects'}>
          <List
            items={projects.map(p => ({
              id: p.name,
              label: p.name,
              hint: `${countSessionsForProject(p.name, windows)} sessions`
            }))}
            onSelect={(id) => onSelectProject(projects.find(p => p.name === id)!)}
          />
        </Panel>

        {/* Right: Sessions */}
        <Panel title="Sessions" focused={focusedPanel === 'sessions'}>
          <List
            items={windows.filter(w => w.index > 0).map(w => ({
              id: String(w.index),
              label: `[${w.index}] ${w.name}`,
            }))}
            onSelect={(id) => onAttachWindow(parseInt(id, 10))}
          />
        </Panel>
      </Box>

      {/* Bottom: Barns status */}
      <Panel title="Barns" height={3}>
        <BarnsStatusRow barns={barns} />
      </Panel>
    </Box>
  );
}
```

**Verification:** App shows global dashboard on launch with all projects/sessions/barns.

---

### Task 3.3: Refactor Home to ProjectContext View

**File:** `/Users/cam/Sites/Yeehaw/cli/src/views/Home.tsx` → Rename to `ProjectContext.tsx`

```typescript
interface ProjectContextProps {
  project: Project;
  barns: Barn[];
  windows: TmuxWindow[];
  onBack: () => void;
  onCreateClaude: (workingDir: string, name: string) => void;
  onAttachWindow: (index: number) => void;
  onSSH: (deployment: Deployment) => void;
}

export function ProjectContext({
  project, barns, windows, onBack, onCreateClaude, onAttachWindow, onSSH
}: ProjectContextProps) {
  const [focusedPanel, setFocusedPanel] = useState<'deployments' | 'sessions'>('deployments');

  // Filter windows to this project (by name prefix or metadata)
  const projectWindows = windows.filter(w => w.name.startsWith(project.name));

  // Get deployments with resolved barn info
  const deploymentsWithBarns = project.deployments.map(d => ({
    ...d,
    barn: barns.find(b => b.name === d.barn)
  }));

  useInput((input, key) => {
    if (key.escape || input === 'q') onBack();
    if (input === 'c') onCreateClaude(project.path, `${project.name}-claude`);
    // ... other keybindings
  });

  return (
    <Box flexDirection="column" flexGrow={1}>
      <Header text={project.name} subtitle={project.path} />

      <Box flexGrow={1}>
        {/* Left: Deployments */}
        <Panel title="Deployments" focused={focusedPanel === 'deployments'}>
          <List
            items={deploymentsWithBarns.map(d => ({
              id: d.name,
              label: `${d.name} (${d.barn?.host || 'unknown'})`,
              hint: d.path
            }))}
            onSelect={(id) => {
              const deployment = project.deployments.find(d => d.name === id);
              if (deployment) onSSH(deployment);
            }}
          />
        </Panel>

        {/* Right: Sessions */}
        <Panel title="Sessions" focused={focusedPanel === 'sessions'}>
          <List
            items={projectWindows.map(w => ({
              id: String(w.index),
              label: `[${w.index}] ${w.name}`,
            }))}
            onSelect={(id) => onAttachWindow(parseInt(id, 10))}
          />
          <Text color="gray">[c] new claude session</Text>
        </Panel>
      </Box>
    </Box>
  );
}
```

**Verification:**
1. Select project from global → shows project context with project name as header
2. Esc returns to global dashboard
3. Deployments panel shows barn+path combos
4. Sessions panel shows only this project's windows

---

### Task 3.4: Update App.tsx Navigation Flow

**File:** `/Users/cam/Sites/Yeehaw/cli/src/app.tsx`

Refactor to handle two-tier navigation:

```typescript
type AppView =
  | { type: 'global' }
  | { type: 'project'; project: Project };

export function App() {
  const { config, projects, barns, currentProject, setCurrentProjectName } = useConfig();
  const { windows, createClaude, createShell, attachToWindow } = useSessions();
  const [view, setView] = useState<AppView>({ type: 'global' });

  const handleSelectProject = (project: Project) => {
    setCurrentProjectName(project.name);
    setView({ type: 'project', project });
  };

  const handleBack = () => {
    setView({ type: 'global' });
  };

  if (view.type === 'global') {
    return (
      <GlobalDashboard
        projects={projects}
        barns={barns}
        windows={windows}
        onSelectProject={handleSelectProject}
        onAttachWindow={attachToWindow}
      />
    );
  }

  return (
    <ProjectContext
      project={view.project}
      barns={barns}
      windows={windows}
      onBack={handleBack}
      onCreateClaude={createClaude}
      onAttachWindow={attachToWindow}
      onSSH={handleSSH}
    />
  );
}
```

**Verification:**
1. App launches to global dashboard
2. Select project → project context
3. Esc → back to global
4. Navigation state is correct throughout

---

### Task 3.5: Update StatusBar for Context-Aware Hints

**File:** `/Users/cam/Sites/Yeehaw/cli/src/components/StatusBar.tsx`

Make hints dynamic based on current view:

```typescript
interface StatusBarProps {
  view: 'global' | 'project';
}

export function StatusBar({ view }: StatusBarProps) {
  const hints = view === 'global'
    ? '[Enter] select  [s] sessions  [b] barns  [?] help  [q] quit'
    : '[c] claude  [Enter] ssh  [?] help  [Esc] back  [q] quit';

  return (
    <Box borderStyle="single" borderTop borderBottom={false} borderLeft={false} borderRight={false}>
      <Text>{hints}</Text>
    </Box>
  );
}
```

**Verification:** Status bar shows correct hints for each view.

---

## Phase 4: Branded tmux

### Task 4.1: Context-Aware Status Bar Content

**File:** `/Users/cam/Sites/Yeehaw/cli/src/lib/tmux-config.ts`

Update status bar to show project context:

```typescript
export function updateTmuxStatusBar(projectName?: string): void {
  const left = projectName
    ? `#[bold] YEEHAW | ${projectName} `
    : '#[bold] YEEHAW ';

  execSync(`tmux set -g status-left "${left}"`);
}
```

Call this when entering/leaving project context.

**Verification:** tmux status bar updates when switching projects.

---

### Task 4.2: Hide Status Bar in Yeehaw Window

**File:** `/Users/cam/Sites/Yeehaw/cli/src/lib/tmux-config.ts`

Add hook to hide status when in window 0:

```typescript
export function setupStatusBarHooks(): void {
  // tmux hook to hide status in window 0
  const hookCmd = `tmux set-hook -g window-pane-changed 'if-shell "[ #{window_index} -eq 0 ]" "set status off" "set status on"'`;
  execSync(hookCmd);
}
```

Call this in `createYeehawSession()`.

**Verification:**
1. In window 0 (Yeehaw TUI) - no tmux status bar
2. In window 1+ (Claude/shell) - yellow tmux status bar visible

---

## Summary Checklist

### Phase 1: Data Model
- [ ] Task 1.1: Add Deployment type
- [ ] Task 1.2: Update Project type
- [ ] Task 1.3: Update Session type
- [ ] Task 1.4: Update useConfig for deployments

### Phase 2: tmux Integration
- [ ] Task 2.1: Create tmux.conf template
- [ ] Task 2.2: Refactor tmux.ts for windows
- [ ] Task 2.3: Update entry point for tmux
- [ ] Task 2.4: Update useSessions hook

### Phase 3: Two-Tier UI
- [ ] Task 3.1: Dynamic ASCII Header
- [ ] Task 3.2: GlobalDashboard view
- [ ] Task 3.3: ProjectContext view
- [ ] Task 3.4: App.tsx navigation
- [ ] Task 3.5: Context-aware StatusBar

### Phase 4: Branded tmux
- [ ] Task 4.1: Context-aware tmux status
- [ ] Task 4.2: Hide status in window 0

---

## Success Criteria

1. User launches `yeehaw` → sees global dashboard with YEEHAW header
2. User selects project → sees project context with PROJECT NAME header
3. User presses `c` → Claude opens in new tmux window
4. User presses `Ctrl-y` → returns to Yeehaw instantly
5. tmux bar is yellow/brown branded (not green)
6. `Esc` from project context → returns to global dashboard
7. Deployments show barn + remote path correctly
