import React, { useState, useEffect } from 'react';
import { Box, Text, useInput, useStdout } from 'ink';
import { CritterHeader } from '../components/CritterHeader.js';
import { readCritterLogs } from '../lib/critters.js';
import { loadBarn } from '../lib/config.js';
import type { Barn, Critter } from '../types.js';

interface CritterLogsViewProps {
  barn: Barn;
  critter: Critter;
  onBack: () => void;
}

export function CritterLogsView({ barn, critter, onBack }: CritterLogsViewProps) {
  const { stdout } = useStdout();
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [scrollOffset, setScrollOffset] = useState(0);

  const terminalHeight = stdout?.rows || 24;
  const visibleLines = terminalHeight - 6; // Account for header and padding

  useEffect(() => {
    let mounted = true;

    async function fetchLogs() {
      setLoading(true);
      setError(null);

      const fullBarn = loadBarn(barn.name);
      if (!fullBarn) {
        setError(`Barn not found: ${barn.name}`);
        setLoading(false);
        return;
      }

      const result = await readCritterLogs(critter, fullBarn, { lines: 200 });

      if (!mounted) return;

      if (result.error) {
        setError(result.error);
      } else {
        const lines = result.content.split('\n');
        setLogs(lines);
        // Scroll to bottom initially
        setScrollOffset(Math.max(0, lines.length - visibleLines));
      }
      setLoading(false);
    }

    fetchLogs();

    return () => {
      mounted = false;
    };
  }, [barn.name, critter, visibleLines]);

  useInput((input, key) => {
    if (key.escape) {
      onBack();
      return;
    }

    // Scroll with arrow keys
    if (key.upArrow) {
      setScrollOffset((prev) => Math.max(0, prev - 1));
      return;
    }
    if (key.downArrow) {
      setScrollOffset((prev) => Math.min(logs.length - visibleLines, prev + 1));
      return;
    }

    // Page up/down
    if (key.pageUp) {
      setScrollOffset((prev) => Math.max(0, prev - visibleLines));
      return;
    }
    if (key.pageDown) {
      setScrollOffset((prev) => Math.min(logs.length - visibleLines, prev + visibleLines));
      return;
    }

    // Home/End
    if (input === 'g') {
      setScrollOffset(0);
      return;
    }
    if (input === 'G') {
      setScrollOffset(Math.max(0, logs.length - visibleLines));
      return;
    }

    // Refresh
    if (input === 'r') {
      setLoading(true);
      (async () => {
        const fullBarn = loadBarn(barn.name);
        if (!fullBarn) return;
        const result = await readCritterLogs(critter, fullBarn, { lines: 200 });
        if (result.error) {
          setError(result.error);
        } else {
          const lines = result.content.split('\n');
          setLogs(lines);
          setScrollOffset(Math.max(0, lines.length - visibleLines));
        }
        setLoading(false);
      })();
      return;
    }
  });

  const visibleLogs = logs.slice(scrollOffset, scrollOffset + visibleLines);

  return (
    <Box flexDirection="column" flexGrow={1}>
      <CritterHeader barn={barn} critter={critter} />

      <Box flexDirection="column" paddingX={2} flexGrow={1}>
        {loading ? (
          <Text dimColor>Loading logs...</Text>
        ) : error ? (
          <Text color="red">{error}</Text>
        ) : logs.length === 0 ? (
          <Text dimColor>No logs found</Text>
        ) : (
          <Box flexDirection="column">
            {visibleLogs.map((line, i) => (
              <Text key={scrollOffset + i} wrap="truncate">{line}</Text>
            ))}
          </Box>
        )}
      </Box>

      <Box paddingX={2}>
        <Text dimColor>
          {logs.length > 0 && `${scrollOffset + 1}-${Math.min(scrollOffset + visibleLines, logs.length)} of ${logs.length} | `}
          ↑↓ scroll | g/G top/bottom | r refresh | Esc back
        </Text>
      </Box>
    </Box>
  );
}
