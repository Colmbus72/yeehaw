import { join, isAbsolute } from 'path';
import { existsSync, readFileSync } from 'fs';
import { execa } from 'execa';
import type { Livestock, Barn } from '../types.js';
import { loadBarn } from './config.js';
import { shellEscape } from './shell.js';
import { getErrorMessage } from './errors.js';

/**
 * Resolve a relative path within a livestock to an absolute path.
 * For local livestock, returns the local absolute path.
 * For remote livestock, returns the remote absolute path (for use with SSH).
 */
export function resolveLivestockPath(livestock: Livestock, relativePath: string): string {
  if (isAbsolute(relativePath)) {
    return relativePath;
  }
  return join(livestock.path, relativePath);
}

/**
 * Build SSH command prefix for a barn
 */
export function buildSshCommand(barn: Barn): string[] {
  const args = ['ssh'];
  if (barn.port && barn.port !== 22) {
    args.push('-p', String(barn.port));
  }
  if (barn.identity_file) {
    args.push('-i', barn.identity_file);
  }
  args.push(`${barn.user}@${barn.host}`);
  return args;
}

/**
 * Read a file from livestock (local or remote via SSH)
 */
export async function readLivestockFile(
  livestock: Livestock,
  relativePath: string
): Promise<{ content: string; error?: string }> {
  const fullPath = resolveLivestockPath(livestock, relativePath);

  // Local livestock
  if (!livestock.barn) {
    try {
      if (!existsSync(fullPath)) {
        return { content: '', error: `File not found: ${fullPath}` };
      }
      const content = readFileSync(fullPath, 'utf-8');
      return { content };
    } catch (err) {
      return { content: '', error: `Failed to read file: ${err}` };
    }
  }

  // Remote livestock - SSH to barn
  const barn = loadBarn(livestock.barn);
  if (!barn) {
    return { content: '', error: `Barn not found: ${livestock.barn}` };
  }
  if (!barn.host || !barn.user) {
    return { content: '', error: `Barn '${barn.name}' is not configured for SSH` };
  }

  try {
    const sshArgs = buildSshCommand(barn);
    // Use shellEscape for the path to prevent injection
    const result = await execa(sshArgs[0], [...sshArgs.slice(1), `cat ${shellEscape(fullPath)}`]);
    return { content: result.stdout };
  } catch (err) {
    return { content: '', error: `SSH error: ${getErrorMessage(err)}` };
  }
}

/**
 * Read log files from livestock with filtering options
 */
export async function readLivestockLogs(
  livestock: Livestock,
  options: {
    lines?: number;
    pattern?: string;
  } = {}
): Promise<{ content: string; error?: string }> {
  if (!livestock.log_path) {
    return { content: '', error: 'log_path not configured for this livestock' };
  }

  const { lines = 100, pattern } = options;
  const logPath = resolveLivestockPath(livestock, livestock.log_path);

  // Build the command with proper escaping
  let cmd: string;
  const escapedLogPath = shellEscape(logPath);
  const escapedLines = String(lines); // lines is a number, safe

  if (pattern) {
    // Escape the grep pattern for shell
    const escapedPattern = shellEscape(pattern);
    // grep with tail
    cmd = `find ${escapedLogPath} -name '*.log' -type f 2>/dev/null | xargs tail -n ${escapedLines} 2>/dev/null | grep -i ${escapedPattern} || true`;
  } else {
    // Just tail the logs
    cmd = `find ${escapedLogPath} -name '*.log' -type f 2>/dev/null | xargs tail -n ${escapedLines} 2>/dev/null || true`;
  }

  // Local livestock
  if (!livestock.barn) {
    try {
      const result = await execa('sh', ['-c', cmd]);
      if (!result.stdout.trim()) {
        return { content: '', error: `No log files found in ${logPath}` };
      }
      return { content: result.stdout };
    } catch (err) {
      return { content: '', error: `Failed to read logs: ${getErrorMessage(err)}` };
    }
  }

  // Remote livestock - SSH
  const barn = loadBarn(livestock.barn);
  if (!barn) {
    return { content: '', error: `Barn not found: ${livestock.barn}` };
  }
  if (!barn.host || !barn.user) {
    return { content: '', error: `Barn '${barn.name}' is not configured for SSH` };
  }

  try {
    const sshArgs = buildSshCommand(barn);
    const result = await execa(sshArgs[0], [...sshArgs.slice(1), cmd]);
    if (!result.stdout.trim()) {
      return { content: '', error: `No log files found in ${logPath}` };
    }
    return { content: result.stdout };
  } catch (err) {
    return { content: '', error: `SSH error: ${getErrorMessage(err)}` };
  }
}

/**
 * Parse .env file content into key-value pairs
 */
export function parseEnvFile(content: string): Record<string, string> {
  const result: Record<string, string> = {};
  const lines = content.split('\n');

  for (const line of lines) {
    const trimmed = line.trim();
    // Skip comments and empty lines
    if (!trimmed || trimmed.startsWith('#')) continue;

    const eqIndex = trimmed.indexOf('=');
    if (eqIndex === -1) continue;

    const key = trimmed.slice(0, eqIndex).trim();
    let value = trimmed.slice(eqIndex + 1).trim();

    // Remove surrounding quotes
    if ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }

    result[key] = value;
  }

  return result;
}

/**
 * Read env file from livestock, optionally hiding values
 */
export async function readLivestockEnv(
  livestock: Livestock,
  showValues: boolean = false
): Promise<{ content: string; error?: string }> {
  if (!livestock.env_path) {
    return { content: '', error: 'env_path not configured for this livestock' };
  }

  const result = await readLivestockFile(livestock, livestock.env_path);
  if (result.error) {
    return result;
  }

  if (showValues) {
    return result;
  }

  // Parse and return keys only
  const parsed = parseEnvFile(result.content);
  const keysOnly = Object.keys(parsed)
    .map(key => `${key}=<hidden>`)
    .join('\n');

  return { content: keysOnly };
}

/**
 * Detected framework configuration suggestions
 */
export interface DetectedConfig {
  framework?: 'laravel' | 'django' | 'rails' | 'node' | 'unknown';
  log_path?: string;
  env_path?: string;
}

/**
 * Check if a file exists (local or remote via SSH)
 */
async function fileExists(path: string, fullPath: string, barn?: Barn): Promise<boolean> {
  if (!barn) {
    // Local check
    return existsSync(join(path, fullPath));
  }

  // Remote check via SSH
  if (!barn.host || !barn.user) {
    return false;
  }

  try {
    const sshArgs = buildSshCommand(barn);
    const testPath = join(path, fullPath);
    // Use shellEscape for the path
    await execa(sshArgs[0], [...sshArgs.slice(1), `test -e ${shellEscape(testPath)}`]);
    return true;
  } catch {
    return false;
  }
}

/**
 * Detect framework and suggest log_path/env_path based on project structure
 */
export async function detectLivestockConfig(
  path: string,
  barn?: Barn
): Promise<DetectedConfig> {
  const result: DetectedConfig = {};

  // Laravel: artisan file
  if (await fileExists(path, 'artisan', barn)) {
    result.framework = 'laravel';
    result.log_path = 'storage/logs/';
    result.env_path = '.env';
    return result;
  }

  // Django: manage.py
  if (await fileExists(path, 'manage.py', barn)) {
    result.framework = 'django';
    result.log_path = 'logs/';
    result.env_path = '.env';
    return result;
  }

  // Rails: Gemfile + bin/rails
  if (await fileExists(path, 'Gemfile', barn) && await fileExists(path, 'bin/rails', barn)) {
    result.framework = 'rails';
    result.log_path = 'log/';
    result.env_path = '.env';
    return result;
  }

  // Node: package.json
  if (await fileExists(path, 'package.json', barn)) {
    result.framework = 'node';
    result.log_path = 'logs/';
    result.env_path = '.env';
    return result;
  }

  // Generic fallback: check if .env exists
  if (await fileExists(path, '.env', barn)) {
    result.framework = 'unknown';
    result.env_path = '.env';
    return result;
  }

  return result;
}
