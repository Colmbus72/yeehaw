# Yeehaw CLI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a full-screen terminal UI for managing projects, barns (servers), and Claude Code sessions via tmux.

**Architecture:** Ink (React for terminals) renders the TUI. YAML config files in `~/.yeehaw/` store state. tmux manages background sessions. Vim-style keyboard navigation throughout.

**Tech Stack:** Node.js 20+, TypeScript, Ink 4.x, React 18, figlet, chalk, js-yaml, execa

---

## Phase 1: Project Scaffold

### Task 1: Initialize Node.js Project

**Files:**
- Create: `package.json`
- Create: `tsconfig.json`
- Create: `.gitignore`

**Step 1: Create package.json**

```json
{
  "name": "yeehaw",
  "version": "0.1.0",
  "description": "Terminal UI for managing development infrastructure",
  "type": "module",
  "main": "dist/index.js",
  "bin": {
    "yeehaw": "dist/index.js"
  },
  "scripts": {
    "dev": "tsx watch src/index.tsx",
    "build": "tsc",
    "start": "node dist/index.js",
    "typecheck": "tsc --noEmit"
  },
  "dependencies": {
    "ink": "^4.4.1",
    "react": "^18.2.0",
    "figlet": "^1.7.0",
    "chalk": "^5.3.0",
    "js-yaml": "^4.1.0",
    "chokidar": "^3.6.0",
    "execa": "^8.0.1"
  },
  "devDependencies": {
    "@types/figlet": "^1.5.8",
    "@types/js-yaml": "^4.0.9",
    "@types/react": "^18.2.0",
    "tsx": "^4.7.0",
    "typescript": "^5.3.0"
  },
  "engines": {
    "node": ">=20.0.0"
  }
}
```

**Step 2: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "lib": ["ES2022"],
    "outDir": "dist",
    "rootDir": "src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "declaration": true,
    "jsx": "react-jsx",
    "jsxImportSource": "react"
  },
  "include": ["src/**/*"],
  "exclude": ["node_modules", "dist"]
}
```

**Step 3: Create .gitignore**

```
node_modules/
dist/
*.log
.DS_Store
```

**Step 4: Install dependencies**

Run: `npm install`
Expected: `added X packages` with no errors

**Step 5: Verify TypeScript setup**

Run: `npm run typecheck`
Expected: No errors (empty project)

**Step 6: Commit**

```bash
git add package.json tsconfig.json .gitignore package-lock.json
git commit -m "chore: initialize Node.js project with TypeScript and Ink"
```

---

### Task 2: Create Directory Structure

**Files:**
- Create: `src/index.tsx`
- Create: `src/app.tsx`
- Create: `src/types.ts`
- Create: `src/components/.gitkeep`
- Create: `src/views/.gitkeep`
- Create: `src/hooks/.gitkeep`
- Create: `src/lib/.gitkeep`

**Step 1: Create src/types.ts**

```typescript
// Core domain types

export interface Config {
  version: number;
  default_project: string | null;
  editor: string;
  theme: 'dark' | 'light';
  show_activity: boolean;
  claude: ClaudeConfig;
  tmux: TmuxConfig;
}

export interface ClaudeConfig {
  model: string;
  auto_attach: boolean;
}

export interface TmuxConfig {
  session_prefix: string;
  default_shell: string;
}

export interface Project {
  name: string;
  path: string;
  repositories: Repository[];
  barns: string[];
  github?: GitHubConfig;
}

export interface Repository {
  url: string;
  path: string;
}

export interface GitHubConfig {
  repo: string;
  sync_issues: boolean;
}

export interface Barn {
  name: string;
  host: string;
  user: string;
  port: number;
  identity_file: string;
  critters?: string[];
  livestock?: Livestock[];
}

export interface Livestock {
  name: string;
  type: string;
  path: string;
}

export interface Session {
  id: string;
  type: 'claude' | 'shell';
  project: string | null;
  barn: string | null;
  tmux_session: string;
  started_at: string;
  working_directory: string;
  notes: string;
  status: 'active' | 'detached' | 'ended';
}

export type View = 'home' | 'barns' | 'sessions' | 'projects';
```

**Step 2: Create src/app.tsx (minimal)**

```tsx
import React from 'react';
import { Box, Text } from 'ink';

export function App() {
  return (
    <Box flexDirection="column">
      <Text>Yeehaw CLI - Coming Soon</Text>
    </Box>
  );
}
```

**Step 3: Create src/index.tsx**

```tsx
#!/usr/bin/env node
import React from 'react';
import { render } from 'ink';
import { App } from './app.js';

render(<App />);
```

**Step 4: Create directory placeholders**

```bash
mkdir -p src/components src/views src/hooks src/lib
touch src/components/.gitkeep src/views/.gitkeep src/hooks/.gitkeep src/lib/.gitkeep
```

**Step 5: Verify app runs**

Run: `npm run dev`
Expected: Shows "Yeehaw CLI - Coming Soon" in terminal, then Ctrl+C to exit

**Step 6: Commit**

```bash
git add src/
git commit -m "chore: add directory structure and minimal app entry point"
```

---

## Phase 2: Core Components

### Task 3: Create Header Component with ASCII Art

**Files:**
- Create: `src/components/Header.tsx`
- Modify: `src/app.tsx`

**Step 1: Create src/components/Header.tsx**

```tsx
import React, { useEffect, useState } from 'react';
import { Box, Text } from 'ink';
import figlet from 'figlet';

interface HeaderProps {
  projectName: string | null;
}

export function Header({ projectName }: HeaderProps) {
  const [ascii, setAscii] = useState<string>('');

  useEffect(() => {
    figlet.text('YEEHAW', { font: 'ANSI Shadow' }, (err, result) => {
      if (!err && result) {
        setAscii(result);
      }
    });
  }, []);

  return (
    <Box flexDirection="column" borderStyle="single" borderColor="yellow" paddingX={1}>
      <Text color="yellow">{ascii}</Text>
      <Box marginTop={1}>
        <Text>
          Project: <Text bold color="green">{projectName ?? '(none)'}</Text>
        </Text>
        <Box marginLeft={4}>
          <Text dimColor>[P]rojects</Text>
        </Box>
      </Box>
    </Box>
  );
}
```

**Step 2: Update src/app.tsx to use Header**

```tsx
import React from 'react';
import { Box, useApp, useInput } from 'ink';
import { Header } from './components/Header.js';

export function App() {
  const { exit } = useApp();

  useInput((input) => {
    if (input === 'q') {
      exit();
    }
  });

  return (
    <Box flexDirection="column">
      <Header projectName="acme-webapp" />
      <Box marginTop={1}>
        {/* Main content will go here */}
      </Box>
    </Box>
  );
}
```

**Step 3: Verify header renders**

Run: `npm run dev`
Expected: ASCII "YEEHAW" art in yellow with project name below. Press `q` to quit.

**Step 4: Commit**

```bash
git add src/components/Header.tsx src/app.tsx
git commit -m "feat: add ASCII art header component"
```

---

### Task 4: Create StatusBar Component

**Files:**
- Create: `src/components/StatusBar.tsx`
- Modify: `src/app.tsx`

**Step 1: Create src/components/StatusBar.tsx**

```tsx
import React from 'react';
import { Box, Text } from 'ink';
import type { View } from '../types.js';

interface Shortcut {
  key: string;
  label: string;
}

const globalShortcuts: Shortcut[] = [
  { key: 'b', label: 'barns' },
  { key: 's', label: 'sessions' },
  { key: 'c', label: 'claude' },
  { key: '?', label: 'help' },
];

interface StatusBarProps {
  view: View;
}

export function StatusBar({ view }: StatusBarProps) {
  return (
    <Box
      borderStyle="single"
      borderColor="gray"
      paddingX={1}
      justifyContent="space-between"
    >
      <Box gap={2}>
        {globalShortcuts.map((shortcut) => (
          <Text key={shortcut.key}>
            <Text color="cyan">[{shortcut.key}]</Text>
            <Text dimColor>{shortcut.label}</Text>
          </Text>
        ))}
      </Box>
      <Text>
        <Text color="red">q</Text>
        <Text dimColor>:quit</Text>
      </Text>
    </Box>
  );
}
```

**Step 2: Update src/app.tsx to use StatusBar**

```tsx
import React, { useState } from 'react';
import { Box, useApp, useInput } from 'ink';
import { Header } from './components/Header.js';
import { StatusBar } from './components/StatusBar.js';
import type { View } from './types.js';

export function App() {
  const { exit } = useApp();
  const [view, setView] = useState<View>('home');

  useInput((input) => {
    if (input === 'q') exit();
    if (input === 'b') setView('barns');
    if (input === 's') setView('sessions');
    if (input === 'p') setView('projects');
  });

  return (
    <Box flexDirection="column">
      <Header projectName="acme-webapp" />
      <Box flexGrow={1} marginY={1}>
        {/* Main content based on view */}
      </Box>
      <StatusBar view={view} />
    </Box>
  );
}
```

**Step 3: Verify status bar renders**

Run: `npm run dev`
Expected: Header at top, status bar at bottom with shortcuts. Press `q` to quit.

**Step 4: Commit**

```bash
git add src/components/StatusBar.tsx src/app.tsx
git commit -m "feat: add status bar with keyboard shortcuts"
```

---

### Task 5: Create Panel Component

**Files:**
- Create: `src/components/Panel.tsx`

**Step 1: Create src/components/Panel.tsx**

```tsx
import React, { ReactNode } from 'react';
import { Box, Text } from 'ink';

interface PanelProps {
  title: string;
  children: ReactNode;
  focused?: boolean;
  width?: number | string;
}

export function Panel({ title, children, focused = false, width }: PanelProps) {
  return (
    <Box
      flexDirection="column"
      borderStyle="single"
      borderColor={focused ? 'cyan' : 'gray'}
      width={width}
    >
      <Box paddingX={1} marginBottom={0}>
        <Text bold color={focused ? 'cyan' : 'white'}>
          {title}
        </Text>
      </Box>
      <Box flexDirection="column" paddingX={1}>
        {children}
      </Box>
    </Box>
  );
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/components/Panel.tsx
git commit -m "feat: add reusable Panel component"
```

---

### Task 6: Create List Component with Vim Navigation

**Files:**
- Create: `src/components/List.tsx`

**Step 1: Create src/components/List.tsx**

```tsx
import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';

export interface ListItem {
  id: string;
  label: string;
  status?: 'active' | 'inactive' | 'error';
  meta?: string;
}

interface ListProps {
  items: ListItem[];
  focused?: boolean;
  onSelect?: (item: ListItem) => void;
  onHighlight?: (item: ListItem | null) => void;
}

export function List({ items, focused = false, onSelect, onHighlight }: ListProps) {
  const [selectedIndex, setSelectedIndex] = useState(0);

  useEffect(() => {
    if (items.length > 0 && onHighlight) {
      onHighlight(items[selectedIndex] ?? null);
    }
  }, [selectedIndex, items, onHighlight]);

  useInput((input, key) => {
    if (!focused) return;

    if (input === 'j' || key.downArrow) {
      setSelectedIndex((i) => Math.min(i + 1, items.length - 1));
    }
    if (input === 'k' || key.upArrow) {
      setSelectedIndex((i) => Math.max(i - 1, 0));
    }
    if (input === 'g') {
      setSelectedIndex(0);
    }
    if (input === 'G') {
      setSelectedIndex(items.length - 1);
    }
    if (key.return && items[selectedIndex] && onSelect) {
      onSelect(items[selectedIndex]);
    }
  });

  if (items.length === 0) {
    return <Text dimColor>No items</Text>;
  }

  return (
    <Box flexDirection="column">
      {items.map((item, index) => {
        const isSelected = index === selectedIndex && focused;
        const statusColor =
          item.status === 'active' ? 'green' :
          item.status === 'error' ? 'red' : 'gray';

        return (
          <Box key={item.id} gap={1}>
            <Text color={isSelected ? 'cyan' : undefined}>
              {isSelected ? '›' : ' '}
            </Text>
            <Text color={isSelected ? 'cyan' : undefined} bold={isSelected}>
              {item.label}
            </Text>
            {item.status && (
              <Text color={statusColor}>●</Text>
            )}
            {item.meta && (
              <Text dimColor>{item.meta}</Text>
            )}
          </Box>
        );
      })}
    </Box>
  );
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/components/List.tsx
git commit -m "feat: add List component with Vim-style navigation"
```

---

## Phase 3: Configuration System

### Task 7: Create Config Path Utilities

**Files:**
- Create: `src/lib/paths.ts`

**Step 1: Create src/lib/paths.ts**

```typescript
import { homedir } from 'os';
import { join } from 'path';

export const YEEHAW_DIR = join(homedir(), '.yeehaw');
export const CONFIG_FILE = join(YEEHAW_DIR, 'config.yaml');
export const PROJECTS_DIR = join(YEEHAW_DIR, 'projects');
export const BARNS_DIR = join(YEEHAW_DIR, 'barns');
export const SESSIONS_DIR = join(YEEHAW_DIR, 'sessions');

export function getProjectPath(name: string): string {
  return join(PROJECTS_DIR, `${name}.yaml`);
}

export function getBarnPath(name: string): string {
  return join(BARNS_DIR, `${name}.yaml`);
}

export function getSessionPath(id: string): string {
  return join(SESSIONS_DIR, `${id}.yaml`);
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/lib/paths.ts
git commit -m "feat: add config path utilities"
```

---

### Task 8: Create Config Loading Library

**Files:**
- Create: `src/lib/config.ts`

**Step 1: Create src/lib/config.ts**

```typescript
import { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync } from 'fs';
import YAML from 'js-yaml';
import type { Config, Project, Barn, Session } from '../types.js';
import {
  YEEHAW_DIR,
  CONFIG_FILE,
  PROJECTS_DIR,
  BARNS_DIR,
  SESSIONS_DIR,
  getProjectPath,
  getBarnPath,
} from './paths.js';

const DEFAULT_CONFIG: Config = {
  version: 1,
  default_project: null,
  editor: 'vim',
  theme: 'dark',
  show_activity: true,
  claude: {
    model: 'claude-sonnet-4-20250514',
    auto_attach: true,
  },
  tmux: {
    session_prefix: 'yh-',
    default_shell: '/bin/zsh',
  },
};

export function ensureConfigDirs(): void {
  const dirs = [YEEHAW_DIR, PROJECTS_DIR, BARNS_DIR, SESSIONS_DIR];
  for (const dir of dirs) {
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
    }
  }
}

export function loadConfig(): Config {
  ensureConfigDirs();

  if (!existsSync(CONFIG_FILE)) {
    writeFileSync(CONFIG_FILE, YAML.dump(DEFAULT_CONFIG), 'utf-8');
    return DEFAULT_CONFIG;
  }

  const content = readFileSync(CONFIG_FILE, 'utf-8');
  const parsed = YAML.load(content) as Partial<Config>;
  return { ...DEFAULT_CONFIG, ...parsed };
}

export function loadProjects(): Project[] {
  ensureConfigDirs();

  if (!existsSync(PROJECTS_DIR)) return [];

  const files = readdirSync(PROJECTS_DIR).filter((f) => f.endsWith('.yaml'));
  return files.map((file) => {
    const content = readFileSync(getProjectPath(file.replace('.yaml', '')), 'utf-8');
    return YAML.load(content) as Project;
  });
}

export function loadProject(name: string): Project | null {
  const path = getProjectPath(name);
  if (!existsSync(path)) return null;

  const content = readFileSync(path, 'utf-8');
  return YAML.load(content) as Project;
}

export function loadBarns(): Barn[] {
  ensureConfigDirs();

  if (!existsSync(BARNS_DIR)) return [];

  const files = readdirSync(BARNS_DIR).filter((f) => f.endsWith('.yaml'));
  return files.map((file) => {
    const content = readFileSync(getBarnPath(file.replace('.yaml', '')), 'utf-8');
    return YAML.load(content) as Barn;
  });
}

export function loadBarn(name: string): Barn | null {
  const path = getBarnPath(name);
  if (!existsSync(path)) return null;

  const content = readFileSync(path, 'utf-8');
  return YAML.load(content) as Barn;
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/lib/config.ts
git commit -m "feat: add YAML config loading library"
```

---

### Task 9: Create useConfig Hook

**Files:**
- Create: `src/hooks/useConfig.ts`

**Step 1: Create src/hooks/useConfig.ts**

```typescript
import { useState, useEffect } from 'react';
import { watch } from 'chokidar';
import type { Config, Project, Barn } from '../types.js';
import { loadConfig, loadProjects, loadBarns, loadProject } from '../lib/config.js';
import { YEEHAW_DIR } from '../lib/paths.js';

interface UseConfigReturn {
  config: Config;
  projects: Project[];
  barns: Barn[];
  currentProject: Project | null;
  setCurrentProjectName: (name: string | null) => void;
  reload: () => void;
}

export function useConfig(): UseConfigReturn {
  const [config, setConfig] = useState<Config>(() => loadConfig());
  const [projects, setProjects] = useState<Project[]>(() => loadProjects());
  const [barns, setBarns] = useState<Barn[]>(() => loadBarns());
  const [currentProjectName, setCurrentProjectName] = useState<string | null>(
    () => loadConfig().default_project
  );

  const reload = () => {
    setConfig(loadConfig());
    setProjects(loadProjects());
    setBarns(loadBarns());
  };

  useEffect(() => {
    const watcher = watch(YEEHAW_DIR, {
      ignoreInitial: true,
      depth: 2,
    });

    watcher.on('all', () => {
      reload();
    });

    return () => {
      watcher.close();
    };
  }, []);

  const currentProject = currentProjectName ? loadProject(currentProjectName) : null;

  return {
    config,
    projects,
    barns,
    currentProject,
    setCurrentProjectName,
    reload,
  };
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/hooks/useConfig.ts
git commit -m "feat: add useConfig hook with file watching"
```

---

## Phase 4: tmux Integration

### Task 10: Create tmux Library

**Files:**
- Create: `src/lib/tmux.ts`

**Step 1: Create src/lib/tmux.ts**

```typescript
import { execaSync, execa } from 'execa';
import type { Session } from '../types.js';

export interface TmuxSession {
  name: string;
  created: number;
  activity: number;
  attached: boolean;
}

export function listTmuxSessions(prefix: string = 'yh-'): TmuxSession[] {
  try {
    const result = execaSync('tmux', [
      'list-sessions',
      '-F',
      '#{session_name}:#{session_created}:#{session_activity}:#{session_attached}',
    ]);

    return result.stdout
      .split('\n')
      .filter((line) => line.startsWith(prefix))
      .map((line) => {
        const [name, created, activity, attached] = line.split(':');
        return {
          name,
          created: parseInt(created, 10),
          activity: parseInt(activity, 10),
          attached: attached === '1',
        };
      });
  } catch {
    // tmux server not running or no sessions
    return [];
  }
}

export function createClaudeSession(
  prefix: string,
  workingDirectory: string
): string {
  const sessionName = `${prefix}claude-${Date.now()}`;

  execaSync('tmux', [
    'new-session',
    '-d',
    '-s', sessionName,
    '-c', workingDirectory,
    'claude',
  ]);

  return sessionName;
}

export function createShellSession(
  prefix: string,
  workingDirectory: string,
  shell: string = '/bin/zsh'
): string {
  const sessionName = `${prefix}shell-${Date.now()}`;

  execaSync('tmux', [
    'new-session',
    '-d',
    '-s', sessionName,
    '-c', workingDirectory,
    shell,
  ]);

  return sessionName;
}

export async function attachSession(sessionName: string): Promise<void> {
  // This replaces the current process with tmux attach
  await execa('tmux', ['attach-session', '-t', sessionName], {
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  });
}

export function killSession(sessionName: string): void {
  try {
    execaSync('tmux', ['kill-session', '-t', sessionName]);
  } catch {
    // Session might already be dead
  }
}

export function hasTmux(): boolean {
  try {
    execaSync('which', ['tmux']);
    return true;
  } catch {
    return false;
  }
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/lib/tmux.ts
git commit -m "feat: add tmux session management library"
```

---

### Task 11: Create useSessions Hook

**Files:**
- Create: `src/hooks/useSessions.ts`

**Step 1: Create src/hooks/useSessions.ts**

```typescript
import { useState, useEffect, useCallback } from 'react';
import { listTmuxSessions, createClaudeSession, createShellSession, type TmuxSession } from '../lib/tmux.js';

interface UseSessionsReturn {
  sessions: TmuxSession[];
  loading: boolean;
  refresh: () => void;
  createClaude: (workingDir: string) => string;
  createShell: (workingDir: string) => string;
}

export function useSessions(prefix: string = 'yh-'): UseSessionsReturn {
  const [sessions, setSessions] = useState<TmuxSession[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(() => {
    setLoading(true);
    const result = listTmuxSessions(prefix);
    setSessions(result);
    setLoading(false);
  }, [prefix]);

  useEffect(() => {
    refresh();
    // Poll every 5 seconds for session updates
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, [refresh]);

  const createClaude = useCallback((workingDir: string) => {
    const name = createClaudeSession(prefix, workingDir);
    refresh();
    return name;
  }, [prefix, refresh]);

  const createShell = useCallback((workingDir: string) => {
    const name = createShellSession(prefix, workingDir);
    refresh();
    return name;
  }, [prefix, refresh]);

  return {
    sessions,
    loading,
    refresh,
    createClaude,
    createShell,
  };
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/hooks/useSessions.ts
git commit -m "feat: add useSessions hook for tmux management"
```

---

## Phase 5: Views

### Task 12: Create Home View

**Files:**
- Create: `src/views/Home.tsx`

**Step 1: Create src/views/Home.tsx**

```tsx
import React from 'react';
import { Box, Text } from 'ink';
import { Panel } from '../components/Panel.js';
import { List, type ListItem } from '../components/List.js';
import type { Barn } from '../types.js';
import type { TmuxSession } from '../lib/tmux.js';

interface HomeProps {
  barns: Barn[];
  sessions: TmuxSession[];
  focusedPanel: 'barns' | 'sessions';
  onSelectBarn: (barn: Barn) => void;
  onSelectSession: (session: TmuxSession) => void;
}

export function Home({ barns, sessions, focusedPanel, onSelectBarn, onSelectSession }: HomeProps) {
  const barnItems: ListItem[] = barns.map((barn) => ({
    id: barn.name,
    label: barn.name,
    status: 'active', // TODO: actual status check
    meta: barn.host,
  }));

  const sessionItems: ListItem[] = sessions.map((session) => ({
    id: session.name,
    label: session.name.replace(/^yh-/, ''),
    status: session.attached ? 'active' : 'inactive',
    meta: session.attached ? 'attached' : 'detached',
  }));

  return (
    <Box gap={2}>
      <Panel title="Barns" focused={focusedPanel === 'barns'} width="50%">
        <List
          items={barnItems}
          focused={focusedPanel === 'barns'}
          onSelect={(item) => {
            const barn = barns.find((b) => b.name === item.id);
            if (barn) onSelectBarn(barn);
          }}
        />
      </Panel>
      <Panel title="Sessions" focused={focusedPanel === 'sessions'} width="50%">
        <List
          items={sessionItems}
          focused={focusedPanel === 'sessions'}
          onSelect={(item) => {
            const session = sessions.find((s) => s.name === item.id);
            if (session) onSelectSession(session);
          }}
        />
      </Panel>
    </Box>
  );
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/views/Home.tsx
git commit -m "feat: add Home view with barns and sessions panels"
```

---

### Task 13: Create Projects View

**Files:**
- Create: `src/views/Projects.tsx`

**Step 1: Create src/views/Projects.tsx**

```tsx
import React from 'react';
import { Box, Text } from 'ink';
import { Panel } from '../components/Panel.js';
import { List, type ListItem } from '../components/List.js';
import type { Project } from '../types.js';

interface ProjectsProps {
  projects: Project[];
  currentProject: Project | null;
  onSelect: (project: Project) => void;
}

export function Projects({ projects, currentProject, onSelect }: ProjectsProps) {
  const items: ListItem[] = projects.map((project) => ({
    id: project.name,
    label: project.name,
    status: project.name === currentProject?.name ? 'active' : undefined,
    meta: project.path,
  }));

  if (projects.length === 0) {
    return (
      <Panel title="Projects" focused>
        <Text dimColor>No projects configured.</Text>
        <Text dimColor>Add projects to ~/.yeehaw/projects/</Text>
      </Panel>
    );
  }

  return (
    <Panel title="Select Project" focused>
      <List
        items={items}
        focused
        onSelect={(item) => {
          const project = projects.find((p) => p.name === item.id);
          if (project) onSelect(project);
        }}
      />
      <Box marginTop={1}>
        <Text dimColor>Press Enter to select, Esc to cancel</Text>
      </Box>
    </Panel>
  );
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/views/Projects.tsx
git commit -m "feat: add Projects view for project switching"
```

---

### Task 14: Create HelpOverlay Component

**Files:**
- Create: `src/components/HelpOverlay.tsx`

**Step 1: Create src/components/HelpOverlay.tsx**

```tsx
import React from 'react';
import { Box, Text } from 'ink';

interface Shortcut {
  key: string;
  description: string;
}

const shortcuts: Shortcut[] = [
  { key: 'j/k', description: 'Navigate up/down' },
  { key: 'g/G', description: 'Go to first/last' },
  { key: 'Enter', description: 'Select item' },
  { key: 'Tab', description: 'Switch panel' },
  { key: '', description: '' },
  { key: 'b', description: 'Barns view' },
  { key: 's', description: 'Sessions view' },
  { key: 'p', description: 'Switch project' },
  { key: 'c', description: 'New Claude session' },
  { key: '', description: '' },
  { key: 'Esc', description: 'Back / Cancel' },
  { key: 'q', description: 'Quit' },
  { key: '?', description: 'Toggle help' },
];

export function HelpOverlay() {
  return (
    <Box
      flexDirection="column"
      borderStyle="double"
      borderColor="yellow"
      paddingX={2}
      paddingY={1}
    >
      <Text bold color="yellow">Keyboard Shortcuts</Text>
      <Box marginTop={1} flexDirection="column">
        {shortcuts.map((shortcut, i) => (
          shortcut.key ? (
            <Box key={i} gap={2}>
              <Box width={10}>
                <Text color="cyan">{shortcut.key}</Text>
              </Box>
              <Text>{shortcut.description}</Text>
            </Box>
          ) : (
            <Text key={i}> </Text>
          )
        ))}
      </Box>
      <Box marginTop={1}>
        <Text dimColor>Press ? to close</Text>
      </Box>
    </Box>
  );
}
```

**Step 2: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 3: Commit**

```bash
git add src/components/HelpOverlay.tsx
git commit -m "feat: add HelpOverlay component"
```

---

## Phase 6: Wire It All Together

### Task 15: Integrate All Components in App

**Files:**
- Modify: `src/app.tsx`

**Step 1: Update src/app.tsx with full integration**

```tsx
import React, { useState, useCallback } from 'react';
import { Box, Text, useApp, useInput } from 'ink';
import { Header } from './components/Header.js';
import { StatusBar } from './components/StatusBar.js';
import { HelpOverlay } from './components/HelpOverlay.js';
import { Home } from './views/Home.js';
import { Projects } from './views/Projects.js';
import { useConfig } from './hooks/useConfig.js';
import { useSessions } from './hooks/useSessions.js';
import { attachSession, hasTmux } from './lib/tmux.js';
import type { View } from './types.js';
import type { Barn } from './types.js';
import type { TmuxSession } from './lib/tmux.js';

type FocusedPanel = 'barns' | 'sessions';

export function App() {
  const { exit } = useApp();
  const { config, projects, barns, currentProject, setCurrentProjectName } = useConfig();
  const { sessions, createClaude } = useSessions(config.tmux.session_prefix);

  const [view, setView] = useState<View>('home');
  const [showHelp, setShowHelp] = useState(false);
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('barns');
  const [error, setError] = useState<string | null>(null);

  // Check tmux availability
  const tmuxAvailable = hasTmux();

  const handleSelectBarn = useCallback(async (barn: Barn) => {
    try {
      // SSH into barn (for now, just show info)
      // TODO: Implement SSH with process.stdin handoff
      setError(`SSH to ${barn.host} not yet implemented`);
    } catch (e) {
      setError(`Failed to connect to ${barn.name}`);
    }
  }, []);

  const handleSelectSession = useCallback(async (session: TmuxSession) => {
    try {
      await attachSession(session.name);
    } catch (e) {
      setError(`Failed to attach to ${session.name}`);
    }
  }, []);

  const handleNewClaude = useCallback(() => {
    if (!tmuxAvailable) {
      setError('tmux is not installed');
      return;
    }
    const workingDir = currentProject?.path ?? process.cwd();
    const sessionName = createClaude(workingDir);
    if (config.claude.auto_attach) {
      attachSession(sessionName);
    }
  }, [tmuxAvailable, currentProject, createClaude, config.claude.auto_attach]);

  useInput((input, key) => {
    // Clear error on any input
    if (error) setError(null);

    // Help toggle
    if (input === '?') {
      setShowHelp((s) => !s);
      return;
    }

    // Don't process other keys when help is shown
    if (showHelp) return;

    // Global shortcuts
    if (input === 'q') {
      if (view !== 'home') {
        setView('home');
      } else {
        exit();
      }
      return;
    }

    if (key.escape) {
      if (view !== 'home') {
        setView('home');
      }
      return;
    }

    if (input === 'b') {
      setView('home');
      setFocusedPanel('barns');
      return;
    }

    if (input === 's') {
      setView('home');
      setFocusedPanel('sessions');
      return;
    }

    if (input === 'p') {
      setView('projects');
      return;
    }

    if (input === 'c') {
      handleNewClaude();
      return;
    }

    // Tab to switch panels in home view
    if (key.tab && view === 'home') {
      setFocusedPanel((p) => (p === 'barns' ? 'sessions' : 'barns'));
      return;
    }
  });

  return (
    <Box flexDirection="column" height="100%">
      <Header projectName={currentProject?.name ?? null} />

      {error && (
        <Box paddingX={1}>
          <Text color="red">Error: {error}</Text>
        </Box>
      )}

      <Box flexGrow={1} marginY={1} paddingX={1}>
        {showHelp ? (
          <HelpOverlay />
        ) : view === 'home' ? (
          <Home
            barns={barns}
            sessions={sessions}
            focusedPanel={focusedPanel}
            onSelectBarn={handleSelectBarn}
            onSelectSession={handleSelectSession}
          />
        ) : view === 'projects' ? (
          <Projects
            projects={projects}
            currentProject={currentProject}
            onSelect={(project) => {
              setCurrentProjectName(project.name);
              setView('home');
            }}
          />
        ) : null}
      </Box>

      <StatusBar view={view} />
    </Box>
  );
}
```

**Step 2: Verify app runs**

Run: `npm run dev`
Expected: Full TUI with header, panels, and status bar. Press `?` for help, `q` to quit.

**Step 3: Commit**

```bash
git add src/app.tsx
git commit -m "feat: integrate all components into main App"
```

---

### Task 16: Add Component Exports

**Files:**
- Create: `src/components/index.ts`
- Create: `src/views/index.ts`
- Create: `src/hooks/index.ts`
- Create: `src/lib/index.ts`

**Step 1: Create barrel exports**

`src/components/index.ts`:
```typescript
export { Header } from './Header.js';
export { StatusBar } from './StatusBar.js';
export { Panel } from './Panel.js';
export { List, type ListItem } from './List.js';
export { HelpOverlay } from './HelpOverlay.js';
```

`src/views/index.ts`:
```typescript
export { Home } from './Home.js';
export { Projects } from './Projects.js';
```

`src/hooks/index.ts`:
```typescript
export { useConfig } from './useConfig.js';
export { useSessions } from './useSessions.js';
```

`src/lib/index.ts`:
```typescript
export * from './paths.js';
export * from './config.js';
export * from './tmux.js';
```

**Step 2: Remove .gitkeep files**

```bash
rm src/components/.gitkeep src/views/.gitkeep src/hooks/.gitkeep src/lib/.gitkeep
```

**Step 3: Verify TypeScript compiles**

Run: `npm run typecheck`
Expected: No errors

**Step 4: Commit**

```bash
git add src/components/index.ts src/views/index.ts src/hooks/index.ts src/lib/index.ts
git rm src/components/.gitkeep src/views/.gitkeep src/hooks/.gitkeep src/lib/.gitkeep
git commit -m "chore: add barrel exports for all modules"
```

---

## Phase 7: Polish and Test

### Task 17: Add Sample Config Files

**Files:**
- Create: `examples/config.yaml`
- Create: `examples/projects/demo.yaml`
- Create: `examples/barns/local.yaml`

**Step 1: Create examples/config.yaml**

```yaml
version: 1

default_project: demo

editor: nvim

theme: dark
show_activity: true

claude:
  model: claude-sonnet-4-20250514
  auto_attach: true

tmux:
  session_prefix: "yh-"
  default_shell: /bin/zsh
```

**Step 2: Create examples/projects/demo.yaml**

```yaml
name: demo
path: ~/Projects/demo

repositories:
  - url: git@github.com:example/demo.git
    path: .

barns:
  - local
```

**Step 3: Create examples/barns/local.yaml**

```yaml
name: local
host: localhost
user: dev
port: 22
identity_file: ~/.ssh/id_rsa

critters:
  - nginx
  - mysql

livestock:
  - name: demo-app
    type: node
    path: /var/www/demo
```

**Step 4: Commit**

```bash
mkdir -p examples/projects examples/barns
git add examples/
git commit -m "docs: add example configuration files"
```

---

### Task 18: Add README

**Files:**
- Create: `README.md`

**Step 1: Create README.md**

```markdown
# Yeehaw CLI

A full-screen terminal UI for managing development infrastructure using the "Infrastructure as Farm" metaphor.

## Features

- **Vim-style navigation** - `j/k` to move, `g/G` for first/last, `Enter` to select
- **Project switching** - Manage multiple projects with `p`
- **Barn management** - SSH into servers with `Enter`
- **Claude sessions** - Start Claude Code sessions with `c` (via tmux)
- **Keyboard-driven** - No mouse needed

## Installation

```bash
npm install -g yeehaw
```

## Quick Start

1. Create config directory:
```bash
mkdir -p ~/.yeehaw/projects ~/.yeehaw/barns
```

2. Add a project (`~/.yeehaw/projects/myproject.yaml`):
```yaml
name: myproject
path: ~/Code/myproject
barns:
  - production
```

3. Add a barn (`~/.yeehaw/barns/production.yaml`):
```yaml
name: production
host: myserver.com
user: deploy
port: 22
identity_file: ~/.ssh/id_rsa
```

4. Run:
```bash
yeehaw
```

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `j/k` | Navigate up/down |
| `g/G` | Go to first/last |
| `Enter` | Select item |
| `Tab` | Switch panel |
| `b` | Barns view |
| `s` | Sessions view |
| `p` | Switch project |
| `c` | New Claude session |
| `?` | Toggle help |
| `q` | Quit / Back |

## Requirements

- Node.js 20+
- tmux (for Claude sessions)

## Development

```bash
# Install dependencies
npm install

# Run in development mode
npm run dev

# Build
npm run build

# Type check
npm run typecheck
```

## License

MIT
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add README with usage instructions"
```

---

### Task 19: Final Verification

**Step 1: Clean build**

```bash
rm -rf dist node_modules
npm install
npm run build
```

Expected: No errors, `dist/` directory created

**Step 2: Run built version**

```bash
node dist/index.js
```

Expected: TUI launches, shows header, responds to keyboard

**Step 3: Test keyboard shortcuts**

- Press `?` - Help overlay appears
- Press `?` again - Help closes
- Press `p` - Projects view (shows "No projects configured")
- Press `Esc` - Returns to home
- Press `Tab` - Focus switches between Barns and Sessions panels
- Press `q` - App exits

**Step 4: Final commit**

```bash
git add -A
git commit -m "chore: verify build and keyboard navigation"
```

---

## Summary

**Total tasks:** 19

**Phase breakdown:**
- Phase 1 (Scaffold): 2 tasks
- Phase 2 (Components): 4 tasks
- Phase 3 (Config): 3 tasks
- Phase 4 (tmux): 2 tasks
- Phase 5 (Views): 3 tasks
- Phase 6 (Integration): 2 tasks
- Phase 7 (Polish): 3 tasks

**What you'll have at the end:**
- Full-screen TUI with ASCII header
- Vim-style keyboard navigation
- Project switching from YAML configs
- Barn listing (SSH placeholder)
- tmux session management
- Claude session spawning
- Help overlay
