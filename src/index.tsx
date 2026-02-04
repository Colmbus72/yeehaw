#!/usr/bin/env node
import React from 'react';
import { render } from 'ink';
import { App } from './app.js';
import {
  isInsideYeehawSession,
  yeehawSessionExists,
  createYeehawSession,
  attachToYeehaw,
  hasTmux,
} from './lib/tmux.js';
import { ensureConfigDirs } from './lib/config.js';
import { checkForUpdates, formatUpdateMessage } from './lib/update-check.js';
import { installHookScript, getClaudeHooksConfig, checkClaudeHooksInstalled } from './lib/hooks.js';

/**
 * Handle CLI subcommands
 */
function handleSubcommands(): boolean {
  const args = process.argv.slice(2);

  if (args[0] === 'hooks' && args[1] === 'install') {
    const scriptPath = installHookScript();
    console.log(`\x1b[32m✓\x1b[0m Hook script installed: ${scriptPath}`);
    console.log('');
    console.log('\x1b[33mNote:\x1b[0m Claude sessions started from Yeehaw already have hooks enabled.');
    console.log('This command is only needed for Claude sessions started outside Yeehaw.');

    if (checkClaudeHooksInstalled()) {
      console.log('\n\x1b[32m✓\x1b[0m Claude hooks already configured in ~/.claude/settings.json');
    } else {
      console.log('\nTo enable status tracking for external Claude sessions,');
      console.log('add this to ~/.claude/settings.json:');
      console.log(JSON.stringify(getClaudeHooksConfig(), null, 2));
    }
    return true;
  }

  if (args[0] === 'hooks') {
    console.log('Usage: yeehaw hooks install');
    console.log('');
    console.log('Install Claude hooks for session status tracking.');
    console.log('Note: Sessions started from Yeehaw already have hooks enabled automatically.');
    console.log('This is only needed for Claude sessions started outside Yeehaw.');
    return true;
  }

  return false;
}

function main() {
  // Handle subcommands first (before tmux checks)
  if (handleSubcommands()) {
    process.exit(0);
  }
  // Ensure config directories exist
  ensureConfigDirs();

  // Check for updates (non-blocking, uses cache)
  try {
    const updateInfo = checkForUpdates();
    if (updateInfo?.updateAvailable) {
      console.log('\x1b[33m%s\x1b[0m', formatUpdateMessage(updateInfo));
      console.log('');
    }
  } catch {
    // Ignore update check errors
  }

  // Check if tmux is available
  if (!hasTmux()) {
    console.error('Error: tmux is required but not installed');
    console.error('Install tmux and try again');
    process.exit(1);
  }

  // If we're already inside the yeehaw tmux session, just render the TUI
  if (isInsideYeehawSession()) {
    render(<App />, {
      patchConsole: true,
      incrementalRendering: true,
      // maxFps: 60,
    });
    return;
  }

  // We're not inside yeehaw session - need to create/attach
  if (!yeehawSessionExists()) {
    // Create new session with yeehaw running directly in window 0
    // (no shell intermediary - cleaner startup)
    createYeehawSession();
  }

  // Attach to the yeehaw session
  // This will exec into tmux and not return
  attachToYeehaw();
}

main();
