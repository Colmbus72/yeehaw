import React from 'react';
import { Box, Text } from 'ink';
import type { Project, Livestock } from '../types.js';

interface LivestockHeaderProps {
  project: Project;
  livestock: Livestock;
}

// Base cow ASCII art with @ symbols as pattern spots
// The @ symbols will be replaced with pattern characters for variation
// const COW_TEMPLATE = [
//   " ___,,,___ _..............._",
//   " '-/~'~\-'` @@@@@'   .@@    \\",
//   "   |a a |   '@@@'     `@@   ||",
//   "   |   /@@.  @@@.  .@@@.  @@||",
//   "   (oo/@@'  .@@@@..@@@@@@.  /|",
//   "    `` '._   /@@@   @@@@' <`\/",
//   "          | /`--....-'`Y   \))",
//   "          |||         //'. |((",
//   "          |||        //   ||",
//   "          //(        `    /(",
// ];

const COW_TEMPLATE = [
  "                                 /;    ;\\",
  "                             __  \\\\____//",
  "                            /{_\\_/   `'\\____",
  "                            \\___   (o)  (o  }",
  "       _______________________/          :--'",
  "   ,-,'`@@@@@@@@@@@@@@@@@@@@@@  \\_    `__\\",
  "  ;:(  @@@@@@@@@@@@@@@@@@@@@@@@   \\___(o'o)",
  "  :: ) @@@@@@@@@@@@@@@@@@@@@@@,'@@(  `===='",
  "  :: \\ @@@@@@: @@@@@@@) @@ (  '@@@'",
  "  ;; /\\ @@@  /`,  @@@@@\\   :@@@@@)",
  "  ::/  )    {_----------:  :~`,~~;",
  " ;;'`; :   )            :  / `; ;",
  "`'`' / :  :             :  :  : :",
  "    )_ \\__;             :_ ;  \\_\\",
  "    :__\\  \\             \\  \\  :  \\",
  "        `^'              `^'  `-^-'",
];
//  "                                   /;    ;\\",
//  "                               __  \\\\____//",
//  "                              /{_\\_/   `'\\____",
//  "                              \\___   (o)  (o  }",
//  "   _____________________________/          :--'",
//  ",-,'`@@@@@@@@@@@@@@@@@@@@@         \\_    `__\\",
//  ";:(  @@@@@@@@@@@@@@@@ @@@ @@@@@@@     \\___(o'o)",
//  ":: )  @@@@@@@@@@@   @@@@@@@@@@@@@ ,'@@(  `===='",
//  ":: : @@@@@: @@@@@@@  @@@@ @@@@@@@ `@@@:",
//  ":: \\  @@@@@:       @@@@@@@)    (  '@@@'",
//  ";; /\\      /`,    @@@@@@@@@\\   :@@@@@)",
//  "::/  )    {_----------------:  :~`,~~;",
//  ";;'`; :   )                  :  / `; ;",
//  ";;;; : :   ;                  :  ;  ; :",
//  "`'`' / :  :                   :  :  : :",
//  "    )_ \\__;                     :_ ;  \\_\\       `,','",
//  "    :__\\  \\                   \\  \\  :  \\   *  8`;'*  *",
//  "        `^'                     `^'  `-^-'   \\v/ :  \\/",
//];

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

// Pattern characters - space and block characters only
// Space creates gaps in the spots, blocks create varying density
const PATTERN_CHARS = [' ', ' ', '░', '░', '▒', '▓', '█'];

// Generate pattern variation for the cow based on livestock/project data
function generateCowArt(livestock: Livestock, project: Project): string[] {
  // Use git branch (environment) + livestock name for unique seed
  // Branch typically indicates environment (main, staging, production, etc.)
  const seed = `${livestock.branch || 'default'}-${livestock.name}-${project.name}`;
  const hashes = multiHash(seed);

  // Replace @ symbols with pattern characters
  // Use multiple hashes and position for maximum variety
  let charIndex = 0;
  const cowArt = COW_TEMPLATE.map((line, lineIndex) => {
    let result = '';
    for (const char of line) {
      if (char === '@') {
        // Mix multiple hashes with position data for variety
        const h1 = hashes[0];
        const h2 = hashes[1];
        const h3 = hashes[2];

        // Combine hashes with position in different ways
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

  return cowArt;
}

export function LivestockHeader({ project, livestock }: LivestockHeaderProps) {
  const cowArt = generateCowArt(livestock, project);
  const color = project.color || '#b8860b'; // Use project color or default tan

  // Calculate where to place the livestock name (in the body area)
  // We'll show it to the right of the cow
  const cowHeight = cowArt.length;
  const infoStartLine = Math.floor(cowHeight / 2) - 1;

  return (
    <Box flexDirection="column" paddingTop={1} paddingLeft={1}>
      <Box flexDirection="row">
        {/* Cow ASCII art */}
        <Box flexDirection="column">
          {cowArt.map((line, i) => (
            <Text key={i} color={color} bold>{line}</Text>
          ))}
        </Box>

        {/* Livestock info - positioned to the right */}
        <Box flexDirection="column" marginLeft={2} justifyContent="center">
          <Text bold color={color}>{livestock.name}</Text>
          <Text dimColor>project: {project.name}</Text>
          <Text dimColor>barn: {livestock.barn || 'local'}</Text>
          {livestock.branch && (
            <Text dimColor>branch: {livestock.branch}</Text>
          )}
        </Box>
      </Box>
    </Box>
  );
}
