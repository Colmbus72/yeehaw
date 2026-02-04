import { execaSync } from 'execa';
import { spawnSync } from 'child_process';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { writeTmuxConfig, TMUX_CONFIG_PATH } from './tmux-config.js';
import { shellEscape } from './shell.js';
import { readSignal, getStatusIcon, type WindowStatusInfo, type SessionStatus } from './signals.js';

export type { WindowStatusInfo, SessionStatus } from './signals.js';

// Get the path to the MCP server (it's in dist/, not dist/lib/)
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const MCP_SERVER_PATH = join(__dirname, '..', 'mcp-server.js');
// Get the path to the bundled Claude plugin (at package root, sibling to dist/)
const CLAUDE_PLUGIN_PATH = join(__dirname, '..', '..', 'claude-plugin');

export const YEEHAW_SESSION = 'yeehaw';

// Remote mode state tracking
let remoteWindowIndex: number | null = null;

// Keys to unbind when entering remote mode (so they pass through to inner tmux)
const REMOTE_MODE_UNBIND_KEYS = ['C-h', 'C-l', 'C-y'];

export type WindowType = 'claude' | 'shell' | 'ssh' | '';

export interface TmuxWindow {
  index: number;
  name: string;
  active: boolean;
  paneId: string;
  paneTitle: string;
  paneCurrentCommand: string;
  windowActivity: number;
  type: WindowType;
}

export function hasTmux(): boolean {
  try {
    execaSync('which', ['tmux']);
    return true;
  } catch {
    return false;
  }
}

export function isInsideYeehawSession(): boolean {
  const tmuxEnv = process.env.TMUX;
  if (!tmuxEnv) return false;

  // Check if we're in the yeehaw session
  try {
    const result = execaSync('tmux', ['display-message', '-p', '#{session_name}']);
    return result.stdout.trim() === YEEHAW_SESSION;
  } catch {
    return false;
  }
}

export function yeehawSessionExists(): boolean {
  try {
    execaSync('tmux', ['has-session', '-t', YEEHAW_SESSION]);
    return true;
  } catch {
    return false;
  }
}

export function createYeehawSession(): void {
  // Write the tmux config
  writeTmuxConfig();

  // Create the session with window 0 named "yeehaw", running yeehaw directly
  // This avoids the visible shell spawn - yeehaw runs immediately in the session
  execaSync('tmux', [
    'new-session',
    '-d',
    '-s', YEEHAW_SESSION,
    '-n', 'yeehaw',
    'yeehaw',  // Run yeehaw directly instead of spawning a shell first
  ]);

  // Source the config
  execaSync('tmux', ['source-file', TMUX_CONFIG_PATH]);

  // Set up hook to hide status bar in window 0
  setupStatusBarHooks();
}

export function setupStatusBarHooks(): void {
  // Hide status bar when in window 0, show in other windows
  const statusCheck = 'if-shell -F "#{==:#{window_index},0}" "set status off" "set status on"';

  try {
    // Start with status off (we begin in window 0)
    execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status', 'off']);

    // Hook for when window changes
    execaSync('tmux', [
      'set-hook', '-t', YEEHAW_SESSION,
      'after-select-window',
      statusCheck
    ]);

    // Hook for when a window is killed (we might land back on window 0)
    execaSync('tmux', [
      'set-hook', '-t', YEEHAW_SESSION,
      'window-unlinked',
      statusCheck
    ]);

    // Hook for when pane focus changes (covers edge cases)
    execaSync('tmux', [
      'set-hook', '-t', YEEHAW_SESSION,
      'pane-focus-in',
      statusCheck
    ]);

    // Hook for client attachment (ensure status is correct when reattaching)
    execaSync('tmux', [
      'set-hook', '-t', YEEHAW_SESSION,
      'client-attached',
      statusCheck
    ]);
  } catch {
    // Hooks might fail on older tmux versions, not critical
  }
}

/**
 * Force check and correct the status bar visibility based on current window.
 * Call this when you suspect the status bar might be in the wrong state.
 */
export function ensureCorrectStatusBar(): void {
  try {
    // Get current window index
    const result = execaSync('tmux', ['display-message', '-p', '#{window_index}']);
    const windowIndex = parseInt(result.stdout.trim(), 10);

    // Set status based on window
    if (windowIndex === 0) {
      execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status', 'off']);
    } else {
      execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status', 'on']);
    }
  } catch {
    // Ignore errors
  }
}

export function attachToYeehaw(): void {
  // Attach to the existing yeehaw session
  // Note: Multiple terminals attaching to the same session will share the same view
  // (this is a tmux limitation - they'll see the same window selections)
  spawnSync('tmux', ['attach-session', '-t', YEEHAW_SESSION], {
    stdio: 'inherit',
  });
  process.exit(0);
}

// All yeehaw MCP tools that should be auto-approved
export const YEEHAW_MCP_TOOLS = [
  // Project management
  'mcp__yeehaw__list_projects',
  'mcp__yeehaw__get_project',
  'mcp__yeehaw__create_project',
  'mcp__yeehaw__update_project',
  'mcp__yeehaw__delete_project',
  // Livestock management
  'mcp__yeehaw__add_livestock',
  'mcp__yeehaw__remove_livestock',
  'mcp__yeehaw__read_livestock_logs',
  'mcp__yeehaw__read_livestock_env',
  // Barn management
  'mcp__yeehaw__list_barns',
  'mcp__yeehaw__get_barn',
  'mcp__yeehaw__create_barn',
  'mcp__yeehaw__update_barn',
  'mcp__yeehaw__delete_barn',
  // Critter management
  'mcp__yeehaw__add_critter',
  'mcp__yeehaw__remove_critter',
  'mcp__yeehaw__read_critter_logs',
  'mcp__yeehaw__discover_critters',
  // Wiki management
  'mcp__yeehaw__get_wiki',
  'mcp__yeehaw__get_wiki_section',
  'mcp__yeehaw__add_wiki_section',
  'mcp__yeehaw__update_wiki_section',
  'mcp__yeehaw__delete_wiki_section',
];

/**
 * Set the window type option for a window (used for reliable window type detection)
 */
function setWindowType(windowIndex: number, type: WindowType): void {
  execaSync('tmux', [
    'set-option', '-w', '-t', `${YEEHAW_SESSION}:${windowIndex}`,
    '@yeehaw_type', type,
  ]);
}

export function createClaudeWindow(workingDir: string, windowName: string): number {
  // Ensure workingDir is valid, fallback to current working directory
  const effectiveWorkingDir = workingDir || process.cwd();

  // Build MCP config for yeehaw server
  const mcpConfig = JSON.stringify({
    mcpServers: {
      yeehaw: {
        command: 'node',
        args: [MCP_SERVER_PATH],
      },
    },
  });

  // Build allowed tools list for auto-approval
  const allowedTools = YEEHAW_MCP_TOOLS.join(',');

  // Create new window running claude with yeehaw MCP server (-a appends after current window)
  // Use shell escaping to safely handle special characters in JSON
  // Include the bundled plugin directory for Yeehaw-specific skills
  const claudeCmd = `claude --mcp-config ${shellEscape(mcpConfig)} --allowedTools ${shellEscape(allowedTools)} --plugin-dir ${shellEscape(CLAUDE_PLUGIN_PATH)}`;

  // Create window in background (-d) so we stay in yeehaw UI for splash screen
  // Use -P to print the window info so we can get its index
  const result = execaSync('tmux', [
    'new-window',
    '-a',
    '-d',
    '-P', '-F', '#{window_index}',
    '-t', YEEHAW_SESSION,
    '-n', windowName,
    '-c', effectiveWorkingDir,
    claudeCmd,
  ]);
  const windowIndex = parseInt(result.stdout.trim(), 10);

  // Mark this window as a Claude session
  setWindowType(windowIndex, 'claude');

  return windowIndex;
}

export function createClaudeWindowWithPrompt(
  workingDir: string,
  windowName: string,
  systemPrompt: string
): number {
  // Ensure workingDir is valid, fallback to current working directory
  const effectiveWorkingDir = workingDir || process.cwd();

  // Build MCP config for yeehaw server
  const mcpConfig = JSON.stringify({
    mcpServers: {
      yeehaw: {
        command: 'node',
        args: [MCP_SERVER_PATH],
      },
    },
  });

  // Build allowed tools list for auto-approval
  const allowedTools = YEEHAW_MCP_TOOLS.join(',');

  // Escape the system prompt for shell - use single quotes and escape any single quotes in content
  const escapedPrompt = systemPrompt.replace(/'/g, "'\\''");

  // Create new window running claude with yeehaw MCP server and system prompt
  const claudeCmd = `claude --mcp-config ${shellEscape(mcpConfig)} --allowedTools ${shellEscape(allowedTools)} --plugin-dir ${shellEscape(CLAUDE_PLUGIN_PATH)} --system-prompt '${escapedPrompt}'`;

  // Create window in background (-d) so we stay in yeehaw UI for splash screen
  // Use -P to print the window info so we can get its index
  const result = execaSync('tmux', [
    'new-window',
    '-a',
    '-d',
    '-P', '-F', '#{window_index}',
    '-t', YEEHAW_SESSION,
    '-n', windowName,
    '-c', effectiveWorkingDir,
    claudeCmd,
  ]);
  const windowIndex = parseInt(result.stdout.trim(), 10);

  // Mark this window as a Claude session
  setWindowType(windowIndex, 'claude');

  return windowIndex;
}

export function createShellWindow(workingDir: string, windowName: string, shell?: string): number {
  // Use the user's configured shell from $SHELL, fallback to /bin/bash
  const userShell = shell || process.env.SHELL || '/bin/bash';

  // Create new window running shell as a login shell (-l) so it loads .bashrc/.bash_profile/.zshrc etc.
  // This ensures PS1 and other environment customizations are loaded
  execaSync('tmux', [
    'new-window',
    '-a',
    '-t', YEEHAW_SESSION,
    '-n', windowName,
    '-c', workingDir,
    `${userShell} -l`,
  ]);

  // Get the window index we just created (new window is now current)
  const result = execaSync('tmux', [
    'display-message', '-p', '#{window_index}'
  ]);
  const windowIndex = parseInt(result.stdout.trim(), 10);

  // Mark this window as a shell session
  setWindowType(windowIndex, 'shell');

  return windowIndex;
}

export function createSshWindow(
  windowName: string,
  host: string,
  user: string,
  port: number,
  identityFile: string,
  remotePath: string
): number {
  // Two levels of escaping needed:
  // 1. Remote shell sees: cd /path && exec $SHELL -l
  // 2. Local shell (tmux) sees: ssh ... -t 'cd /path && ...'

  // Build the remote command - escape remotePath for the remote shell
  const remoteCmd = `cd ${shellEscape(remotePath)} && exec $SHELL -l`;

  // Build SSH command parts - escape for local shell (tmux passes to sh)
  const sshParts = [
    'ssh',
    '-p', String(port),
    '-i', shellEscape(identityFile),
    shellEscape(`${user}@${host}`),
    '-t',
    shellEscape(remoteCmd)  // Double-escaped: once for remote, once for local
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

  // Mark this window as an SSH session
  setWindowType(windowIndex, 'ssh');

  return windowIndex;
}

export function detachFromSession(): void {
  execaSync('tmux', ['detach-client']);
}

export function killYeehawSession(): void {
  try {
    execaSync('tmux', ['kill-session', '-t', YEEHAW_SESSION]);
  } catch {
    // Session might already be dead
  }
}

export function restartYeehaw(): void {
  // Respawn window 0 with a fresh yeehaw process
  // This kills the current process but preserves all other windows
  execaSync('tmux', ['respawn-window', '-k', '-t', `${YEEHAW_SESSION}:0`, 'yeehaw']);
}

export function switchToWindow(windowIndex: number): void {
  execaSync('tmux', ['select-window', '-t', `${YEEHAW_SESSION}:${windowIndex}`]);
}

export function listYeehawWindows(): TmuxWindow[] {
  try {
    // Use tab as delimiter since pane_title can contain colons
    // Include @yeehaw_type window option for reliable window type detection
    const result = execaSync('tmux', [
      'list-windows',
      '-t', YEEHAW_SESSION,
      '-F', '#{window_index}\t#{window_name}\t#{window_active}\t#{pane_id}\t#{pane_title}\t#{pane_current_command}\t#{window_activity}\t#{@yeehaw_type}',
    ]);

    return result.stdout
      .split('\n')
      .filter(Boolean)
      .map((line) => {
        const [index, name, active, paneId, paneTitle, paneCurrentCommand, windowActivity, type] = line.split('\t');
        return {
          index: parseInt(index, 10),
          name,
          active: active === '1',
          paneId: paneId || '',
          paneTitle: paneTitle || '',
          paneCurrentCommand: paneCurrentCommand || '',
          windowActivity: parseInt(windowActivity, 10) || 0,
          type: (type || '') as WindowType,
        };
      });
  } catch {
    return [];
  }
}

export function killWindow(windowIndex: number): void {
  try {
    execaSync('tmux', ['kill-window', '-t', `${YEEHAW_SESSION}:${windowIndex}`]);
  } catch {
    // Window might already be dead
  }
}

/**
 * Format relative time from a Unix timestamp
 */
function formatRelativeTime(timestamp: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - timestamp;

  if (diff < 60) return 'now';
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86400)}d`;
}

/**
 * Check if a pane title indicates Claude is actively working (has spinner)
 */
function isClaudeWorking(paneTitle: string): boolean {
  // Braille spinner characters used by Claude Code
  const spinnerChars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '⠂', '⠐', '⠈'];
  return spinnerChars.some(char => paneTitle.startsWith(char));
}

/**
 * Get formatted status info for a tmux window
 */
export function getWindowStatus(window: TmuxWindow): WindowStatusInfo {
  const isClaudeSession = window.type === 'claude';
  const relativeTime = formatRelativeTime(window.windowActivity);

  // Check for signal file first (written by Claude hooks)
  if (isClaudeSession && window.paneId) {
    const signal = readSignal(window.paneId);
    if (signal) {
      const icon = getStatusIcon(signal.status);
      const text = signal.status === 'waiting'
        ? 'Waiting for input'
        : signal.status === 'working'
        ? window.paneTitle || 'Working...'
        : signal.status === 'error'
        ? 'Error'
        : `idle ${relativeTime}`;
      return { text: `${icon} ${text}`, status: signal.status, icon };
    }
  }

  // Fallback to tmux-native detection for Claude sessions
  if (isClaudeSession) {
    if (window.paneTitle) {
      const working = isClaudeWorking(window.paneTitle);
      if (working) {
        return {
          text: window.paneTitle,
          status: 'working',
          icon: getStatusIcon('working'),
        };
      }
      // Not working - likely idle
      const text = relativeTime !== 'now' && relativeTime !== '1m'
        ? `${window.paneTitle} (${relativeTime})`
        : window.paneTitle;
      return { text, status: 'idle', icon: getStatusIcon('idle') };
    }
    const text = relativeTime === 'now' ? 'active' : `idle ${relativeTime}`;
    return { text: `○ ${text}`, status: 'idle', icon: '○' };
  }

  // For shell sessions, check if pane is dead
  if (window.paneCurrentCommand === '') {
    return { text: '✖ disconnected', status: 'error', icon: '✖' };
  }

  // For shell sessions, show current command
  const cmd = window.paneCurrentCommand;
  if (cmd && cmd !== 'zsh' && cmd !== 'bash' && cmd !== 'sh' && cmd !== 'fish') {
    return { text: cmd, status: 'working', icon: getStatusIcon('working') };
  }

  // At shell prompt - show idle time
  const text = relativeTime === 'now' ? 'ready' : `idle ${relativeTime}`;
  return { text: `○ ${text}`, status: 'idle', icon: '○' };
}

export function updateStatusBar(projectName?: string): void {
  const left = projectName
    ? `#[bold] YEEHAW | ${projectName} `
    : '#[bold] YEEHAW ';

  try {
    execaSync('tmux', ['set', '-t', YEEHAW_SESSION, 'status-left', left]);
  } catch {
    // Not critical if this fails
  }
}

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
