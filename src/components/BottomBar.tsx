import React from 'react';
import { Box, Text } from 'ink';

interface RemoteEnvironment {
  barn: { name: string };
  state: 'available' | 'not-checked' | 'checking' | 'unavailable' | 'unreachable';
}

interface BottomBarProps {
  items: Array<{ key: string; label: string }>;
  environments?: RemoteEnvironment[];
  isDetecting?: boolean;
}

// Yeehaw brand gold
const BRAND_COLOR = '#f0c040';

export function BottomBar({ items, environments = [], isDetecting = false }: BottomBarProps) {
  return (
    <Box paddingX={2} justifyContent="space-between">
      {/* Left side: help items */}
      <Box gap={2}>
        {items.map((item, i) => (
          <Text key={i}>
            <Text color={BRAND_COLOR}>{item.key}</Text>
            <Text dimColor> {item.label}</Text>
          </Text>
        ))}
      </Box>

      {/* Right side: environments */}
      <Box gap={2}>
        <Text>
          <Text color="green">[Local]</Text>
        </Text>
        {environments.map((env, i) => (
          <Text key={env.barn.name}>
            <Text color={BRAND_COLOR}>g{i + 1}</Text>
            <Text dimColor> {env.barn.name}</Text>
          </Text>
        ))}
        {isDetecting && (
          <Text dimColor>...</Text>
        )}
      </Box>
    </Box>
  );
}
