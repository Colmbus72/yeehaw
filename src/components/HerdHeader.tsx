import React from 'react';
import { Box, Text } from 'ink';
import type { Herd } from '../types.js';

interface HerdHeaderProps {
  herd: Herd;
  projectColor?: string;
  ranchHandName?: string;
}

// Fence ASCII art representing a herd/corral
const FENCE_ART = [
  "   _             _              _",
  "__| |___________| |____________| |________",
  "__| |___________| |____________| |________",
  "  | |           | |            | |",
  "  | |           | |            | |",
  "__| |___________| |____________| |________",
  "__| |___________| |____________| |________",
  "  | |           | |            | |",
];

export function HerdHeader({ herd, projectColor, ranchHandName }: HerdHeaderProps) {
  // Use project color or default earthy brown for herds
  const color = projectColor || '#8b7355';

  return (
    <Box flexDirection="column" paddingTop={1} paddingLeft={1}>
      <Box flexDirection="row">
        {/* Fence ASCII art */}
        <Box flexDirection="column">
          {FENCE_ART.map((line, i) => (
            <Text key={i} color={color} bold>{line}</Text>
          ))}
        </Box>

        {/* Herd info - positioned to the right */}
        <Box flexDirection="column" marginLeft={2} justifyContent="center">
          <Text bold color={color}>{herd.name}</Text>
          <Text dimColor>{herd.livestock.length} livestock, {herd.critters.length} critters</Text>
          {ranchHandName && (
            <Text dimColor>synced by: {ranchHandName}</Text>
          )}
        </Box>
      </Box>
    </Box>
  );
}
