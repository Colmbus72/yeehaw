import { execFileSync } from 'child_process';
import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'fs';
import { join } from 'path';
import { YEEHAW_DIR } from './paths.js';

const PACKAGE_NAME = '@colmbus72/yeehaw';
const CACHE_FILE = join(YEEHAW_DIR, '.update-check');
const CACHE_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours

interface CacheData {
  latestVersion: string;
  checkedAt: number;
}

function getCurrentVersion(): string {
  try {
    const packagePath = new URL('../../package.json', import.meta.url);
    const pkg = JSON.parse(readFileSync(packagePath, 'utf-8'));
    return pkg.version;
  } catch {
    return '0.0.0';
  }
}

function readCache(): CacheData | null {
  try {
    if (!existsSync(CACHE_FILE)) return null;
    const data = JSON.parse(readFileSync(CACHE_FILE, 'utf-8'));
    if (Date.now() - data.checkedAt < CACHE_TTL_MS) {
      return data;
    }
    return null; // Cache expired
  } catch {
    return null;
  }
}

function writeCache(latestVersion: string): void {
  try {
    if (!existsSync(YEEHAW_DIR)) {
      mkdirSync(YEEHAW_DIR, { recursive: true });
    }
    const data: CacheData = {
      latestVersion,
      checkedAt: Date.now(),
    };
    writeFileSync(CACHE_FILE, JSON.stringify(data), 'utf-8');
  } catch {
    // Ignore cache write errors
  }
}

function fetchLatestVersion(): string | null {
  try {
    // Use execFileSync for safety (no shell injection possible)
    const result = execFileSync('npm', ['view', PACKAGE_NAME, 'version'], {
      encoding: 'utf-8',
      timeout: 5000,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    return result.trim();
  } catch {
    return null;
  }
}

function compareVersions(current: string, latest: string): number {
  const parseVersion = (v: string) => v.split('.').map(n => parseInt(n, 10) || 0);
  const [cMajor, cMinor, cPatch] = parseVersion(current);
  const [lMajor, lMinor, lPatch] = parseVersion(latest);

  if (lMajor > cMajor) return 1;
  if (lMajor < cMajor) return -1;
  if (lMinor > cMinor) return 1;
  if (lMinor < cMinor) return -1;
  if (lPatch > cPatch) return 1;
  if (lPatch < cPatch) return -1;
  return 0;
}

export interface UpdateInfo {
  updateAvailable: boolean;
  currentVersion: string;
  latestVersion: string;
}

/**
 * Get version information synchronously using cached data.
 * Safe to call from React components.
 */
export function getVersionInfo(): { current: string; latest: string | null } {
  const current = getCurrentVersion();
  const cached = readCache();
  return {
    current,
    latest: cached?.latestVersion || null,
  };
}

/**
 * Check for updates. Returns immediately with cached data if available,
 * otherwise fetches from npm (with 5s timeout).
 */
export function checkForUpdates(): UpdateInfo | null {
  try {
    const currentVersion = getCurrentVersion();

    // Try cache first
    const cached = readCache();
    if (cached) {
      return {
        updateAvailable: compareVersions(currentVersion, cached.latestVersion) > 0,
        currentVersion,
        latestVersion: cached.latestVersion,
      };
    }

    // Fetch from npm
    const latestVersion = fetchLatestVersion();
    if (!latestVersion) return null;

    // Update cache
    writeCache(latestVersion);

    return {
      updateAvailable: compareVersions(currentVersion, latestVersion) > 0,
      currentVersion,
      latestVersion,
    };
  } catch {
    return null;
  }
}

/**
 * Format update notification message
 */
export function formatUpdateMessage(info: UpdateInfo): string {
  return `Update available: ${info.currentVersion} → ${info.latestVersion}\nRun: npm install -g ${PACKAGE_NAME}`;
}
