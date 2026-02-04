import React, { useState, useEffect, useCallback, useMemo, useRef, memo } from 'react';
import { Box, Text, useInput, useStdout } from 'ink';
import type { NightSkyContext, VisualizerSession } from '../types.js';

// Easing function for smooth transitions (ease-in-out cubic)
function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

// Static star characters (dimmer, always visible)
const STATIC_STAR_CHARS = ['.', '·', '+'];


// Test messages for cloud demo
const TEST_MESSAGES = [
  'Hello from the desert!',
  'New commit pushed',
  'Build succeeded',
  'PR #42 merged',
  'Deploy complete',
  'Tests passing',
];

// Cactus templates - block-style saguaros
const CACTUS_TEMPLATES = [
  // Tall saguaro with both arms
  [
    '  █  ',
    '█ █  ',
    '  █ █',
    '  █  ',
    '  █  ',
  ],
  // Medium saguaro with left arm
  [
    '  █  ',
    '█ █  ',
    '  █  ',
    '  █  ',
  ],
  // Medium saguaro with right arm
  [
    '  █  ',
    '  █ █',
    '  █  ',
    '  █  ',
  ],
  // Small saguaro
  [
    '  █  ',
    '  █  ',
    '  █  ',
  ],
  // Tiny cactus
  [
    ' █ ',
    ' █ ',
  ],
];

// Green gradient colors for cacti - all vibrant greens (lighter top to darker bottom)
const CACTUS_COLORS = ['#5fa33a', '#4a9030', '#3d8028', '#307020'];

// Simple 5x7 bitmap font for constellation text
// Each letter is an array of strings representing rows
// '#' = star position, ' ' = empty
const BITMAP_FONT: Record<string, string[]> = {
  'A': [
    '  #  ',
    ' # # ',
    '#   #',
    '#####',
    '#   #',
    '#   #',
    '#   #',
  ],
  'B': [
    '#### ',
    '#   #',
    '#   #',
    '#### ',
    '#   #',
    '#   #',
    '#### ',
  ],
  'C': [
    ' ### ',
    '#   #',
    '#    ',
    '#    ',
    '#    ',
    '#   #',
    ' ### ',
  ],
  'D': [
    '#### ',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    '#### ',
  ],
  'E': [
    '#####',
    '#    ',
    '#    ',
    '#### ',
    '#    ',
    '#    ',
    '#####',
  ],
  'F': [
    '#####',
    '#    ',
    '#    ',
    '#### ',
    '#    ',
    '#    ',
    '#    ',
  ],
  'G': [
    ' ### ',
    '#   #',
    '#    ',
    '# ###',
    '#   #',
    '#   #',
    ' ### ',
  ],
  'H': [
    '#   #',
    '#   #',
    '#   #',
    '#####',
    '#   #',
    '#   #',
    '#   #',
  ],
  'I': [
    '#####',
    '  #  ',
    '  #  ',
    '  #  ',
    '  #  ',
    '  #  ',
    '#####',
  ],
  'J': [
    '#####',
    '    #',
    '    #',
    '    #',
    '    #',
    '#   #',
    ' ### ',
  ],
  'K': [
    '#   #',
    '#  # ',
    '# #  ',
    '##   ',
    '# #  ',
    '#  # ',
    '#   #',
  ],
  'L': [
    '#    ',
    '#    ',
    '#    ',
    '#    ',
    '#    ',
    '#    ',
    '#####',
  ],
  'M': [
    '#   #',
    '## ##',
    '# # #',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
  ],
  'N': [
    '#   #',
    '##  #',
    '# # #',
    '#  ##',
    '#   #',
    '#   #',
    '#   #',
  ],
  'O': [
    ' ### ',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    ' ### ',
  ],
  'P': [
    '#### ',
    '#   #',
    '#   #',
    '#### ',
    '#    ',
    '#    ',
    '#    ',
  ],
  'Q': [
    ' ### ',
    '#   #',
    '#   #',
    '#   #',
    '# # #',
    '#  # ',
    ' ## #',
  ],
  'R': [
    '#### ',
    '#   #',
    '#   #',
    '#### ',
    '# #  ',
    '#  # ',
    '#   #',
  ],
  'S': [
    ' ### ',
    '#   #',
    '#    ',
    ' ### ',
    '    #',
    '#   #',
    ' ### ',
  ],
  'T': [
    '#####',
    '  #  ',
    '  #  ',
    '  #  ',
    '  #  ',
    '  #  ',
    '  #  ',
  ],
  'U': [
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    ' ### ',
  ],
  'V': [
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    ' # # ',
    '  #  ',
  ],
  'W': [
    '#   #',
    '#   #',
    '#   #',
    '#   #',
    '# # #',
    '## ##',
    '#   #',
  ],
  'X': [
    '#   #',
    '#   #',
    ' # # ',
    '  #  ',
    ' # # ',
    '#   #',
    '#   #',
  ],
  'Y': [
    '#   #',
    '#   #',
    ' # # ',
    '  #  ',
    '  #  ',
    '  #  ',
    '  #  ',
  ],
  'Z': [
    '#####',
    '    #',
    '   # ',
    '  #  ',
    ' #   ',
    '#    ',
    '#####',
  ],
  '0': [
    ' ### ',
    '#   #',
    '#  ##',
    '# # #',
    '##  #',
    '#   #',
    ' ### ',
  ],
  '1': [
    '  #  ',
    ' ##  ',
    '  #  ',
    '  #  ',
    '  #  ',
    '  #  ',
    '#####',
  ],
  '2': [
    ' ### ',
    '#   #',
    '    #',
    '  ## ',
    ' #   ',
    '#    ',
    '#####',
  ],
  '3': [
    ' ### ',
    '#   #',
    '    #',
    '  ## ',
    '    #',
    '#   #',
    ' ### ',
  ],
  '4': [
    '#   #',
    '#   #',
    '#   #',
    '#####',
    '    #',
    '    #',
    '    #',
  ],
  '5': [
    '#####',
    '#    ',
    '#### ',
    '    #',
    '    #',
    '#   #',
    ' ### ',
  ],
  '6': [
    ' ### ',
    '#    ',
    '#    ',
    '#### ',
    '#   #',
    '#   #',
    ' ### ',
  ],
  '7': [
    '#####',
    '    #',
    '   # ',
    '  #  ',
    ' #   ',
    ' #   ',
    ' #   ',
  ],
  '8': [
    ' ### ',
    '#   #',
    '#   #',
    ' ### ',
    '#   #',
    '#   #',
    ' ### ',
  ],
  '9': [
    ' ### ',
    '#   #',
    '#   #',
    ' ####',
    '    #',
    '    #',
    ' ### ',
  ],
  '-': [
    '     ',
    '     ',
    '     ',
    '#####',
    '     ',
    '     ',
    '     ',
  ],
  ' ': [
    '     ',
    '     ',
    '     ',
    '     ',
    '     ',
    '     ',
    '     ',
  ],
};

// Star characters for constellation - weighted toward smaller/dimmer chars for legibility
// Mostly dots with occasional brighter stars for sparkle
const CONSTELLATION_CHARS_WEIGHTED = [
  '·', '·', '·', '·', '·',  // 50% tiny dots
  '.', '.', '.',            // 30% periods
  '+', '*',                 // 20% medium/bright
];

interface ConstellationStar {
  x: number;
  y: number;
  char: string;
  baseBrightness: number;
  pulsePhase: number;
  pulseSpeed: number;
  pulseAmount: number;
}

/**
 * Generate constellation stars for a text string
 */
function generateConstellation(
  text: string,
  startX: number,
  startY: number,
): ConstellationStar[] {
  const stars: ConstellationStar[] = [];
  const upperText = text.toUpperCase();
  let cursorX = startX;

  for (const char of upperText) {
    const bitmap = BITMAP_FONT[char];
    if (!bitmap) continue;

    for (let row = 0; row < bitmap.length; row++) {
      for (let col = 0; col < bitmap[row].length; col++) {
        if (bitmap[row][col] === '#') {
          // Small random offset for organic feel
          const offsetX = (Math.random() - 0.5) * 0.5;
          const offsetY = (Math.random() - 0.5) * 0.3;

          stars.push({
            x: Math.round(cursorX + col + offsetX),
            y: Math.round(startY + row + offsetY),
            char: CONSTELLATION_CHARS_WEIGHTED[randomInt(0, CONSTELLATION_CHARS_WEIGHTED.length - 1)],
            baseBrightness: randomFloat(0.5, 0.8), // Slightly dimmer overall
            pulsePhase: randomFloat(0, Math.PI * 2),
            pulseSpeed: randomFloat(0.03, 0.08),
            pulseAmount: randomFloat(0.1, 0.25),
          });
        }
      }
    }

    cursorX += 6; // Character width + spacing
  }

  return stars;
}

// Simplified ground entity templates for visualizer
const GROUND_COW = [
  '|NAME',
  '   ^__^',
  '   (oo)\\_______',
  '   (__)\\       )\\/\\',
  '       ||----w |',
  '       ||     ||',
];

const GROUND_RABBIT = [
  '  |NAME',
  '  (\\(\\',
  '  ( -.-)o',
  '  o_(\")(\")_',
];

// Tumbleweed template with status line above
const GROUND_TUMBLEWEED = [
  '|STATUS',
  ' ░ ░▒░ ░▒░',
  '░▒ · ‿ · ▒░',
  '▒░ ▒░▒░ ░▒',
  ' ░▒░ ░▒░ ░',
];

const TUMBLEWEED_COLOR = '#b8860b';

interface GroundEntity {
  type: 'livestock' | 'critter' | 'session';
  name: string;
  id: string;         // Unique identifier (for sessions: index-name)
  x: number;
  yOffset: number;    // Vertical offset from ground line (negative = higher up)
  art: string[];
  color: string;
  statusText?: string;  // For sessions
}

/**
 * Process ASCII art template, replacing |NAME or |STATUS with actual text
 */
function processAsciiArt(template: string[], name: string, statusText?: string): string[] {
  return template.map(line => {
    // Replace |NAME
    let nameIndex = line.indexOf('|NAME');
    if (nameIndex !== -1) {
      const before = line.slice(0, nameIndex);
      const after = line.slice(nameIndex + 5);
      return before + name + after.slice(Math.max(0, name.length - after.length));
    }

    // Replace |STATUS (for tumbleweeds)
    const statusIndex = line.indexOf('|STATUS');
    if (statusIndex !== -1 && statusText) {
      const before = line.slice(0, statusIndex);
      const after = line.slice(statusIndex + 7);
      return before + statusText + after.slice(Math.max(0, statusText.length - after.length));
    } else if (statusIndex !== -1) {
      // No status text, return line without |STATUS placeholder
      return line.slice(0, statusIndex) + line.slice(statusIndex + 7);
    }

    return line;
  });
}

/**
 * Simple deterministic hash for positioning
 */
function simpleHash(str: string): number {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    hash = ((hash << 5) - hash) + str.charCodeAt(i);
    hash = hash & hash;
  }
  return Math.abs(hash);
}

/**
 * Place entities along the ground - entities are prioritized over cacti
 * Uses deterministic positioning based on entity IDs to avoid flicker when status changes
 * Returns placed entities and their positions so cacti can fill gaps
 */
function placeGroundEntities(
  entities: Array<{ type: 'livestock' | 'critter' | 'session'; name: string; id: string; color: string; statusText?: string }>,
  groundWidth: number,
  entityCount: number
): { placed: GroundEntity[]; usedRanges: Array<[number, number]> } {
  const placed: GroundEntity[] = [];
  const usedRanges: Array<[number, number]> = [];

  // Sort entities by ID hash for consistent ordering (use ID for uniqueness)
  const sortedEntities = [...entities].sort((a, b) => simpleHash(a.id) - simpleHash(b.id));

  // Determine vertical spread based on entity count (more entities = more spread)
  const useVerticalSpread = entityCount > 3;
  const maxYOffset = useVerticalSpread ? 2 : 0;  // Reduced to keep entities closer to ground

  for (let i = 0; i < sortedEntities.length; i++) {
    const entity = sortedEntities[i];
    let art: string[];
    if (entity.type === 'session') {
      art = GROUND_TUMBLEWEED;
    } else if (entity.type === 'livestock') {
      art = GROUND_COW;
    } else {
      art = GROUND_RABBIT;
    }

    // Calculate width based on art and text that will be inserted
    const textWidth = entity.type === 'session'
      ? (entity.statusText?.length || 12) // Shorter default for truncated status
      : entity.name.length;
    const artWidth = Math.max(...art.map(line => line.length)) + textWidth;

    // Use deterministic position based on entity ID (with fallback for collisions)
    const idHash = simpleHash(entity.id);
    const baseX = 10 + (idHash % Math.max(1, groundWidth - artWidth - 20));

    // Deterministic y offset based on hash (creates visual depth)
    const yOffset = useVerticalSpread ? -((idHash % (maxYOffset + 1))) : 0;

    // Find a free position, starting from the deterministic base
    let x = -1;
    const offsets = [0, 15, -15, 30, -30, 45, -45, 60, -60];

    for (const offset of offsets) {
      const candidateX = Math.max(5, Math.min(groundWidth - artWidth - 5, baseX + offset));
      const candidateEnd = candidateX + artWidth;

      const overlaps = usedRanges.some(
        ([start, end]) => !(candidateEnd < start - 2 || candidateX > end + 2)
      );

      if (!overlaps) {
        x = candidateX;
        usedRanges.push([candidateX, candidateEnd]);
        break;
      }
    }

    if (x >= 0) {
      placed.push({
        type: entity.type,
        name: entity.name,
        id: entity.id,
        x,
        yOffset,
        art: processAsciiArt(art, entity.name, entity.statusText),
        color: entity.type === 'session' ? TUMBLEWEED_COLOR : entity.color,
        statusText: entity.statusText,
      });
    }
  }

  return { placed, usedRanges };
}

/**
 * Generate cacti in gaps left by entities
 */
function generateCactiAroundEntities(width: number, usedRanges: Array<[number, number]>): Cactus[] {
  const cacti: Cactus[] = [];
  let pos = randomInt(15, 30);

  while (pos < width - 15) {
    const cactusWidth = 8; // Approximate width of cactus

    // Check if this position overlaps with any entity
    const overlaps = usedRanges.some(
      ([start, end]) => !(pos + cactusWidth < start - 2 || pos > end + 2)
    );

    if (!overlaps) {
      const templateIndex = randomInt(0, 2);
      const template = CACTUS_TEMPLATES[templateIndex];
      const colorIndex = randomInt(0, CACTUS_COLORS.length - 1);
      const yOffset = randomInt(-1, 1);
      cacti.push({ x: pos, yOffset, template, colorIndex });
    }

    pos += randomInt(20, 35); // Slightly tighter spacing to fill more gaps
  }

  return cacti;
}

interface WinkingStar {
  type: 'winking';
  x: number;
  y: number;
  progress: number;
  speed: number;
  holdDuration: number;
}

interface StaticStar {
  type: 'static';
  x: number;
  y: number;
  char: string;
  baseBrightness: number;
  pulsePhase: number;
  pulseSpeed: number;
  pulseAmount: number;
}

type Star = WinkingStar | StaticStar;

interface MessageCloud {
  text: string;
  x: number;
  y: number;
  targetX: number;
  targetY: number;
  progress: number;
  speed: number;
}

interface Cactus {
  x: number;
  yOffset: number; // Vertical offset from ground line
  template: string[];
  colorIndex: number;
}

// Combined animation state to batch updates
interface AnimationState {
  stars: Star[];
  clouds: MessageCloud[];
  constellationStars: ConstellationStar[];
}

// Animation parameters
const FRAME_INTERVAL = 80; // ~12.5 FPS - smoother rendering with less CPU usage
const TARGET_WINKING_STARS = { min: 5, max: 10 };
const STATIC_STAR_COUNT = { min: 30, max: 50 };
const SPAWN_CHANCE = 0.05;
const LANDSCAPE_HEIGHT = 10; // Height of ground area
const GROUND_LINE_Y = 5; // Ground line position - entities stand here

function randomFloat(min: number, max: number): number {
  return Math.random() * (max - min) + min;
}

function randomInt(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

function createWinkingStar(width: number, height: number, existingStars: Star[]): WinkingStar | null {
  const maxAttempts = 20;
  for (let i = 0; i < maxAttempts; i++) {
    const x = randomInt(0, width - 1);
    const y = randomInt(0, height - 1);

    const occupied = existingStars.some((s) => s.x === x && s.y === y);
    if (!occupied) {
      return {
        type: 'winking',
        x,
        y,
        progress: 0,
        speed: randomFloat(0.025, 0.06),
        holdDuration: randomFloat(0.3, 0.8),
      };
    }
  }
  return null;
}

function createStaticStar(width: number, height: number, existingStars: Star[]): StaticStar | null {
  const maxAttempts = 30;
  for (let i = 0; i < maxAttempts; i++) {
    const x = randomInt(0, width - 1);
    const y = randomInt(0, height - 1);

    const occupied = existingStars.some((s) => s.x === x && s.y === y);
    if (!occupied) {
      return {
        type: 'static',
        x,
        y,
        char: STATIC_STAR_CHARS[randomInt(0, STATIC_STAR_CHARS.length - 1)],
        baseBrightness: randomFloat(0.2, 0.6),
        pulsePhase: randomFloat(0, Math.PI * 2),
        pulseSpeed: randomFloat(0.04, 0.12),
        pulseAmount: randomFloat(0.05, 0.2),
      };
    }
  }
  return null;
}

function createMessageCloud(text: string, width: number, skyHeight: number): MessageCloud {
  const startX = randomInt(5, Math.max(6, width - text.length - 10));
  const startY = randomInt(2, Math.max(3, Math.floor(skyHeight / 2)));

  return {
    text,
    x: startX,
    y: startY,
    targetX: startX + randomInt(-5, 5),
    targetY: startY + randomInt(-2, 2),
    progress: 0,
    speed: 0.012,
  };
}

function generateCacti(width: number): Cactus[] {
  const cacti: Cactus[] = [];
  let pos = randomInt(15, 30);

  while (pos < width - 15) {
    // Only use the tall saguaro templates (first 3)
    const templateIndex = randomInt(0, 2);
    const template = CACTUS_TEMPLATES[templateIndex];
    const colorIndex = randomInt(0, CACTUS_COLORS.length - 1);
    // Random vertical offset: some cacti sit higher, some lower
    const yOffset = randomInt(-1, 1);
    cacti.push({ x: pos, yOffset, template, colorIndex });
    pos += randomInt(25, 45);
  }

  return cacti;
}

function generateGround(width: number, height: number): string[][] {
  const rows: string[][] = [];

  for (let y = 0; y < height; y++) {
    const row: string[] = [];
    for (let x = 0; x < width; x++) {
      if (y === GROUND_LINE_Y) {
        // Ground line - simple texture
        const r = Math.random();
        row.push(r < 0.6 ? '~' : r < 0.8 ? '-' : r < 0.95 ? '_' : '.');
      } else if (y > GROUND_LINE_Y) {
        // Below ground line - sparse sand specs
        const r = Math.random();
        row.push(r < 0.03 ? '.' : r < 0.05 ? ',' : ' ');
      } else {
        // Above ground line (cactus area)
        row.push(' ');
      }
    }
    rows.push(row);
  }

  return rows;
}

function initializeStars(width: number, skyHeight: number): Star[] {
  const staticCount = randomInt(STATIC_STAR_COUNT.min, STATIC_STAR_COUNT.max);
  const stars: Star[] = [];
  for (let i = 0; i < staticCount; i++) {
    const star = createStaticStar(width, skyHeight, stars);
    if (star) stars.push(star);
  }
  return stars;
}

interface NightSkyViewProps {
  context?: NightSkyContext;
  onExit: () => void;
}

export function NightSkyView({ context, onExit }: NightSkyViewProps) {
  const { stdout } = useStdout();
  const width = stdout?.columns || 80;
  const height = (stdout?.rows || 24) - 1;
  const skyHeight = height - LANDSCAPE_HEIGHT;

  // Extract context info
  const contextName = useMemo(() => {
    if (!context || context.type === 'global') return null;
    switch (context.type) {
      case 'project': return context.project?.name || null;
      case 'livestock': return context.livestock?.name || null;
      case 'barn': return context.barn?.name || null;
      case 'critter': return context.critter?.name || null;
    }
  }, [context]);

  const contextColor = useMemo(() => {
    if (!context || context.type === 'global') return '#FFFFFF';
    switch (context.type) {
      case 'project': return context.project?.color || '#d4a020';
      case 'livestock': return context.project?.color || '#b8860b';
      case 'barn': return '#8fbc8f';
      case 'critter': return '#8fbc8f';
    }
  }, [context]);

  // Stable entity keys for positioning (doesn't change when status text changes)
  // Uses unique IDs for sessions (index-name) to handle multiple sessions with same name
  const entityKeys = useMemo(() => {
    const keys: Array<{ type: 'livestock' | 'critter' | 'session'; name: string; id: string; color: string }> = [];

    // Session keys - use index for unique ID (handles multiple sessions with same name)
    if (context?.sessions) {
      for (const session of context.sessions) {
        keys.push({
          type: 'session',
          name: session.name,
          id: `session-${session.index}`,  // Unique by window index
          color: TUMBLEWEED_COLOR,
        });
      }
    }

    if (!context || context.type === 'global') return keys;

    switch (context.type) {
      case 'project':
        for (const ls of context.project?.livestock || []) {
          keys.push({ type: 'livestock', name: ls.name, id: `livestock-${ls.name}`, color: context.project?.color || '#b8860b' });
        }
        break;
      case 'livestock':
        if (context.livestock) {
          keys.push({ type: 'livestock', name: context.livestock.name, id: `livestock-${context.livestock.name}`, color: context.project?.color || '#b8860b' });
        }
        break;
      case 'barn':
        for (const cr of context.barn?.critters || []) {
          keys.push({ type: 'critter', name: cr.name, id: `critter-${cr.name}`, color: '#8fbc8f' });
        }
        break;
      case 'critter':
        if (context.critter) {
          keys.push({ type: 'critter', name: context.critter.name, id: `critter-${context.critter.name}`, color: '#8fbc8f' });
        }
        break;
    }

    return keys;
  // Only depend on structural changes, not session status updates
  }, [context?.type, context?.project?.name, context?.livestock?.name, context?.barn?.name, context?.critter?.name,
      context?.project?.livestock?.map(l => l.name).join(','),
      context?.barn?.critters?.map(c => c.name).join(','),
      context?.sessions?.map(s => `${s.index}:${s.name}`).join(',')]);

  // Get current session status - keyed by unique session ID
  // This changes when sessions update, which will re-render the landscape
  const sessionStatusMap = useMemo(() => {
    const map = new Map<string, string>();
    if (context?.sessions) {
      for (const session of context.sessions) {
        // Truncate status to keep it short and sweet (~8 chars after icon)
        const shortStatus = session.statusText.slice(0, 8);
        // Key by unique session ID (session-{index})
        map.set(`session-${session.index}`, `${session.statusIcon} ${shortStatus}`);
      }
    }
    return map;
  }, [context?.sessions]);

  // Single state object for all animated elements
  const [state, setState] = useState<AnimationState>(() => {
    const initialStars = initializeStars(width, skyHeight);

    // Generate constellation if we have context
    let constellationStars: ConstellationStar[] = [];
    if (contextName) {
      // Center the constellation in the upper portion of the sky
      const textWidth = contextName.length * 6;
      const startX = Math.floor((width - textWidth) / 2);
      const startY = Math.floor(skyHeight * 0.15); // Upper 15% of sky
      constellationStars = generateConstellation(contextName, startX, startY);
    }

    return {
      stars: initialStars,
      clouds: [],
      constellationStars,
    };
  });
  const [isPaused, setIsPaused] = useState(false);

  const dimsRef = useRef({ width, skyHeight });
  dimsRef.current = { width, skyHeight };

  // Generate landscape elements once
  const groundBase = useMemo(() => generateGround(width, LANDSCAPE_HEIGHT), [width]);

  // Place ground entities FIRST (they have priority), then fill gaps with cacti
  // Only recalculates when entity structure changes, not when session status changes
  const { entityPositions, cacti } = useMemo(() => {
    // Convert entityKeys to the format expected by placeGroundEntities
    const entitiesToPlace = entityKeys.map(key => ({
      ...key,
      statusText: key.type === 'session' ? '            ' : undefined, // Shorter placeholder for truncated status
    }));
    // Place entities first (pass count for vertical spread decision)
    const { placed, usedRanges } = placeGroundEntities(entitiesToPlace, width, entityKeys.length);
    // Then generate cacti in the remaining space
    const generatedCacti = generateCactiAroundEntities(width, usedRanges);
    return { entityPositions: placed, cacti: generatedCacti };
  }, [entityKeys, width]);

  // Reinitialize stars when dimensions change
  useEffect(() => {
    setState(prev => ({
      ...prev,
      stars: initializeStars(width, skyHeight),
    }));
  }, [width, skyHeight]);

  const randomize = useCallback(() => {
    const { width, skyHeight } = dimsRef.current;

    // Regenerate constellation if we have context
    let constellationStars: ConstellationStar[] = [];
    if (contextName) {
      const textWidth = contextName.length * 6;
      const startX = Math.floor((width - textWidth) / 2);
      const startY = Math.floor(skyHeight * 0.15);
      constellationStars = generateConstellation(contextName, startX, startY);
    }

    setState({
      stars: initializeStars(width, skyHeight),
      clouds: [],
      constellationStars,
    });
  }, [contextName]);

  const spawnCloud = useCallback(() => {
    const { width, skyHeight } = dimsRef.current;
    const message = TEST_MESSAGES[randomInt(0, TEST_MESSAGES.length - 1)];
    const cloud = createMessageCloud(message, width, skyHeight);
    setState(prev => ({
      ...prev,
      clouds: [...prev.clouds, cloud],
    }));
  }, []);

  useInput((input, key) => {
    if (key.escape) {
      onExit();
    } else if (input === 'r') {
      randomize();
    } else if (input === 'c') {
      spawnCloud();
    } else if (input === ' ') {
      setIsPaused(p => !p);
    }
  });

  // Single animation loop with batched state update
  useEffect(() => {
    if (isPaused) return;

    const interval = setInterval(() => {
      const { width, skyHeight } = dimsRef.current;

      setState(prev => {
        // Update stars
        const updatedStars: Star[] = [];
        for (const star of prev.stars) {
          if (star.type === 'static') {
            updatedStars.push({
              ...star,
              pulsePhase: star.pulsePhase + star.pulseSpeed,
            });
          } else {
            const newProgress = star.progress + star.speed;
            if (newProgress < 2 + star.holdDuration) {
              updatedStars.push({ ...star, progress: newProgress });
            }
          }
        }

        // Spawn new winking stars
        const winkingCount = updatedStars.filter(s => s.type === 'winking').length;
        if (winkingCount < TARGET_WINKING_STARS.min && Math.random() < SPAWN_CHANCE) {
          const newStar = createWinkingStar(width, skyHeight, updatedStars);
          if (newStar) updatedStars.push(newStar);
        }

        // Update clouds
        const updatedClouds = prev.clouds
          .map(cloud => {
            const newProgress = cloud.progress + cloud.speed;
            let newX = cloud.x;
            let newY = cloud.y;
            if (newProgress > 0.5 && newProgress < 2.5) {
              newX += (cloud.targetX - cloud.x) * 0.02;
              newY += (cloud.targetY - cloud.y) * 0.02;
            }
            return { ...cloud, x: newX, y: newY, progress: newProgress };
          })
          .filter(cloud => cloud.progress < 3);

        // Update constellation stars (pulsing)
        const updatedConstellation = prev.constellationStars.map(star => ({
          ...star,
          pulsePhase: star.pulsePhase + star.pulseSpeed,
        }));

        return {
          stars: updatedStars,
          clouds: updatedClouds,
          constellationStars: updatedConstellation,
        };
      });
    }, FRAME_INTERVAL);

    return () => clearInterval(interval);
  }, [isPaused]);

  // Memoized rendering helpers to ensure stable references
  const getWinkingStarDisplay = useCallback((star: WinkingStar): { char: string; dim: boolean } => {
    const { progress, holdDuration } = star;
    let brightness: number;
    if (progress < 1) {
      brightness = easeInOutCubic(progress);
    } else if (progress < 1 + holdDuration) {
      brightness = 1;
    } else {
      brightness = 1 - easeInOutCubic(progress - 1 - holdDuration);
    }
    const chars = [' ', '.', '·', '+', '*'];
    return { char: chars[Math.round(brightness * 4)], dim: brightness < 0.5 };
  }, []);

  const getStaticStarDisplay = useCallback((star: StaticStar): { char: string; dim: boolean } => {
    const brightness = star.baseBrightness + Math.sin(star.pulsePhase) * star.pulseAmount;
    return { char: star.char, dim: brightness < 0.4 };
  }, []);

  // Cloud display - simple opacity-based fading
  const getCloudOpacity = useCallback((cloud: MessageCloud): { dim: boolean; visible: boolean } => {
    const { progress } = cloud;

    if (progress < 0.3) {
      // Fading in - dim
      return { dim: true, visible: true };
    } else if (progress < 2.7) {
      // Fully visible
      return { dim: false, visible: true };
    } else {
      // Fading out - dim
      return { dim: true, visible: true };
    }
  }, []);

  // Memoized sky row rendering to reduce re-render overhead
  const skyRows = useMemo(() => {
    const rows: React.ReactNode[] = [];
    for (let y = 0; y < skyHeight; y++) {
      const rowChars = ' '.repeat(width).split('');
      const rowDims = new Array(width).fill(false);
      const rowColors: (string | undefined)[] = new Array(width).fill(undefined);

      // Place stars
      for (const star of state.stars) {
        if (star.y === y && star.x >= 0 && star.x < width) {
          const display = star.type === 'winking' ? getWinkingStarDisplay(star) : getStaticStarDisplay(star);
          if (display.char !== ' ') {
            rowChars[star.x] = display.char;
            rowDims[star.x] = display.dim;
          }
        }
      }

      // Place constellation stars (with context color)
      for (const star of state.constellationStars) {
        if (star.y === y && star.x >= 0 && star.x < width) {
          const brightness = star.baseBrightness + Math.sin(star.pulsePhase) * star.pulseAmount;
          rowChars[star.x] = star.char;
          rowDims[star.x] = brightness < 0.5;
          rowColors[star.x] = contextColor;
        }
      }

      // Place clouds
      for (const cloud of state.clouds) {
        const display = getCloudOpacity(cloud);
        if (!display.visible) continue;

        const cloudY = Math.round(cloud.y);
        const cloudX = Math.round(cloud.x);

        // Simple box cloud
        const textLen = cloud.text.length + 2;
        const lines = [
          '╭' + '─'.repeat(textLen) + '╮',
          '│ ' + cloud.text + ' │',
          '╰' + '─'.repeat(textLen) + '╯',
        ];

        const lineIdx = y - cloudY;
        if (lineIdx >= 0 && lineIdx < 3) {
          const line = lines[lineIdx];
          for (let lx = 0; lx < line.length; lx++) {
            const cellX = cloudX + lx;
            if (cellX >= 0 && cellX < width) {
              rowChars[cellX] = line[lx];
              rowDims[cellX] = display.dim;
              rowColors[cellX] = '#87CEEB';
            }
          }
        }
      }

      // Build segments for efficient rendering - join consecutive same-style chars
      const segments: Array<{ text: string; dim: boolean; color?: string }> = [];
      let seg = { text: rowChars[0], dim: rowDims[0], color: rowColors[0] };
      for (let x = 1; x < width; x++) {
        if (rowDims[x] === seg.dim && rowColors[x] === seg.color) {
          seg.text += rowChars[x];
        } else {
          segments.push(seg);
          seg = { text: rowChars[x], dim: rowDims[x], color: rowColors[x] };
        }
      }
      segments.push(seg);

      rows.push(
        <Text key={y}>
          {segments.map((s, i) => (
            <Text key={`${y}-${i}`} color={s.color || 'white'} dimColor={s.dim}>{s.text}</Text>
          ))}
        </Text>
      );
    }
    return rows;
  }, [state.stars, state.clouds, state.constellationStars, width, skyHeight, contextColor, getWinkingStarDisplay, getStaticStarDisplay, getCloudOpacity]);

  // Landscape rendering - builds grid and renders on each frame
  // Uses stable entity positions but reads current session status from ref
  // This runs on every animation frame but is efficient (simple grid operations)
  const renderLandscape = useCallback(() => {
    // Create ground grid from base (fresh copy each render)
    const ground = groundBase.map(row => [...row]);

    // Track entity colors for each cell
    const entityColors: (string | undefined)[][] = ground.map(row => new Array(row.length).fill(undefined));

    // Place cacti first (they go behind entities)
    for (const cactus of cacti) {
      const cactusHeight = cactus.template.length;
      const cactusStartY = GROUND_LINE_Y - cactusHeight + 1 + cactus.yOffset;
      for (let cy = 0; cy < cactusHeight; cy++) {
        const groundY = cactusStartY + cy;
        if (groundY >= 0 && groundY < LANDSCAPE_HEIGHT) {
          const line = cactus.template[cy];
          for (let cx = 0; cx < line.length; cx++) {
            const groundX = cactus.x + cx;
            if (groundX >= 0 && groundX < width && line[cx] !== ' ') {
              ground[groundY][groundX] = line[cx];
            }
          }
        }
      }
    }

    // Place ground entities (livestock/critters/sessions)
    for (const entity of entityPositions) {
      // Get the art - for sessions, inject current status
      let art: string[];
      if (entity.type === 'session') {
        const currentStatus = sessionStatusMap.get(entity.id) || '';
        art = processAsciiArt(GROUND_TUMBLEWEED, entity.name, currentStatus);
      } else {
        art = entity.art;
      }

      const entityHeight = art.length;
      const entityStartY = GROUND_LINE_Y - entityHeight + 1 + entity.yOffset;

      for (let ey = 0; ey < entityHeight; ey++) {
        const groundY = entityStartY + ey;
        if (groundY >= 0 && groundY < LANDSCAPE_HEIGHT) {
          const line = art[ey];
          for (let ex = 0; ex < line.length; ex++) {
            const groundX = entity.x + ex;
            if (groundX >= 0 && groundX < width && line[ex] !== ' ') {
              ground[groundY][groundX] = line[ex];
              entityColors[groundY][groundX] = entity.color;
            }
          }
        }
      }
    }

    // Render ground rows with coloring
    const rows: React.ReactNode[] = [];
    for (let y = 0; y < LANDSCAPE_HEIGHT; y++) {
      const segments: Array<{ text: string; color: string; dim: boolean }> = [];
      let currentColor = '#C2B280';
      let currentDim = false;
      let currentText = '';

      for (let x = 0; x < width; x++) {
        const char = ground[y][x];
        let charColor = entityColors[y][x] || '#C2B280';
        let charDim = y > GROUND_LINE_Y && !entityColors[y][x];

        // Check if this char is part of a cactus (█ character) and not an entity
        if (char === '█' && !entityColors[y][x]) {
          for (const cactus of cacti) {
            const cactusHeight = cactus.template.length;
            const cactusStartY = GROUND_LINE_Y - cactusHeight + 1 + cactus.yOffset;
            const relY = y - cactusStartY;
            const relX = x - cactus.x;

            if (relY >= 0 && relY < cactusHeight && relX >= 0 && relX < cactus.template[relY].length) {
              if (cactus.template[relY][relX] === '█') {
                const gradientIndex = Math.min(
                  CACTUS_COLORS.length - 1,
                  Math.floor(relY / cactusHeight * CACTUS_COLORS.length)
                );
                charColor = CACTUS_COLORS[gradientIndex];
                charDim = false;
                break;
              }
            }
          }
        }

        if (charColor === currentColor && charDim === currentDim) {
          currentText += char;
        } else {
          if (currentText) segments.push({ text: currentText, color: currentColor, dim: currentDim });
          currentText = char;
          currentColor = charColor;
          currentDim = charDim;
        }
      }
      if (currentText) segments.push({ text: currentText, color: currentColor, dim: currentDim });

      rows.push(
        <Text key={`land-${y}`}>
          {segments.map((s, i) => (
            <Text key={`land-${y}-${i}`} color={s.color} dimColor={s.dim}>{s.text}</Text>
          ))}
        </Text>
      );
    }

    return rows;
  }, [cacti, groundBase, width, entityPositions, sessionStatusMap]);

  // Memoized landscape rendering - only recalculates when structure or status changes
  const landscapeRows = useMemo(() => renderLandscape(), [renderLandscape]);

  return (
    <Box flexDirection="column" height={height}>
      <Box flexDirection="column">{skyRows}</Box>
      <Box flexDirection="column">{landscapeRows}</Box>
    </Box>
  );
}
