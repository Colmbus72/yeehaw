import React, { useState, useEffect } from 'react';
import { Box, Text, useInput, useStdout } from 'ink';
import { Panel } from './Panel.js';
import { ScrollableMarkdown } from './ScrollableMarkdown.js';

// Yeehaw brand gold
const BRAND_COLOR = '#d4a020';

interface ClaudeSplashScreenProps {
  systemPrompt: string;
  mcpTools: string[];
  projectColor?: string;
  onComplete: () => void;
  onCancel: () => void;
}

type FocusedPanel = 'prompt' | 'tools';

/**
 * Splash screen shown while Claude session loads in the background.
 * Displays the system prompt and MCP tools being injected.
 */
export function ClaudeSplashScreen({
  systemPrompt,
  mcpTools,
  projectColor,
  onComplete,
  onCancel,
}: ClaudeSplashScreenProps) {
  const { stdout } = useStdout();
  const terminalHeight = stdout?.rows || 24;

  const [countdown, setCountdown] = useState(3);
  const [paused, setPaused] = useState(false);
  const [focusedPanel, setFocusedPanel] = useState<FocusedPanel>('prompt');
  const [toolsIndex, setToolsIndex] = useState(0);

  // Calculate available height for panels
  // Header area: ~6 lines (title, countdown, help text, spacing)
  // Bottom help: ~2 lines
  const panelHeight = Math.max(10, terminalHeight - 10);

  // Countdown timer
  useEffect(() => {
    if (paused) return;

    if (countdown <= 0) {
      onComplete();
      return;
    }

    const timer = setTimeout(() => {
      setCountdown((c) => c - 1);
    }, 1000);

    return () => clearTimeout(timer);
  }, [countdown, paused, onComplete]);

  // Keyboard handling
  useInput((input, key) => {
    // Space: toggle pause
    if (input === ' ') {
      setPaused((p) => !p);
      return;
    }

    // Escape: cancel
    if (key.escape) {
      onCancel();
      return;
    }

    // Tab: switch panels
    if (key.tab) {
      setFocusedPanel((p) => (p === 'prompt' ? 'tools' : 'prompt'));
      return;
    }

    // j/k navigation for tools panel
    if (focusedPanel === 'tools') {
      if (input === 'j' || key.downArrow) {
        setToolsIndex((i) => Math.min(i + 1, mcpTools.length - 1));
      }
      if (input === 'k' || key.upArrow) {
        setToolsIndex((i) => Math.max(i - 1, 0));
      }
      if (input === 'g') {
        setToolsIndex(0);
      }
      if (input === 'G') {
        setToolsIndex(mcpTools.length - 1);
      }
    }
  });

  // Strip mcp__yeehaw__ prefix from tool names for display
  const displayTools = mcpTools.map((tool) =>
    tool.replace(/^mcp__yeehaw__/, '')
  );

  // Calculate visible tools window
  const toolsVisibleCount = Math.max(5, panelHeight - 4);
  const toolsStart = Math.max(
    0,
    Math.min(toolsIndex - Math.floor(toolsVisibleCount / 2), mcpTools.length - toolsVisibleCount)
  );
  const visibleTools = displayTools.slice(toolsStart, toolsStart + toolsVisibleCount);

  return (
    <Box flexDirection="column" height={terminalHeight}>
      {/* Header section */}
      <Box flexDirection="column" alignItems="center" paddingY={1}>
        <Text bold color={projectColor || BRAND_COLOR}>
          Starting Session
        </Text>
        <Box marginY={1}>
          <Text
            bold
            color={paused ? 'gray' : projectColor || BRAND_COLOR}
            dimColor={paused}
          >
            {countdown}
          </Text>
        </Box>
        <Text dimColor>
          {paused ? '[space] resume' : '[space] pause'}
        </Text>
      </Box>

      {/* Two-panel content area */}
      <Box flexGrow={1} paddingX={1} gap={1}>
        {/* Left: System Prompt (~65%) */}
        <Panel
          title="System Prompt"
          focused={focusedPanel === 'prompt'}
          width="65%"
          hints="j/k scroll"
        >
          <ScrollableMarkdown focused={focusedPanel === 'prompt'} height={panelHeight - 3}>
            {systemPrompt || '_No system prompt_'}
          </ScrollableMarkdown>
        </Panel>

        {/* Right: MCP Tools (~35%) */}
        <Panel
          title="MCP Tools"
          focused={focusedPanel === 'tools'}
          width="35%"
          hints="j/k scroll"
        >
          <Box flexDirection="column" overflow="hidden">
            {visibleTools.map((tool, i) => {
              const actualIndex = toolsStart + i;
              const isSelected = actualIndex === toolsIndex && focusedPanel === 'tools';
              return (
                <Box key={tool}>
                  <Text color={isSelected ? BRAND_COLOR : undefined}>
                    {isSelected ? '› ' : '  '}
                  </Text>
                  <Text
                    color={isSelected ? BRAND_COLOR : undefined}
                    bold={isSelected}
                    wrap="truncate"
                  >
                    {tool}
                  </Text>
                </Box>
              );
            })}
            {mcpTools.length > toolsVisibleCount && (
              <Box marginTop={1} justifyContent="flex-end">
                <Text dimColor>
                  [{toolsIndex + 1}/{mcpTools.length}]
                </Text>
              </Box>
            )}
          </Box>
        </Panel>
      </Box>

      {/* Bottom help text */}
      <Box justifyContent="center" paddingY={1}>
        <Text dimColor>
          <Text color={BRAND_COLOR}>[space]</Text> {paused ? 'resume' : 'pause'}
          {'  '}
          <Text color={BRAND_COLOR}>[esc]</Text> cancel
          {'  '}
          <Text color={BRAND_COLOR}>[tab]</Text> switch
          {'  '}
          <Text color={BRAND_COLOR}>[j/k]</Text> scroll
        </Text>
      </Box>
    </Box>
  );
}
