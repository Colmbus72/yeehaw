import React, { ReactNode } from 'react';
import { Box, Text } from 'ink';

// Yeehaw brand gold (darker for light mode readability)
const BRAND_COLOR = '#d4a020';

interface PanelProps {
  title: string;
  children: ReactNode;
  focused?: boolean;
  width?: number | string;
  hints?: string;  // Contextual hotkey hints like "[n] new  [d] delete"
}

// Render hints with gold keys and gray labels: "[n] new" -> gold "[n]" + gray " new"
function renderHints(hints: string) {
  const parts: React.ReactNode[] = [];
  // Match [key] patterns and split around them
  const regex = /(\[[^\]]+\])/g;
  let lastIndex = 0;
  let match;
  let keyIndex = 0;

  while ((match = regex.exec(hints)) !== null) {
    // Add text before the match (gray)
    if (match.index > lastIndex) {
      parts.push(
        <Text key={`text-${keyIndex}`} dimColor>
          {hints.slice(lastIndex, match.index)}
        </Text>
      );
    }
    // Add the [key] part (gold)
    parts.push(
      <Text key={`key-${keyIndex}`} color={BRAND_COLOR}>
        {match[1]}
      </Text>
    );
    lastIndex = regex.lastIndex;
    keyIndex++;
  }

  // Add remaining text after last match (gray)
  if (lastIndex < hints.length) {
    parts.push(
      <Text key={`text-end`} dimColor>
        {hints.slice(lastIndex)}
      </Text>
    );
  }

  return parts;
}

export function Panel({ title, children, focused = false, width, hints }: PanelProps) {
  return (
    <Box
      flexDirection="column"
      borderStyle="single"
      borderColor={focused ? BRAND_COLOR : 'gray'}
      width={width}
      minHeight={7}
      flexShrink={1}
    >
      {/* Title - fixed at top */}
      <Box paddingX={1} flexShrink={0}>
        <Text bold color={focused ? BRAND_COLOR : 'gray'}>
          {title}
        </Text>
      </Box>
      {/* Content - grows and clips overflow */}
      <Box flexDirection="column" paddingX={1} flexGrow={1} flexShrink={1} overflow="hidden">
        {children}
      </Box>
      {/* Hints - fixed at bottom, never clipped */}
      <Box paddingX={1} justifyContent="flex-end" height={1} flexShrink={0}>
        {focused && hints ? (
          <Text>{renderHints(hints)}</Text>
        ) : (
          <Text> </Text>
        )}
      </Box>
    </Box>
  );
}
