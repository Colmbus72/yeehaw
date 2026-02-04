import { existsSync, readFileSync, readdirSync, unlinkSync, mkdirSync } from 'fs';
import { join } from 'path';
import { SIGNALS_DIR } from './paths.js';

export type SessionStatus = 'working' | 'waiting' | 'idle' | 'error';

export interface SessionSignal {
  status: SessionStatus;
  updated: number;
}

export interface WindowStatusInfo {
  text: string;
  status: SessionStatus;
  icon: string;
}

const STATUS_ICONS: Record<SessionStatus, string> = {
  working: '⠿',
  waiting: '◆',
  idle: '○',
  error: '✖',
};

const SIGNAL_MAX_AGE_MS = 5 * 60 * 1000; // 5 minutes

/**
 * Sanitize pane ID to create a safe filename
 */
function sanitizePaneId(paneId: string): string {
  return paneId.replace(/[^a-zA-Z0-9]/g, '_');
}

/**
 * Read signal file for a tmux pane
 */
export function readSignal(paneId: string): SessionSignal | null {
  if (!existsSync(SIGNALS_DIR)) return null;

  const filename = `${sanitizePaneId(paneId)}.json`;
  const filepath = join(SIGNALS_DIR, filename);

  if (!existsSync(filepath)) return null;

  try {
    const content = readFileSync(filepath, 'utf-8');
    const signal = JSON.parse(content) as SessionSignal;

    // Check if signal is stale
    const ageMs = Date.now() - signal.updated * 1000;
    if (ageMs > SIGNAL_MAX_AGE_MS) {
      return null;
    }

    return signal;
  } catch {
    return null;
  }
}

/**
 * Get status icon for a session status
 */
export function getStatusIcon(status: SessionStatus): string {
  return STATUS_ICONS[status];
}

/**
 * Ensure the signals directory exists
 */
export function ensureSignalsDir(): void {
  if (!existsSync(SIGNALS_DIR)) {
    mkdirSync(SIGNALS_DIR, { recursive: true });
  }
}

/**
 * Clean up signal file for a pane
 */
export function cleanupSignal(paneId: string): void {
  const filename = `${sanitizePaneId(paneId)}.json`;
  const filepath = join(SIGNALS_DIR, filename);

  try {
    if (existsSync(filepath)) {
      unlinkSync(filepath);
    }
  } catch {
    // Ignore cleanup errors
  }
}

/**
 * Clean up all stale signal files (older than 1 hour)
 */
export function cleanupStaleSignals(): void {
  if (!existsSync(SIGNALS_DIR)) return;

  const STALE_AGE_MS = 60 * 60 * 1000; // 1 hour
  const now = Date.now();

  try {
    const files = readdirSync(SIGNALS_DIR).filter(f => f.endsWith('.json'));
    for (const file of files) {
      const filepath = join(SIGNALS_DIR, file);
      try {
        const content = readFileSync(filepath, 'utf-8');
        const signal = JSON.parse(content) as SessionSignal;
        const ageMs = now - signal.updated * 1000;
        if (ageMs > STALE_AGE_MS) {
          unlinkSync(filepath);
        }
      } catch {
        // Delete malformed files
        try {
          unlinkSync(filepath);
        } catch {
          // Ignore
        }
      }
    }
  } catch {
    // Ignore errors
  }
}
