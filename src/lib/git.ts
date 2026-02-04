import { execaSync } from 'execa';
import { existsSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';
import type { Barn } from '../types.js';
import { hasValidSshConfig } from './config.js';
import { shellEscape } from './shell.js';

function expandPath(path: string): string {
  if (path.startsWith('~/')) {
    return join(homedir(), path.slice(2));
  }
  return path;
}

export interface GitInfo {
  isGitRepo: boolean;
  remoteUrl?: string;
  branch?: string;
}

export function detectGitInfo(path: string): GitInfo {
  const expandedPath = expandPath(path);

  // Check if .git exists
  if (!existsSync(join(expandedPath, '.git'))) {
    return { isGitRepo: false };
  }

  try {
    // Get remote URL
    const remoteResult = execaSync('git', ['-C', expandedPath, 'config', '--get', 'remote.origin.url'], {
      reject: false,
    });
    const remoteUrl = remoteResult.exitCode === 0 ? remoteResult.stdout.trim() : undefined;

    // Get current branch
    const branchResult = execaSync('git', ['-C', expandedPath, 'rev-parse', '--abbrev-ref', 'HEAD'], {
      reject: false,
    });
    const branch = branchResult.exitCode === 0 ? branchResult.stdout.trim() : undefined;

    return {
      isGitRepo: true,
      remoteUrl,
      branch,
    };
  } catch {
    return { isGitRepo: false };
  }
}

/**
 * Detect git info on a remote server via SSH.
 */
export function detectRemoteGitInfo(path: string, barn: Barn): GitInfo {
  // Verify barn has valid SSH config
  if (!hasValidSshConfig(barn)) {
    return { isGitRepo: false };
  }

  try {
    // Run git commands via SSH
    const result = execaSync('ssh', [
      '-p', String(barn.port),
      '-i', barn.identity_file,
      '-o', 'BatchMode=yes',
      '-o', 'ConnectTimeout=5',
      `${barn.user}@${barn.host}`,
      `cd ${shellEscape(path)} && git config --get remote.origin.url 2>/dev/null && git rev-parse --abbrev-ref HEAD 2>/dev/null`
    ], { timeout: 10000, reject: false });

    if (result.exitCode !== 0) {
      return { isGitRepo: false };
    }

    const lines = result.stdout.trim().split('\n');
    const remoteUrl = lines[0] || undefined;
    const branch = lines[1] || undefined;

    return {
      isGitRepo: true,
      remoteUrl,
      branch,
    };
  } catch {
    return { isGitRepo: false };
  }
}
