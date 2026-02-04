import { useState, useEffect, useCallback } from 'react';
import type { Barn } from '../types.js';
import { probeBarns, isCacheFresh, type BarnDetectionResult, type DetectionState } from '../lib/detection.js';
import { hasValidSshConfig, isLocalBarn } from '../lib/config.js';

export interface RemoteEnvironment {
  barn: Barn;
  state: DetectionState;
}

interface UseRemoteYeehawReturn {
  environments: RemoteEnvironment[];
  isDetecting: boolean;
  refresh: () => void;
}

export function useRemoteYeehaw(barns: Barn[]): UseRemoteYeehawReturn {
  const [results, setResults] = useState<Map<string, BarnDetectionResult>>(new Map());
  const [isDetecting, setIsDetecting] = useState(false);

  const sshBarns = barns.filter(b => !isLocalBarn(b) && hasValidSshConfig(b));

  const runDetection = useCallback(async () => {
    if (sshBarns.length === 0) return;

    setIsDetecting(true);
    try {
      const detectionResults = await probeBarns(sshBarns);
      setResults(prev => {
        const next = new Map(prev);
        for (const result of detectionResults) {
          next.set(result.barnName, result);
        }
        return next;
      });
    } finally {
      setIsDetecting(false);
    }
  }, [sshBarns.map(b => b.name).join(',')]);

  // Run detection on mount and when barns change
  useEffect(() => {
    // Check if we need to refresh any cached results
    const needsRefresh = sshBarns.some(barn => {
      const cached = results.get(barn.name);
      return !cached || !isCacheFresh(cached);
    });

    if (needsRefresh) {
      runDetection();
    }
  }, [sshBarns.map(b => b.name).join(',')]);

  // Build environments list - only include available barns
  const environments: RemoteEnvironment[] = sshBarns
    .map(barn => ({
      barn,
      state: results.get(barn.name)?.state ?? 'not-checked',
    }))
    .filter(env => env.state === 'available');

  return {
    environments,
    isDetecting,
    refresh: runDetection,
  };
}
