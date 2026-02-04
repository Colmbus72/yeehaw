// src/lib/hotkeys.ts

export type HotkeyScope =
  | 'global'           // Available everywhere
  | 'global-dashboard' // GlobalDashboard view
  | 'project-context'  // ProjectContext view
  | 'barn-context'     // BarnContext view
  | 'wiki-view'        // WikiView
  | 'issues-view'      // IssuesView
  | 'livestock-detail' // LivestockDetailView
  | 'logs-view'        // LogsView
  | 'critter-detail'   // CritterDetailView
  | 'critter-logs'     // CritterLogsView
  | 'herd-detail'      // HerdDetailView
  | 'ranchhand-detail' // RanchHandDetailView
  | 'night-sky'        // NightSkyView (screensaver)
  | 'list'             // When focused on a List component
  | 'content';         // When focused on content/markdown panel

export type HotkeyCategory =
  | 'navigation'   // Moving around (j/k, Tab, etc.)
  | 'action'       // Creating/modifying (n, e, d, etc.)
  | 'system';      // Meta actions (?, q, Q)

export interface Hotkey {
  key: string;           // Display key (e.g., 'j/k', 'Tab', 'Ctrl+S')
  description: string;   // What it does (e.g., 'Navigate up/down')
  category: HotkeyCategory;
  scopes: HotkeyScope[]; // Where this hotkey is active
  panel?: string;        // Optional: only when specific panel focused (e.g., 'projects', 'livestock')
}

export const HOTKEYS: Hotkey[] = [
  // === GLOBAL (everywhere) ===
  { key: '?', description: 'Toggle help', category: 'system', scopes: ['global'] },
  { key: 'q', description: 'Detach', category: 'system', scopes: ['global-dashboard'] },
  { key: 'Q', description: 'Quit Yeehaw', category: 'system', scopes: ['global-dashboard'] },
  { key: 'Esc', description: 'Back / Cancel', category: 'system', scopes: ['global'] },

  // === LIST NAVIGATION (any focused list) ===
  { key: 'j/k', description: 'Move up/down', category: 'navigation', scopes: ['list'] },
  { key: 'g/G', description: 'Jump to first/last', category: 'navigation', scopes: ['list'] },
  { key: 'Enter', description: 'Open selected item', category: 'navigation', scopes: ['list'] },

  // === CONTENT NAVIGATION (markdown panels) ===
  { key: 'j/k', description: 'Scroll up/down', category: 'navigation', scopes: ['content'] },
  { key: 'g/G', description: 'Jump to top/bottom', category: 'navigation', scopes: ['content'] },
  { key: 'PgUp/PgDn', description: 'Scroll page', category: 'navigation', scopes: ['content'] },

  // === PANEL NAVIGATION ===
  { key: 'Tab', description: 'Switch panel', category: 'navigation', scopes: ['global-dashboard', 'project-context', 'barn-context', 'wiki-view', 'issues-view'] },

  // === GLOBAL SESSION SWITCHING ===
  { key: '1-9', description: 'Switch to session', category: 'action', scopes: ['global-dashboard', 'project-context'] },

  // === ROW-LEVEL ACTIONS (shown on selected rows) ===
  { key: 'c', description: 'Claude session (at path)', category: 'action', scopes: ['global-dashboard'], panel: 'projects' },
  { key: 'c', description: 'Claude session (at path)', category: 'action', scopes: ['project-context'], panel: 'livestock' },
  { key: 's', description: 'Shell into server', category: 'action', scopes: ['global-dashboard'], panel: 'barns' },
  { key: 's', description: 'Shell into server', category: 'action', scopes: ['project-context', 'barn-context'], panel: 'livestock' },

  // === CARD/PAGE-LEVEL ACTIONS ===
  { key: 'n', description: 'New (in focused panel)', category: 'action', scopes: ['global-dashboard', 'project-context', 'barn-context', 'wiki-view'] },
  { key: 'e', description: 'Edit', category: 'action', scopes: ['project-context', 'barn-context', 'wiki-view', 'livestock-detail'] },
  { key: 'd', description: 'Delete', category: 'action', scopes: ['project-context', 'barn-context', 'wiki-view'] },
  { key: 'D', description: 'Delete container', category: 'action', scopes: ['project-context', 'barn-context'] },

  // === PROJECT CONTEXT PAGE-LEVEL ===
  { key: 'w', description: 'Open wiki', category: 'action', scopes: ['project-context'] },
  { key: 'i', description: 'Open issues', category: 'action', scopes: ['project-context'] },

  // === LIVESTOCK DETAIL PAGE-LEVEL ===
  { key: 'c', description: 'Claude session (local only)', category: 'action', scopes: ['livestock-detail'] },
  { key: 's', description: 'Shell session', category: 'action', scopes: ['livestock-detail'] },
  { key: 'l', description: 'View logs', category: 'action', scopes: ['livestock-detail'] },

  // === CRITTER DETAIL PAGE-LEVEL ===
  { key: 'l', description: 'View logs', category: 'action', scopes: ['critter-detail'] },
  { key: 'e', description: 'Edit', category: 'action', scopes: ['critter-detail'] },

  // === ISSUES VIEW ===
  { key: 'r', description: 'Refresh', category: 'action', scopes: ['issues-view', 'logs-view', 'critter-logs'] },
  { key: 'o', description: 'Open in browser', category: 'action', scopes: ['issues-view'] },
  { key: 'c', description: 'Open in Claude', category: 'action', scopes: ['issues-view'] },

  // === NIGHT SKY ===
  { key: 'v', description: 'Visualizer', category: 'navigation', scopes: ['global-dashboard', 'project-context', 'livestock-detail', 'barn-context', 'critter-detail'] },
  { key: 'c', description: 'Spawn cloud', category: 'action', scopes: ['night-sky'] },
  { key: 'r', description: 'Randomize', category: 'action', scopes: ['night-sky'] },
];

/**
 * Get hotkeys for a specific view and optional panel focus
 */
export function getHotkeysForContext(
  scope: HotkeyScope,
  focusedPanel?: string,
  includeGlobal = true
): Hotkey[] {
  return HOTKEYS.filter((h) => {
    // Include if scope matches
    const scopeMatch = h.scopes.includes(scope) ||
                       (includeGlobal && h.scopes.includes('global'));

    // If hotkey has panel requirement, check it
    if (h.panel && focusedPanel && h.panel !== focusedPanel) {
      return false;
    }

    return scopeMatch;
  });
}

/**
 * Get hotkeys formatted for the help overlay (grouped by category)
 */
export function getHotkeysGrouped(scope: HotkeyScope, focusedPanel?: string): {
  navigation: Hotkey[];
  action: Hotkey[];
  system: Hotkey[];
} {
  const hotkeys = getHotkeysForContext(scope, focusedPanel);

  return {
    navigation: hotkeys.filter((h) => h.category === 'navigation'),
    action: hotkeys.filter((h) => h.category === 'action'),
    system: hotkeys.filter((h) => h.category === 'system'),
  };
}

/**
 * Format hotkeys as a single-line hint string
 * Example: "[n] new  [e] edit  [d] delete  [q] back"
 */
export function formatHotkeyHints(
  scope: HotkeyScope,
  focusedPanel?: string,
  maxKeys = 6
): string {
  const hotkeys = getHotkeysForContext(scope, focusedPanel, true);

  // Prioritize: actions first, then navigation, then system (except always include q)
  const prioritized = [
    ...hotkeys.filter((h) => h.category === 'action'),
    ...hotkeys.filter((h) => h.category === 'navigation'),
    ...hotkeys.filter((h) => h.category === 'system' && h.key !== 'q' && h.key !== '?'),
    ...hotkeys.filter((h) => h.key === 'q' || h.key === '?'),
  ];

  // Dedupe by key
  const seen = new Set<string>();
  const unique = prioritized.filter((h) => {
    if (seen.has(h.key)) return false;
    seen.add(h.key);
    return true;
  });

  return unique
    .slice(0, maxKeys)
    .map((h) => `[${h.key}] ${h.description.toLowerCase()}`)
    .join('  ');
}
