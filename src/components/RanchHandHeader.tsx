import React from 'react';
import { Box, Text } from 'ink';
import type { RanchHand } from '../types.js';

interface RanchHandHeaderProps {
  ranchhand: RanchHand;
  projectColor?: string;
}

// Static cowboy bust ASCII art
const COWBOY_ART = [
  "       ,_.,",
  "    __/ `_(__",
  "   '-..,__..-`",
  "     @ *Y*|",
  "     |  - |   ",
  "  ___'_..'.._",
  " /   \\_\\'/_| \\",
];

export function RanchHandHeader({ ranchhand, projectColor }: RanchHandHeaderProps) {
  // Use project color or default tan/brown for cowboys
  const color = projectColor || '#cd853f';

  return (
    <Box flexDirection="column" paddingTop={1} paddingLeft={1}>
      <Box flexDirection="row">
        {/* Cowboy ASCII art */}
        <Box flexDirection="column">
          {COWBOY_ART.map((line, i) => (
            <Text key={i} color={color} bold>{line}</Text>
          ))}
        </Box>

        {/* Ranch hand info - positioned to the right */}
        <Box flexDirection="column" marginLeft={2} justifyContent="center">
          <Text bold color={color}>{ranchhand.name}</Text>
          <Text dimColor>type: {ranchhand.type}</Text>
          <Text dimColor>herd: {ranchhand.herd || '(none)'}</Text>
          {ranchhand.last_sync && (
            <Text dimColor>last sync: {new Date(ranchhand.last_sync).toLocaleString()}</Text>
          )}
        </Box>
      </Box>
    </Box>
  );
}
