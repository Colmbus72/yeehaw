import { existsSync, mkdirSync, writeFileSync, chmodSync, readFileSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import { HOOKS_DIR, SIGNALS_DIR } from './paths.js';

const HOOK_SCRIPT_NAME = 'claude-hook';

const HOOK_SCRIPT_CONTENT = `#!/bin/bash
# Yeehaw Claude Hook - writes session status for the CLI to read
STATUS="$1"
PANE_ID="\${TMUX_PANE:-unknown}"
SIGNAL_DIR="$HOME/.yeehaw/session-signals"
SIGNAL_FILE="$SIGNAL_DIR/\${PANE_ID//[^a-zA-Z0-9]/_}.json"

mkdir -p "$SIGNAL_DIR"
cat > "$SIGNAL_FILE" << EOF
{"status":"$STATUS","updated":$(date +%s)}
EOF
`;

/**
 * Install the Claude hook script to ~/.yeehaw/bin/
 */
export function installHookScript(): string {
  // Ensure directories exist
  if (!existsSync(HOOKS_DIR)) {
    mkdirSync(HOOKS_DIR, { recursive: true });
  }
  if (!existsSync(SIGNALS_DIR)) {
    mkdirSync(SIGNALS_DIR, { recursive: true });
  }

  const scriptPath = join(HOOKS_DIR, HOOK_SCRIPT_NAME);
  writeFileSync(scriptPath, HOOK_SCRIPT_CONTENT, 'utf-8');
  chmodSync(scriptPath, 0o755);

  return scriptPath;
}

/**
 * Get the path to the hook script
 */
export function getHookScriptPath(): string {
  return join(HOOKS_DIR, HOOK_SCRIPT_NAME);
}

/**
 * Get the Claude settings.json hooks configuration
 */
export function getClaudeHooksConfig(): object {
  const hookPath = join(HOOKS_DIR, HOOK_SCRIPT_NAME);

  return {
    hooks: {
      PreToolUse: [
        {
          matcher: '*',
          hooks: [`${hookPath} working`],
        },
      ],
      Stop: [
        {
          matcher: '*',
          hooks: [`${hookPath} waiting`],
        },
      ],
      Notification: [
        {
          matcher: 'idle_prompt',
          hooks: [`${hookPath} waiting`],
        },
      ],
    },
  };
}

/**
 * Check if Claude hooks are already configured
 */
export function checkClaudeHooksInstalled(): boolean {
  const claudeSettingsPath = join(homedir(), '.claude', 'settings.json');

  if (!existsSync(claudeSettingsPath)) {
    return false;
  }

  try {
    const content = readFileSync(claudeSettingsPath, 'utf-8');
    const settings = JSON.parse(content);
    return settings.hooks?.PreToolUse?.some((h: { hooks?: string[] }) =>
      h.hooks?.some((cmd: string) => cmd.includes('yeehaw'))
    );
  } catch {
    return false;
  }
}

/**
 * Check if hook script exists
 */
export function hookScriptExists(): boolean {
  const scriptPath = join(HOOKS_DIR, HOOK_SCRIPT_NAME);
  return existsSync(scriptPath);
}
