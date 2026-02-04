import { readFileSync, existsSync } from 'fs';
import { homedir } from 'os';
import { join } from 'path';

export interface SshHost {
  name: string;
  hostname?: string;
  user?: string;
  port?: number;
  identityFile?: string;
}

/**
 * Parse ~/.ssh/config to find configured hosts
 */
export function parseSshConfig(): SshHost[] {
  const configPath = join(homedir(), '.ssh', 'config');

  if (!existsSync(configPath)) {
    return [];
  }

  try {
    const content = readFileSync(configPath, 'utf-8');
    const lines = content.split('\n');
    const hosts: SshHost[] = [];
    let currentHost: SshHost | null = null;

    for (const line of lines) {
      const trimmed = line.trim();

      // Skip comments and empty lines
      if (trimmed.startsWith('#') || trimmed === '') {
        continue;
      }

      // Parse key-value pairs
      const match = trimmed.match(/^(\S+)\s+(.+)$/);
      if (!match) continue;

      const [, key, value] = match;
      const keyLower = key.toLowerCase();

      if (keyLower === 'host') {
        // Skip wildcard hosts
        if (value.includes('*')) {
          currentHost = null;
          continue;
        }

        // Save previous host if exists
        if (currentHost) {
          hosts.push(currentHost);
        }

        currentHost = { name: value };
      } else if (currentHost) {
        switch (keyLower) {
          case 'hostname':
            currentHost.hostname = value;
            break;
          case 'user':
            currentHost.user = value;
            break;
          case 'port':
            currentHost.port = parseInt(value, 10);
            break;
          case 'identityfile':
            // Expand ~ in path
            currentHost.identityFile = value.startsWith('~/')
              ? join(homedir(), value.slice(2))
              : value;
            break;
        }
      }
    }

    // Don't forget the last host
    if (currentHost) {
      hosts.push(currentHost);
    }

    // Filter out hosts without hostname (they're just aliases)
    return hosts.filter((h) => h.hostname || h.name);
  } catch {
    return [];
  }
}

/**
 * Get a list of SSH host names for quick selection
 */
export function getSshHostNames(): string[] {
  return parseSshConfig().map((h) => h.name);
}
