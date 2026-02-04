import { useState, useEffect, useCallback } from 'react';
import {
  listYeehawWindows,
  createClaudeWindow,
  createShellWindow,
  switchToWindow,
  type TmuxWindow,
} from '../lib/tmux.js';

interface UseSessionsReturn {
  windows: TmuxWindow[];
  loading: boolean;
  refresh: () => void;
  createClaude: (workingDir: string, name: string) => number;
  createShell: (workingDir: string, name: string) => number;
  attachToWindow: (index: number) => void;
}

export function useSessions(): UseSessionsReturn {
  const [windows, setWindows] = useState<TmuxWindow[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(() => {
    const result = listYeehawWindows();
    setWindows(prev => {
      // Only update if windows actually changed (avoids unnecessary re-renders)
      if (prev.length !== result.length) return result;
      const changed = result.some((w, i) =>
        w.index !== prev[i].index || w.name !== prev[i].name
      );
      return changed ? result : prev;
    });
  }, []);

  useEffect(() => {
    // Initial load with loading state
    const result = listYeehawWindows();
    setWindows(result);
    setLoading(false);

    // Poll for window updates (without loading state changes)
    const interval = setInterval(refresh, 800);
    return () => clearInterval(interval);
  }, [refresh]);

  const createClaude = useCallback((workingDir: string, name: string) => {
    const windowIndex = createClaudeWindow(workingDir, name);
    refresh();
    return windowIndex;
  }, [refresh]);

  const createShell = useCallback((workingDir: string, name: string) => {
    const windowIndex = createShellWindow(workingDir, name);
    refresh();
    return windowIndex;
  }, [refresh]);

  const attachToWindow = useCallback((index: number) => {
    switchToWindow(index);
  }, []);

  return {
    windows,
    loading,
    refresh,
    createClaude,
    createShell,
    attachToWindow,
  };
}
