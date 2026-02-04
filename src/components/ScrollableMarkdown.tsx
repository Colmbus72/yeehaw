import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import { marked } from 'marked';
// @ts-ignore - no types available
import TerminalRenderer from 'marked-terminal';

interface ScrollableMarkdownProps {
  children: string;
  focused?: boolean;
  height?: number;
}

// Configure marked to use terminal renderer
marked.setOptions({
  renderer: new TerminalRenderer({
    showSectionPrefix: false,
    reflowText: true,
    width: 80,
  }),
});

/**
 * Scrollable markdown content panel.
 * When focused, j/k or arrow keys scroll the content.
 */
export function ScrollableMarkdown({
  children,
  focused = false,
  height = 20,
}: ScrollableMarkdownProps) {
  const [scrollOffset, setScrollOffset] = useState(0);

  // Reset scroll when content changes
  useEffect(() => {
    setScrollOffset(0);
  }, [children]);

  // Render markdown to string with ANSI codes
  const rendered = String(marked.parse(children)).trim();
  const lines = rendered.split('\n');
  const totalLines = lines.length;
  const visibleLines = height - 1; // Leave room for scroll indicator

  useInput((input, key) => {
    if (!focused) return;

    if (input === 'j' || key.downArrow) {
      setScrollOffset((prev) => Math.min(prev + 1, Math.max(0, totalLines - visibleLines)));
    }
    if (input === 'k' || key.upArrow) {
      setScrollOffset((prev) => Math.max(prev - 1, 0));
    }
    if (input === 'g') {
      setScrollOffset(0);
    }
    if (input === 'G') {
      setScrollOffset(Math.max(0, totalLines - visibleLines));
    }
    if (key.pageDown) {
      setScrollOffset((prev) => Math.min(prev + visibleLines, Math.max(0, totalLines - visibleLines)));
    }
    if (key.pageUp) {
      setScrollOffset((prev) => Math.max(prev - visibleLines, 0));
    }
  });

  // Get visible slice of lines
  const displayLines = lines.slice(scrollOffset, scrollOffset + visibleLines);
  const showScrollIndicator = totalLines > visibleLines;

  return (
    <Box flexDirection="column" flexGrow={1} flexShrink={1} overflow="hidden">
      <Box flexDirection="column" flexGrow={1} flexShrink={1}>
        {displayLines.map((line, idx) => (
          <Text key={scrollOffset + idx} wrap="truncate">{line || ' '}</Text>
        ))}
      </Box>
      {showScrollIndicator && (
        <Box justifyContent="flex-end" flexShrink={0}>
          <Text dimColor>
            [{scrollOffset + 1}-{Math.min(scrollOffset + visibleLines, totalLines)}/{totalLines}]
            {focused ? ' (j/k to scroll)' : ''}
          </Text>
        </Box>
      )}
    </Box>
  );
}
