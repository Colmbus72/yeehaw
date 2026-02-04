import { homedir } from 'os';
import { join } from 'path';

export const YEEHAW_DIR = join(homedir(), '.yeehaw');
export const CONFIG_FILE = join(YEEHAW_DIR, 'config.yaml');
export const AUTH_FILE = join(YEEHAW_DIR, 'auth.yaml');
export const PROJECTS_DIR = join(YEEHAW_DIR, 'projects');
export const BARNS_DIR = join(YEEHAW_DIR, 'barns');
export const RANCHHANDS_DIR = join(YEEHAW_DIR, 'ranchhands');
export const SESSIONS_DIR = join(YEEHAW_DIR, 'sessions');
export const SIGNALS_DIR = join(YEEHAW_DIR, 'session-signals');
export const HOOKS_DIR = join(YEEHAW_DIR, 'bin');

/**
 * Validate a name to prevent path traversal attacks.
 * Rejects names containing path separators or parent directory references.
 */
function validateName(name: string, type: string): void {
  if (name.includes('/') || name.includes('\\') || name.includes('..') || name.includes('\0')) {
    throw new Error(`Invalid ${type} name: contains forbidden characters`);
  }
}

export function getProjectPath(name: string): string {
  validateName(name, 'project');
  return join(PROJECTS_DIR, `${name}.yaml`);
}

export function getBarnPath(name: string): string {
  validateName(name, 'barn');
  return join(BARNS_DIR, `${name}.yaml`);
}

export function getSessionPath(id: string): string {
  validateName(id, 'session');
  return join(SESSIONS_DIR, `${id}.yaml`);
}

export function getRanchHandPath(name: string): string {
  validateName(name, 'ranchhand');
  return join(RANCHHANDS_DIR, `${name}.yaml`);
}
