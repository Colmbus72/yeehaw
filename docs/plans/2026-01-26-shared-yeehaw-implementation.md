# Shared Yeehaw Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable users to switch between local and remote Yeehaw instances running on barns via Ctrl+number hotkeys.

**Architecture:** Detection probes barns via SSH for running tmux sessions. The BottomBar shows available environments on the right. Connecting creates an SSH window that attaches to the remote tmux. Remote mode unbinds local keys and shows a minimal status bar. Ctrl-\ returns to local.

**Tech Stack:** React/Ink, tmux, SSH, execa for shell commands

---

## Task 1: Fix detachFromSession Bug

**Files:**
- Modify: `src/lib/tmux.ts:209-211`

**Step 1: Fix the detach command**

The current code detaches ALL clients from the session. Remove the `-s` flag to detach only the current client.

In `src/lib/tmux.ts`, change:

```typescript
export function detachFromSession(): void {
  execaSync('tmux', ['detach-client', '-s', YEEHAW_SESSION]);
}
```

To:

```typescript
export function detachFromSession(): void {
  execaSync('tmux', ['detach-client']);
}
```

**Step 2: Test manually**

1. Open two terminals
2. Run `yeehaw` in both (they attach to same session)
3. Press `q` in one terminal
4. Verify the other terminal remains attached

**Step 3: Commit**

```bash
git add src/lib/tmux.ts
git commit -m "fix(tmux): detach only current client, not all clients"
```

---

## Task 2: Create Detection Module

**Files:**
- Create: `src/lib/detection.ts`

**Step 1: Create the detection module**

Create `src/lib/detection.ts`:

```typescript
import { execa } from 'execa';
import type { Barn } from '../types.js';
import { hasValidSshConfig, isLocalBarn } from './config.js';

export type DetectionState =
  | 'not-checked'
  | 'checking'
  | 'available'
  | 'unavailable'
  | 'unreachable';

export interface BarnDetectionResult {
  barnName: string;
  state: DetectionState;
  checkedAt: number;
}

const CACHE_TTL_MS = 5 * 60 * 1000; // 5 minutes
const SSH_TIMEOUT_SECONDS = 5;

/**
 * Probe a single barn to check if Yeehaw is running.
 * Returns 'available' if tmux session 'yeehaw' exists on the remote.
 */
export async function probeBarns(barns: Barn[]): Promise<BarnDetectionResult[]> {
  const sshBarns = barns.filter(b => !isLocalBarn(b) && hasValidSshConfig(b));

  const probes = sshBarns.map(async (barn): Promise<BarnDetectionResult> => {
    if (!hasValidSshConfig(barn)) {
      return { barnName: barn.name, state: 'unreachable', checkedAt: Date.now() };
    }

    try {
      const result = await execa('ssh', [
        '-o', 'ConnectTimeout=' + SSH_TIMEOUT_SECONDS,
        '-o', 'BatchMode=yes',
        '-o', 'StrictHostKeyChecking=accept-new',
        '-p', String(barn.port),
        '-i', barn.identity_file,
        `${barn.user}@${barn.host}`,
        'tmux has-session -t yeehaw 2>/dev/null && echo "yeehaw:running"'
      ], { timeout: (SSH_TIMEOUT_SECONDS + 2) * 1000 });

      const state: DetectionState = result.stdout.includes('yeehaw:running')
        ? 'available'
        : 'unavailable';

      return { barnName: barn.name, state, checkedAt: Date.now() };
    } catch {
      return { barnName: barn.name, state: 'unreachable', checkedAt: Date.now() };
    }
  });

  return Promise.all(probes);
}

/**
 * Check if a cached result is still fresh.
 */
export function isCacheFresh(result: BarnDetectionResult): boolean {
  return Date.now() - result.checkedAt < CACHE_TTL_MS;
}
```

**Step 2: Verify it compiles**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run typecheck
```

Expected: No errors related to detection.ts

**Step 3: Commit**

```bash
git add src/lib/detection.ts
git commit -m "feat(detection): add module to probe barns for remote Yeehaw instances"
```

---

## Task 3: Create useRemoteYeehaw Hook

**Files:**
- Create: `src/hooks/useRemoteYeehaw.ts`
- Modify: `src/hooks/index.ts`

**Step 1: Create the hook**

Create `src/hooks/useRemoteYeehaw.ts`:

```typescript
import { useState, useEffect, useCallback } from 'react';
import type { Barn } from '../types.js';
import { probeBarns, isCacheFresh, type BarnDetectionResult, type DetectionState } from '../lib/detection.js';
import { hasValidSshConfig, isLocalBarn } from '../lib/config.js';

export interface RemoteEnvironment {
  barn: Barn;
  state: DetectionState;
}

interface UseRemoteYeehawReturn {
  environments: RemoteEnvironment[];
  isDetecting: boolean;
  refresh: () => void;
}

export function useRemoteYeehaw(barns: Barn[]): UseRemoteYeehawReturn {
  const [results, setResults] = useState<Map<string, BarnDetectionResult>>(new Map());
  const [isDetecting, setIsDetecting] = useState(false);

  const sshBarns = barns.filter(b => !isLocalBarn(b) && hasValidSshConfig(b));

  const runDetection = useCallback(async () => {
    if (sshBarns.length === 0) return;

    setIsDetecting(true);
    try {
      const detectionResults = await probeBarns(sshBarns);
      setResults(prev => {
        const next = new Map(prev);
        for (const result of detectionResults) {
          next.set(result.barnName, result);
        }
        return next;
      });
    } finally {
      setIsDetecting(false);
    }
  }, [sshBarns.map(b => b.name).join(',')]);

  // Run detection on mount and when barns change
  useEffect(() => {
    // Check if we need to refresh any cached results
    const needsRefresh = sshBarns.some(barn => {
      const cached = results.get(barn.name);
      return !cached || !isCacheFresh(cached);
    });

    if (needsRefresh) {
      runDetection();
    }
  }, [sshBarns.map(b => b.name).join(',')]);

  // Build environments list - only include available barns
  const environments: RemoteEnvironment[] = sshBarns
    .map(barn => ({
      barn,
      state: results.get(barn.name)?.state ?? 'not-checked',
    }))
    .filter(env => env.state === 'available');

  return {
    environments,
    isDetecting,
    refresh: runDetection,
  };
}
```

**Step 2: Export from index**

In `src/hooks/index.ts`, add:

```typescript
export { useRemoteYeehaw } from './useRemoteYeehaw.js';
```

**Step 3: Verify it compiles**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run typecheck
```

**Step 4: Commit**

```bash
git add src/hooks/useRemoteYeehaw.ts src/hooks/index.ts
git commit -m "feat(hooks): add useRemoteYeehaw for detecting remote instances"
```

---

## Task 4: Add Remote Mode Functions to tmux.ts

**Files:**
- Modify: `src/lib/tmux.ts`

**Step 1: Add remote mode constants and types**

At the top of `src/lib/tmux.ts` after the existing imports and constants, add:

```typescript
// Remote mode state tracking
let remoteWindowIndex: number | null = null;

// Keys to unbind when entering remote mode (so they pass through to inner tmux)
const REMOTE_MODE_UNBIND_KEYS = ['C-h', 'C-l', 'C-y'];
```

**Step 2: Add enterRemoteMode function**

Add this function after `createSshWindow`:

```typescript
export function enterRemoteMode(
  barnName: string,
  host: string,
  user: string,
  port: number,
  identityFile: string
): number {
  // 1. Create SSH window that attaches to remote yeehaw tmux
  const windowName = `remote:${barnName}`;
  const remoteCmd = 'tmux attach -t yeehaw';

  const sshParts = [
    'ssh',
    '-p', String(port),
    '-i', shellEscape(identityFile),
    '-t',  // Force TTY allocation
    shellEscape(`${user}@${host}`),
    shellEscape(remoteCmd)
  ];

  const sshCmd = sshParts.join(' ');

  execaSync('tmux', [
    'new-window',
    '-a',
    '-t', YEEHAW_SESSION,
    '-n', windowName,
    sshCmd,
  ]);

  const result = execaSync('tmux', [
    'display-message', '-p', '#{window_index}'
  ]);
  const windowIndex = parseInt(result.stdout.trim(), 10);
  remoteWindowIndex = windowIndex;

  // 2. Unbind navigation keys so they pass through to inner tmux
  for (const key of REMOTE_MODE_UNBIND_KEYS) {
    try {
      execaSync('tmux', ['unbind-key', '-n', key]);
    } catch {
      // Key might not be bound, ignore
    }
  }

  // 3. Bind Ctrl-\ as escape hatch
  try {
    execaSync('tmux', [
      'bind-key', '-n', 'C-\\',
      `run-shell "tmux kill-window -t ${YEEHAW_SESSION}:${windowIndex}; exit 0"`
    ]);
  } catch {
    // Ignore errors
  }

  // 4. Show minimal status bar with connection info
  try {
    execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status', 'on']);
    execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status-left', `#[bold] Connected to: ${barnName} `]);
    execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status-right', ' Ctrl-\\ back ']);
  } catch {
    // Not critical
  }

  // 5. Set up hook to restore when remote window closes
  try {
    execaSync('tmux', [
      'set-hook', '-t', YEEHAW_SESSION,
      'window-unlinked',
      `if-shell "[ ! -z \\"#{@remote_mode}\\" ]" "run-shell \\"tmux set -u @remote_mode; tmux source-file ${TMUX_CONFIG_PATH}; tmux select-window -t ${YEEHAW_SESSION}:0; tmux set status off\\""`
    ]);
    // Mark that we're in remote mode
    execaSync('tmux', ['set', '-t', YEEHAW_SESSION, '@remote_mode', '1']);
  } catch {
    // Hooks might fail on older tmux
  }

  return windowIndex;
}
```

**Step 3: Add exitRemoteMode function**

Add after `enterRemoteMode`:

```typescript
export function exitRemoteMode(): void {
  // Kill the remote window if it exists
  if (remoteWindowIndex !== null) {
    try {
      execaSync('tmux', ['kill-window', '-t', `${YEEHAW_SESSION}:${remoteWindowIndex}`]);
    } catch {
      // Window might already be dead
    }
    remoteWindowIndex = null;
  }

  // Restore keybindings by re-sourcing config
  try {
    execaSync('tmux', ['source-file', TMUX_CONFIG_PATH]);
  } catch {
    // Not critical
  }

  // Unbind the escape hatch
  try {
    execaSync('tmux', ['unbind-key', '-n', 'C-\\']);
  } catch {
    // Ignore
  }

  // Hide status bar and switch to window 0
  try {
    execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status', 'off']);
    execaSync('tmux', ['select-window', '-t', `${YEEHAW_SESSION}:0`]);
    execaSync('tmux', ['set', '-u', '-t', YEEHAW_SESSION, '@remote_mode']);
  } catch {
    // Not critical
  }
}

export function isInRemoteMode(): boolean {
  return remoteWindowIndex !== null;
}

export function getRemoteWindowIndex(): number | null {
  return remoteWindowIndex;
}
```

**Step 4: Add import for TMUX_CONFIG_PATH if not already imported**

Verify at top of file that `TMUX_CONFIG_PATH` is imported from `./tmux-config.js`. It should already be there.

**Step 5: Verify it compiles**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run typecheck
```

**Step 6: Commit**

```bash
git add src/lib/tmux.ts
git commit -m "feat(tmux): add enterRemoteMode and exitRemoteMode functions"
```

---

## Task 5: Update BottomBar Component

**Files:**
- Modify: `src/components/BottomBar.tsx`

**Step 1: Update BottomBar to accept environments**

Replace the entire `src/components/BottomBar.tsx` with:

```typescript
import React from 'react';
import { Box, Text } from 'ink';

interface RemoteEnvironment {
  barn: { name: string };
  state: 'available' | 'not-checked' | 'checking' | 'unavailable' | 'unreachable';
}

interface BottomBarProps {
  items: Array<{ key: string; label: string }>;
  environments?: RemoteEnvironment[];
  isDetecting?: boolean;
}

// Yeehaw brand gold
const BRAND_COLOR = '#f0c040';

export function BottomBar({ items, environments = [], isDetecting = false }: BottomBarProps) {
  return (
    <Box paddingX={2} justifyContent="space-between">
      {/* Left side: help items */}
      <Box gap={2}>
        {items.map((item, i) => (
          <Text key={i}>
            <Text color={BRAND_COLOR}>{item.key}</Text>
            <Text dimColor> {item.label}</Text>
          </Text>
        ))}
      </Box>

      {/* Right side: environments */}
      <Box gap={2}>
        <Text>
          <Text color="green">[Local]</Text>
        </Text>
        {environments.map((env, i) => (
          <Text key={env.barn.name}>
            <Text color={BRAND_COLOR}>^{i + 1}</Text>
            <Text dimColor> {env.barn.name}</Text>
          </Text>
        ))}
        {isDetecting && (
          <Text dimColor>...</Text>
        )}
      </Box>
    </Box>
  );
}
```

**Step 2: Verify it compiles**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run typecheck
```

**Step 3: Commit**

```bash
git add src/components/BottomBar.tsx
git commit -m "feat(ui): update BottomBar to show remote environments on right side"
```

---

## Task 6: Integrate Remote Yeehaw in App

**Files:**
- Modify: `src/app.tsx`

**Step 1: Add imports**

At the top of `src/app.tsx`, add to the existing imports:

```typescript
import { useRemoteYeehaw } from './hooks/useRemoteYeehaw.js';
import {
  hasTmux,
  switchToWindow,
  updateStatusBar,
  createShellWindow,
  createSshWindow,
  detachFromSession,
  killYeehawSession,
  enterRemoteMode,
} from './lib/tmux.js';
```

(Note: `enterRemoteMode` is the new addition to the existing import)

**Step 2: Add the hook in App component**

Inside the `App` function, after the existing hooks (around line 91), add:

```typescript
const { environments, isDetecting, refresh: refreshEnvironments } = useRemoteYeehaw(barns);
```

**Step 3: Add handler for connecting to remote**

After `handleSshToBarn` callback (around line 285), add:

```typescript
const handleConnectToRemote = useCallback((envIndex: number) => {
  if (!tmuxAvailable) {
    setError('tmux is not installed');
    return;
  }

  const env = environments[envIndex];
  if (!env || env.state !== 'available') {
    setError('Remote Yeehaw not available');
    return;
  }

  const { barn } = env;
  if (!barn.host || !barn.user || !barn.port || !barn.identity_file) {
    setError(`Barn '${barn.name}' is missing SSH configuration`);
    return;
  }

  enterRemoteMode(barn.name, barn.host, barn.user, barn.port, barn.identity_file);
  switchToWindow(environments[envIndex] ? envIndex + 1 : 1); // +1 because window 0 is local yeehaw
}, [tmuxAvailable, environments]);
```

**Step 4: Add Ctrl+number input handling**

Inside the `useInput` callback, after the `if (key.escape)` block (around line 343), add:

```typescript
// Ctrl+number: Connect to remote environment
if (key.ctrl && /^[1-9]$/.test(input)) {
  const envIndex = parseInt(input, 10) - 1;
  if (envIndex < environments.length) {
    handleConnectToRemote(envIndex);
  }
  return;
}
```

**Step 5: Pass environments to BottomBar**

Update the BottomBar rendering at the end of the component (around line 487):

```typescript
{!showHelp && (
  <BottomBar
    items={getBottomBarItems(view.type)}
    environments={environments}
    isDetecting={isDetecting}
  />
)}
```

**Step 6: Verify it compiles**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run typecheck
```

**Step 7: Commit**

```bash
git add src/app.tsx
git commit -m "feat(app): integrate remote Yeehaw switching with Ctrl+number"
```

---

## Task 7: Export Detection Module

**Files:**
- Modify: `src/lib/index.ts`

**Step 1: Add export**

In `src/lib/index.ts`, add:

```typescript
export * from './detection.js';
```

**Step 2: Verify it compiles**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run typecheck
```

**Step 3: Commit**

```bash
git add src/lib/index.ts
git commit -m "feat(lib): export detection module"
```

---

## Task 8: Build and Manual Test

**Step 1: Build the project**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run build
```

**Step 2: Test locally**

1. Run `yeehaw`
2. Verify bottom bar shows `[Local]` on the right
3. If you have a barn with SSH config, wait for detection
4. If a remote Yeehaw is found, it should show `^1 barnname`

**Step 3: Test remote connection (if available)**

1. Ensure remote barn has Yeehaw running (`yeehaw` on the remote)
2. Press Ctrl-1 to connect
3. Verify you see remote Yeehaw
4. Verify Ctrl-\ returns you to local

**Step 4: Commit any fixes if needed**

---

## Task 9: Final Cleanup and Documentation

**Step 1: Update the lib/tmux.ts exports**

Ensure all new functions are exported. Add to exports if not already:

```typescript
export {
  // ... existing exports
  enterRemoteMode,
  exitRemoteMode,
  isInRemoteMode,
  getRemoteWindowIndex,
};
```

**Step 2: Final build verification**

```bash
cd /Users/kev/Sites/Yeehaw/cli && npm run build
```

**Step 3: Final commit**

```bash
git add -A
git commit -m "feat(shared-yeehaw): complete implementation of remote Yeehaw switching"
```

---

## Summary

After completing all tasks, you will have:

1. Fixed the detach bug (Task 1)
2. Detection module that probes barns via SSH (Tasks 2, 7)
3. React hook for managing detection state (Task 3)
4. tmux functions for entering/exiting remote mode (Task 4)
5. Updated BottomBar showing environments (Task 5)
6. App integration with Ctrl+number hotkeys (Task 6)
7. Working feature ready for testing (Tasks 8, 9)

The feature matches the design document at `docs/plans/2026-01-26-shared-yeehaw-design.md`.
