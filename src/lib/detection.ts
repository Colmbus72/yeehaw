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
