import React, { useState, useEffect, useRef } from 'react';
import { Box, Text, useInput } from 'ink';
import { readdirSync, existsSync } from 'fs';
import { join, dirname, basename } from 'path';
import { homedir } from 'os';
import { execaSync } from 'execa';
import type { Barn } from '../types.js';
import { hasValidSshConfig } from '../lib/config.js';

interface PathInputProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: (value: string) => void;
  barn?: Barn; // If provided, do remote completion via SSH
}

// Cache for remote directory listings: Map<"barnName:dirPath", string[]>
const remoteCompletionCache = new Map<string, string[]>();

function getCacheKey(barnName: string, dirPath: string): string {
  return `${barnName}:${dirPath}`;
}

function expandPath(path: string): string {
  if (path.startsWith('~/')) {
    return join(homedir(), path.slice(2));
  }
  return path;
}

function getLocalCompletions(partialPath: string): string[] {
  if (!partialPath) return [];

  const expanded = expandPath(partialPath);
  const dir = partialPath.endsWith('/') ? expanded : dirname(expanded);
  const prefix = partialPath.endsWith('/') ? '' : basename(expanded);

  try {
    if (!existsSync(dir)) return [];

    const entries = readdirSync(dir, { withFileTypes: true });
    const matches = entries
      .filter((e) => e.name.startsWith(prefix) && e.isDirectory())
      .map((e) => e.name);

    return matches;
  } catch {
    return [];
  }
}

function fetchRemoteDirectoryListing(dir: string, barn: Barn): string[] {
  if (!hasValidSshConfig(barn)) {
    return [];
  }

  try {
    // Fetch ALL directories in this directory (not filtered by prefix)
    // This allows us to cache the full listing and filter client-side
    const result = execaSync('ssh', [
      '-p', String(barn.port),
      '-i', barn.identity_file,
      '-o', 'BatchMode=yes',
      '-o', 'ConnectTimeout=3',
      '-o', 'StrictHostKeyChecking=accept-new',
      `${barn.user}@${barn.host}`,
      `ls -1F ${dir} 2>/dev/null | grep '/$' | sed 's|/$||' || true`
    ], { timeout: 5000 });

    const output = result.stdout.trim();
    if (!output) return [];

    return output.split('\n').filter(Boolean);
  } catch {
    return [];
  }
}

function getRemoteCompletions(partialPath: string, barn: Barn): string[] {
  if (!partialPath) return [];

  // Verify barn has valid SSH config
  if (!hasValidSshConfig(barn)) {
    return [];
  }

  // Handle ~ expansion for display
  const dir = partialPath.endsWith('/') ? partialPath.slice(0, -1) || '~' : dirname(partialPath) || '~';
  const prefix = partialPath.endsWith('/') ? '' : basename(partialPath);
  const cacheKey = getCacheKey(barn.name, dir);

  // Check cache first
  let allDirs = remoteCompletionCache.get(cacheKey);
  if (!allDirs) {
    // Fetch and cache
    allDirs = fetchRemoteDirectoryListing(dir, barn);
    remoteCompletionCache.set(cacheKey, allDirs);
  }

  // Filter by prefix
  if (prefix) {
    return allDirs.filter((d) => d.startsWith(prefix));
  }
  return allDirs;
}

// Pre-fetch a directory's contents in the background
function prefetchRemoteDirectory(dir: string, barn: Barn): void {
  if (!hasValidSshConfig(barn)) return;

  const cacheKey = getCacheKey(barn.name, dir);
  if (remoteCompletionCache.has(cacheKey)) return; // Already cached

  // Fetch asynchronously (don't block)
  setTimeout(() => {
    const dirs = fetchRemoteDirectoryListing(dir, barn);
    remoteCompletionCache.set(cacheKey, dirs);
  }, 0);
}

export function PathInput({ value, onChange, onSubmit, barn }: PathInputProps) {
  const [cursorPos, setCursorPos] = useState(value.length);
  const [completions, setCompletions] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const pendingFetch = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Update completions when value changes
  useEffect(() => {
    if (barn) {
      // Remote completion - check cache first
      const dir = value.endsWith('/') ? value.slice(0, -1) || '~' : dirname(value) || '~';
      const cacheKey = getCacheKey(barn.name, dir);
      const cached = remoteCompletionCache.has(cacheKey);

      if (cached) {
        // Use cached data immediately
        const results = getRemoteCompletions(value, barn);
        setCompletions(results);
        setLoading(false);
      } else {
        // Not cached - debounce the fetch
        setLoading(true);
        if (pendingFetch.current) {
          clearTimeout(pendingFetch.current);
        }
        pendingFetch.current = setTimeout(() => {
          const results = getRemoteCompletions(value, barn);
          setCompletions(results);
          setLoading(false);
        }, 200);
      }
      return () => {
        if (pendingFetch.current) {
          clearTimeout(pendingFetch.current);
        }
      };
    } else {
      // Local completion - immediate
      setCompletions(getLocalCompletions(value));
    }
  }, [value, barn]);

  useInput((input, key) => {
    if (key.return) {
      onSubmit(value);
      return;
    }

    if (key.tab) {
      // Tab completion
      if (completions.length === 1) {
        // Single match - complete it
        const displayDir = value.endsWith('/') ? value : value.slice(0, value.lastIndexOf('/') + 1) || (barn ? '~/' : '');
        const newPath = displayDir + completions[0] + '/';
        onChange(newPath);
        setCursorPos(newPath.length);
        // Pre-fetch the subdirectory contents for faster subsequent completions
        if (barn) {
          prefetchRemoteDirectory(newPath.slice(0, -1), barn);
        }
      } else if (completions.length > 1) {
        // Multiple matches - find common prefix
        const commonPrefix = completions.reduce((acc, curr) => {
          let i = 0;
          while (i < acc.length && i < curr.length && acc[i] === curr[i]) i++;
          return acc.slice(0, i);
        });
        if (commonPrefix) {
          const displayDir = value.endsWith('/') ? value : value.slice(0, value.lastIndexOf('/') + 1) || (barn ? '~/' : '');
          const newPath = displayDir + commonPrefix;
          onChange(newPath);
          setCursorPos(newPath.length);
        }
      }
      return;
    }

    if (key.backspace || key.delete) {
      if (value.length > 0) {
        const newValue = value.slice(0, -1);
        onChange(newValue);
        setCursorPos(Math.max(0, cursorPos - 1));
      }
      return;
    }

    if (key.leftArrow) {
      setCursorPos(Math.max(0, cursorPos - 1));
      return;
    }

    if (key.rightArrow) {
      setCursorPos(Math.min(value.length, cursorPos + 1));
      return;
    }

    // Regular character input
    if (input && !key.ctrl && !key.meta) {
      const newValue = value + input;
      onChange(newValue);
      setCursorPos(newValue.length);
    }
  });

  // Show completions hint
  const showHint = completions.length > 1 && completions.length <= 5;

  return (
    <Box flexDirection="column">
      <Box>
        <Text>{value}</Text>
        <Text backgroundColor="white" color="black"> </Text>
        {loading && <Text dimColor> (loading...)</Text>}
      </Box>
      {showHint && (
        <Box marginTop={1}>
          <Text dimColor>Tab: {completions.join('  ')}</Text>
        </Box>
      )}
    </Box>
  );
}
