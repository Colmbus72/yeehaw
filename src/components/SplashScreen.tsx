import React, { useState, useEffect, useMemo } from 'react';
import { Box, Text, useStdout } from 'ink';
import figlet from 'figlet';

// Tumbleweed ASCII art - same as in Header.tsx
const TUMBLEWEED = [
  ' ░ ░▒░ ░▒░',
  '░▒ · ‿ · ▒░',
  '▒░ ▒░▒░ ░▒',
  ' ░▒░ ░▒░ ░',
];

const TUMBLEWEED_COLOR = '#b8860b';
const BRAND_COLOR = '#d4a020';  // Darker for light mode readability

// Match Header.tsx positioning exactly
const HEADER_PADDING_TOP = 1;
const TUMBLEWEED_TOP_PADDING = 1;
const HEADER_PADDING_LEFT = 2;
const TUMBLEWEED_WIDTH = 11;
const TITLE_OFFSET_LEFT = HEADER_PADDING_LEFT + TUMBLEWEED_WIDTH + 2;

// Gradient color helpers (matching Header.tsx)
function hexToRgb(hex: string): { r: number; g: number; b: number } | null {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  return result
    ? { r: parseInt(result[1], 16), g: parseInt(result[2], 16), b: parseInt(result[3], 16) }
    : null;
}

function interpolateColor(
  c1: { r: number; g: number; b: number },
  c2: { r: number; g: number; b: number },
  factor: number
): string {
  const r = Math.round(c1.r + (c2.r - c1.r) * factor);
  const g = Math.round(c1.g + (c2.g - c1.g) * factor);
  const b = Math.round(c1.b + (c2.b - c1.b) * factor);
  return `rgb(${r},${g},${b})`;
}

function getGradientColor(row: number, totalRows: number): string {
  const rgb = hexToRgb(BRAND_COLOR);
  if (!rgb) return BRAND_COLOR;
  const startRgb = rgb;
  const endRgb = { r: Math.round(rgb.r * 0.3), g: Math.round(rgb.g * 0.3), b: Math.round(rgb.b * 0.3) };
  const factor = row / Math.max(totalRows - 1, 1);
  return interpolateColor(startRgb, endRgb, factor);
}

type Phase = 'build' | 'pulse' | 'done';

interface Dot {
  row: number;
  col: number;
  distance: number;
  gradientRow: number;
  totalRows: number;
}

interface SplashScreenProps {
  onComplete: () => void;
}

export function SplashScreen({ onComplete }: SplashScreenProps) {
  const { stdout } = useStdout();
  const terminalHeight = stdout?.rows || 24;
  const terminalWidth = stdout?.columns || 80;

  const [phase, setPhase] = useState<Phase>('build');
  const [visibleCount, setVisibleCount] = useState(0);
  const [waveDistance, setWaveDistance] = useState(0);

  const waveOriginRow = HEADER_PADDING_TOP + TUMBLEWEED_TOP_PADDING + 2;
  const waveOriginCol = HEADER_PADDING_LEFT + 5;

  // Generate title dots from figlet
  const allDots = useMemo(() => {
    const dots: Dot[] = [];

    try {
      const ascii = figlet.textSync('YEEHAW', { font: 'ANSI Shadow' });
      const lines = ascii.split('\n').filter(line => line.trim() !== '');
      const totalRows = lines.length;

      lines.forEach((line, row) => {
        for (let col = 0; col < line.length; col++) {
          const char = line[col];
          if (char !== ' ') {
            const screenRow = HEADER_PADDING_TOP + row;
            const screenCol = TITLE_OFFSET_LEFT + col;
            const distance = Math.sqrt(
              Math.pow(screenRow - waveOriginRow, 2) +
              Math.pow((screenCol - waveOriginCol) * 0.5, 2)
            );
            dots.push({ row: screenRow, col: screenCol, distance, gradientRow: row, totalRows });
          }
        }
      });
    } catch {
      // Fallback if figlet fails
    }

    return dots;
  }, [waveOriginRow, waveOriginCol]);

  const maxDistance = useMemo(() => {
    if (allDots.length === 0) return 100;
    return Math.max(...allDots.map((d) => d.distance)) + 5;
  }, [allDots]);

  // Flatten tumbleweed into array of { char, row, col } for animation
  const allChars = useMemo(() => {
    const chars: Array<{ char: string; row: number; col: number }> = [];
    TUMBLEWEED.forEach((line, row) => {
      for (let col = 0; col < line.length; col++) {
        const char = line[col];
        if (char !== ' ') {
          chars.push({ char, row, col });
        }
      }
    });
    return chars;
  }, []);

  // Shuffle the characters for random build order
  const shuffledChars = useMemo(
    () => [...allChars].sort(() => Math.random() - 0.5),
    [allChars]
  );

  useEffect(() => {
    if (phase === 'build') {
      if (visibleCount < shuffledChars.length) {
        const timer = setTimeout(() => {
          setVisibleCount((c) => Math.min(c + 2, shuffledChars.length));
        }, 30);
        return () => clearTimeout(timer);
      } else {
        // Tumbleweed complete, start pulse
        const timer = setTimeout(() => setPhase('pulse'), 100);
        return () => clearTimeout(timer);
      }
    } else if (phase === 'pulse') {
      if (waveDistance < maxDistance) {
        // Advance the wave
        const timer = setTimeout(() => {
          setWaveDistance((d) => d + 3);
        }, 20);
        return () => clearTimeout(timer);
      } else {
        // Wave complete
        const timer = setTimeout(onComplete, 200);
        return () => clearTimeout(timer);
      }
    }
  }, [phase, visibleCount, waveDistance, shuffledChars.length, maxDistance, onComplete]);

  // Build the current visible state of the tumbleweed
  const visibleSet = new Set(
    shuffledChars.slice(0, visibleCount).map((c) => `${c.row},${c.col}`)
  );

  const renderedTumbleweed = TUMBLEWEED.map((line, row) => {
    let result = '';
    for (let col = 0; col < line.length; col++) {
      const char = line[col];
      if (char === ' ' || visibleSet.has(`${row},${col}`)) {
        result += char;
      } else {
        result += ' ';
      }
    }
    return result;
  });

  const topPadding = HEADER_PADDING_TOP + TUMBLEWEED_TOP_PADDING;

  // Render the wave revealing dots
  const renderWave = () => {
    const lines: React.ReactNode[] = [];
    const waveWidth = 8;

    // Group dots by row
    const dotsByRow = new Map<number, Dot[]>();
    allDots.forEach((dot) => {
      if (!dotsByRow.has(dot.row)) {
        dotsByRow.set(dot.row, []);
      }
      dotsByRow.get(dot.row)!.push(dot);
    });

    for (let row = 0; row < terminalHeight; row++) {
      const rowDots = dotsByRow.get(row) || [];

      if (rowDots.length === 0) {
        lines.push(<Text key={row}>{' '.repeat(terminalWidth)}</Text>);
        continue;
      }

      rowDots.sort((a, b) => a.col - b.col);

      const segments: React.ReactNode[] = [];
      let lastCol = 0;

      for (const dot of rowDots) {
        if (dot.distance > waveDistance) continue;

        if (dot.col > lastCol) {
          segments.push(
            <Text key={`space-${lastCol}`}>{' '.repeat(dot.col - lastCol)}</Text>
          );
        }

        const atWaveFront = dot.distance >= waveDistance - waveWidth;
        const color = getGradientColor(dot.gradientRow, dot.totalRows);

        segments.push(
          <Text key={`dot-${dot.col}`} color={color} bold={atWaveFront}>
            ·
          </Text>
        );

        lastCol = dot.col + 1;
      }

      lines.push(<Box key={row}>{segments}</Box>);
    }

    return lines;
  };

  return (
    <Box flexDirection="column" height={terminalHeight}>
      {/* Wave layer - reveals YEEHAW title */}
      {phase === 'pulse' && (
        <Box position="absolute" flexDirection="column">
          {renderWave()}
        </Box>
      )}

      {/* Tumbleweed layer */}
      <Box flexDirection="column" paddingTop={topPadding} paddingLeft={HEADER_PADDING_LEFT}>
        {renderedTumbleweed.map((line, i) => (
          <Text key={i} color={TUMBLEWEED_COLOR} bold>
            {line}
          </Text>
        ))}
      </Box>
    </Box>
  );
}
