import React from 'react';
import { Box, Text } from 'ink';
import type { Critter, Barn } from '../types.js';

interface CritterHeaderProps {
  barn: Barn;
  critter: Critter;
}

// Rabbit ASCII art template - @ symbols are pattern spots
const RABBIT_TEMPLATE = [
  "                              __",
  "                     /\\    .-\" /",
  "                    /  ; .'  .' ",
  "                   :   :/  .'   ",
  "                    \\  ;-.'     ",
  "       .--\"\"\"\"--..__/     `.    ",
  "     .'@@@@@@@@@@@.'     o  \\   ",
  "    /@@@@@@@@@@@@@@@@        ;  ",
  "   :@@@@@@@@@@@@@@@@\\       :  ",
  " .-;@@@@@@@@-.@@@@@@@`.__.--'  ",
  ":  ;@@@@@@@@@@\\@@@@@,   ;       ",
  "'._:@@@@@@@@@@@;@@@:   (        ",
  "    \\/  .__    ;    \\   `-.     ",
  "     ;     \"-,/_..--\"`-..__)    ",
  "     '\"\"--.._:                  ",
];

// Generate multiple hash values from a string for better distribution
function multiHash(str: string): number[] {
  const hashes: number[] = [];

  // First hash - djb2
  let hash1 = 5381;
  for (let i = 0; i < str.length; i++) {
    hash1 = ((hash1 << 5) + hash1) ^ str.charCodeAt(i);
  }
  hashes.push(Math.abs(hash1));

  // Second hash - sdbm
  let hash2 = 0;
  for (let i = 0; i < str.length; i++) {
    hash2 = str.charCodeAt(i) + (hash2 << 6) + (hash2 << 16) - hash2;
  }
  hashes.push(Math.abs(hash2));

  // Third hash - fnv-1a inspired
  let hash3 = 2166136261;
  for (let i = 0; i < str.length; i++) {
    hash3 ^= str.charCodeAt(i);
    hash3 = (hash3 * 16777619) >>> 0;
  }
  hashes.push(hash3);

  return hashes;
}

// Pattern characters - space and block characters
const PATTERN_CHARS = [' ', ' ', '░', '░', '▒', '▓', '█'];

// Generate pattern variation for the rabbit based on critter/barn data
function generateRabbitArt(critter: Critter, barn: Barn): string[] {
  // Use barn name + critter name + service for unique seed
  const seed = `${barn.name}-${critter.name}-${critter.service}`;
  const hashes = multiHash(seed);

  // Replace @ symbols with pattern characters
  let charIndex = 0;
  const rabbitArt = RABBIT_TEMPLATE.map((line, lineIndex) => {
    let result = '';
    for (const char of line) {
      if (char === '@') {
        const h1 = hashes[0];
        const h2 = hashes[1];
        const h3 = hashes[2];

        const mix = (h1 >> (charIndex % 17)) ^
                    (h2 >> ((charIndex + lineIndex) % 13)) ^
                    (h3 >> ((charIndex * 7 + lineIndex * 3) % 19));

        const charChoice = Math.abs(mix) % PATTERN_CHARS.length;
        result += PATTERN_CHARS[charChoice];
        charIndex++;
      } else {
        result += char;
      }
    }
    return result;
  });

  return rabbitArt;
}

export function CritterHeader({ barn, critter }: CritterHeaderProps) {
  const rabbitArt = generateRabbitArt(critter, barn);
  // Use a default color for critters (could be configurable later)
  const color = '#8fbc8f'; // Dark sea green

  return (
    <Box flexDirection="column" paddingTop={1} paddingLeft={1}>
      <Box flexDirection="row">
        {/* Rabbit ASCII art */}
        <Box flexDirection="column">
          {rabbitArt.map((line, i) => (
            <Text key={i} color={color} bold>{line}</Text>
          ))}
        </Box>

        {/* Critter info - positioned to the right */}
        <Box flexDirection="column" marginLeft={2} justifyContent="center">
          <Text bold color={color}>{critter.name}</Text>
          <Text dimColor>barn: {barn.name}</Text>
          <Text dimColor>service: {critter.service}</Text>
        </Box>
      </Box>
    </Box>
  );
}
