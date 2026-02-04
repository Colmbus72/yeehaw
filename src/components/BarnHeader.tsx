import React from 'react';
import { Box, Text } from 'ink';

interface BarnHeaderProps {
  name: string;
  subtitle?: string;
}

const BARN_DOOR_ART = `
       _.-^-._    .--.
    .-'   _   '-. |__|
   /     |_|     \\|  |
  /               \\  |
 /|     _____     |\\ |
  |    |==|==|    |  |
  |    |--|--|    |  |
  |    |==|==|    |  |
`;

// Grey gradient from light to dark
const GREY_GRADIENT = [
  'rgb(160,160,160)',
  'rgb(130,130,130)',
  'rgb(100,100,100)',
  'rgb(80,80,80)',
  'rgb(60,60,60)',
];

export function BarnHeader({ name, subtitle }: BarnHeaderProps) {
  const lines = BARN_DOOR_ART.split('\n');

  return (
    <Box flexDirection="column" paddingTop={1} paddingLeft={2}>
      <Box>
        {/* Server rack art */}
        <Box flexDirection="column">
          {lines.map((line, i) => (
            <Text key={i} color={GREY_GRADIENT[i] || GREY_GRADIENT[GREY_GRADIENT.length - 1]}>
              {line}
            </Text>
          ))}
        </Box>

        {/* Barn name and info */}
        <Box flexDirection="column" marginLeft={3} justifyContent="center">
          <Text bold color="rgb(200,200,200)">{name.toUpperCase()}</Text>
          {subtitle && (
            <Text dimColor>{subtitle}</Text>
          )}
        </Box>
      </Box>
    </Box>
  );
}
