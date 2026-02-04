import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import { Header } from '../components/Header.js';
import { Panel } from '../components/Panel.js';
import { readLivestockLogs } from '../lib/livestock.js';
import type { Project, Livestock } from '../types.js';

interface LogsViewProps {
  project: Project;
  livestock: Livestock;
  onBack: () => void;
}

export function LogsView({ project, livestock, onBack }: LogsViewProps) {
  const [logs, setLogs] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [scrollOffset, setScrollOffset] = useState(0);

  // Calculate visible height (leave room for header, panel border, hints)
  const visibleHeight = 20;

  const fetchLogs = async () => {
    setLoading(true);
    setError(null);

    const result = await readLivestockLogs(livestock, { lines: 200 });

    if (result.error) {
      setError(result.error);
      setLogs([]);
    } else {
      const lines = result.content.split('\n');
      setLogs(lines);
      // Scroll to bottom by default (most recent logs)
      setScrollOffset(Math.max(0, lines.length - visibleHeight));
    }

    setLoading(false);
  };

  // Fetch logs on mount
  useEffect(() => {
    fetchLogs();
  }, [livestock]);

  const totalLines = logs.length;

  useInput((input, key) => {
    if (key.escape) {
      onBack();
      return;
    }

    // Refresh logs
    if (input === 'r') {
      fetchLogs();
      return;
    }

    // Scroll navigation
    if (input === 'j' || key.downArrow) {
      setScrollOffset((prev) => Math.min(prev + 1, Math.max(0, totalLines - visibleHeight)));
      return;
    }

    if (input === 'k' || key.upArrow) {
      setScrollOffset((prev) => Math.max(prev - 1, 0));
      return;
    }

    if (input === 'g') {
      setScrollOffset(0);
      return;
    }

    if (input === 'G') {
      setScrollOffset(Math.max(0, totalLines - visibleHeight));
      return;
    }

    if (key.pageDown) {
      setScrollOffset((prev) => Math.min(prev + visibleHeight, Math.max(0, totalLines - visibleHeight)));
      return;
    }

    if (key.pageUp) {
      setScrollOffset((prev) => Math.max(prev - visibleHeight, 0));
      return;
    }
  });

  // Loading state
  if (loading) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Logs: ${livestock.name}`} color={project.color} />
        <Box padding={2}>
          <Text>Loading logs...</Text>
        </Box>
      </Box>
    );
  }

  // Error state
  if (error) {
    return (
      <Box flexDirection="column" flexGrow={1}>
        <Header text={project.name} subtitle={`Logs: ${livestock.name}`} color={project.color} />
        <Box padding={2} flexDirection="column">
          <Text color="red">Error: {error}</Text>
          <Box marginTop={1}>
            <Text dimColor>[r] retry  [q] back</Text>
          </Box>
        </Box>
      </Box>
    );
  }

  // Get visible slice of lines
  const displayLines = logs.slice(scrollOffset, scrollOffset + visibleHeight);
  const showScrollIndicator = totalLines > visibleHeight;

  const hints = '[r] refresh  [j/k] scroll  [g/G] top/bottom  [q] back';

  return (
    <Box flexDirection="column" flexGrow={1}>
      <Header text={project.name} subtitle={`Logs: ${livestock.name}`} color={project.color} />

      <Box flexGrow={1} paddingX={1}>
        <Panel
          title={livestock.log_path || 'Logs'}
          focused={true}
          hints={hints}
        >
          <Box flexDirection="column" height={visibleHeight + 1}>
            <Box flexDirection="column" flexGrow={1}>
              {displayLines.length > 0 ? (
                displayLines.map((line, idx) => (
                  <Text key={scrollOffset + idx} wrap="truncate">
                    {line || ' '}
                  </Text>
                ))
              ) : (
                <Text dimColor>No log content</Text>
              )}
            </Box>
            {showScrollIndicator && (
              <Box justifyContent="flex-end">
                <Text dimColor>
                  [{scrollOffset + 1}-{Math.min(scrollOffset + visibleHeight, totalLines)}/{totalLines}]
                </Text>
              </Box>
            )}
          </Box>
        </Panel>
      </Box>
    </Box>
  );
}
