# Livestock Detail Page Design

## Overview

Add a detail page for livestock instances, fixing a bug with local livestock visibility, and establishing consistent navigation patterns across the app.

## Changes

### 1. Bug Fix: Local Livestock in Barn View

**Problem**: `getLivestockForBarn('local')` returns empty because local livestock has `barn: undefined`, not `barn: 'local'`.

**Fix**: In `src/lib/config.ts`, update `getLivestockForBarn()`:

```typescript
export function getLivestockForBarn(barnName: string): Array<{ project: Project; livestock: Livestock }> {
  const projects = loadProjects();
  const result: Array<{ project: Project; livestock: Livestock }> = [];

  for (const project of projects) {
    for (const livestock of project.livestock || []) {
      // Match by barn name, or match local barn with undefined/missing barn field
      if (livestock.barn === barnName || (barnName === 'local' && !livestock.barn)) {
        result.push({ project, livestock });
      }
    }
  }

  return result;
}
```

### 2. Navigation Model Update

Establish consistent navigation across the app:

| Action | Meaning |
|--------|---------|
| Enter | Navigate into detail/context page |
| s | Open shell/session (SSH for remote, shell for local) |

**Changes required:**

- **ProjectContext**: Livestock list Enter → Livestock Detail Page (currently opens tmux)
- **ProjectContext**: Add `s` hotkey on livestock → opens tmux session
- **BarnContext**: Livestock list Enter → Livestock Detail Page (currently opens tmux)
- **BarnContext**: Add `s` hotkey on livestock → opens tmux session

### 3. New View: LivestockDetailView

**Location**: `src/views/LivestockDetailView.tsx`

**Props**:
```typescript
interface LivestockDetailViewProps {
  project: Project;
  livestock: Livestock;
  onBack: () => void;
  onEdit: (livestock: Livestock) => void;
  onOpenSession: (project: Project, livestock: Livestock) => void;
}
```

**Layout**: Single panel displaying livestock configuration:

```
┌─ Livestock: production ──────────────────────────────────┐
│                                                          │
│  Path:      /home/forge/ascendtraining.io/               │
│  Barn:      ascend (ascendtraining.io)                   │
│  Repo:      git@github.com:Colmbus72/ascend-core.git     │
│  Branch:    master                                       │
│  Log Path:  storage/logs                                 │
│  Env Path:  .env                                         │
│                                                          │
│                                                          │
│  [s] shell  [l] logs  [e] edit  [q] back                 │
└──────────────────────────────────────────────────────────┘
```

**Hotkeys**:
- `s` → Open tmux session for this livestock
- `l` → Open LogsView
- `e` → Edit livestock (reuse existing edit form pattern)
- `q`/Esc → Back to previous context

### 4. New View: LogsView

**Location**: `src/views/LogsView.tsx`

**Props**:
```typescript
interface LogsViewProps {
  project: Project;
  livestock: Livestock;
  onBack: () => void;
}
```

**Behavior**:
- On mount, calls `readLivestockLogs()` to fetch logs
- Shows loading state while fetching
- Displays error if log_path not configured or fetch fails
- Full-screen scrollable content (uses ScrollableMarkdown or similar)

**Layout**:
```
┌─ Logs: production ───────────────────────────────────────┐
│                                                          │
│  [2026-01-26 10:23:45] INFO: Request started...          │
│  [2026-01-26 10:23:45] INFO: Processing user 123         │
│  [2026-01-26 10:23:46] ERROR: Database connection failed │
│  Stack trace:                                            │
│    at Connection.connect (db.js:45)                      │
│    at Pool.getConnection (pool.js:123)                   │
│    ...                                                   │
│                                                          │
│  [j/k] scroll  [g/G] top/bottom  [r] refresh  [q] back   │
└──────────────────────────────────────────────────────────┘
```

**Hotkeys**:
- `j`/`k`/arrows → Scroll line by line
- `g`/`G` → Jump to top/bottom
- PgUp/PgDn → Scroll page
- `r` → Refresh (re-fetch logs)
- `q`/Esc → Back to livestock detail

**Data fetching**:
- No caching - fetches fresh each time view opens or `r` pressed
- Uses existing `readLivestockLogs()` from `src/lib/livestock.ts`
- Default 100 lines (can increase later if needed)

### 5. App View Type Updates

**In `src/types.ts`**, add new view types:

```typescript
export type AppView =
  | { type: 'global' }
  | { type: 'project'; project: Project }
  | { type: 'barn'; barn: Barn }
  | { type: 'wiki'; project: Project }
  | { type: 'issues'; project: Project }
  | { type: 'livestock'; project: Project; livestock: Livestock }
  | { type: 'logs'; project: Project; livestock: Livestock };
```

### 6. Hotkey Registry Updates

Add to `src/lib/hotkeys.ts`:

```typescript
// New scopes
type HotkeyScope = ... | 'livestock-detail' | 'logs-view';

// New hotkeys
{ key: 's', description: 'Open shell session', category: 'action', scopes: ['project-context', 'barn-context', 'livestock-detail'] },
{ key: 'l', description: 'View logs', category: 'action', scopes: ['livestock-detail'] },
{ key: 'r', description: 'Refresh', category: 'action', scopes: ['logs-view', 'issues-view'] },
```

## File Changes Summary

| File | Change |
|------|--------|
| `src/lib/config.ts` | Fix `getLivestockForBarn()` for local livestock |
| `src/types.ts` | Add `livestock` and `logs` view types |
| `src/lib/hotkeys.ts` | Add new scopes and hotkeys |
| `src/views/LivestockDetailView.tsx` | New file |
| `src/views/LogsView.tsx` | New file |
| `src/views/ProjectContext.tsx` | Change Enter behavior, add `s` hotkey |
| `src/views/BarnContext.tsx` | Change Enter behavior, add `s` hotkey |
| `src/app.tsx` | Add view routing for livestock and logs views |

## Future Considerations (not in this implementation)

- Log grouping/collapsing for stack traces
- Adapter system for custom log parsers
- Environment variable viewing (`e` or separate hotkey)
- Log filtering by pattern (already supported in `readLivestockLogs`)
- Log line count configuration
