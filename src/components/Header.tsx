import React, { useMemo } from 'react';
import { Box, Text } from 'ink';
import figlet from 'figlet';

interface HeaderProps {
  text: string;
  subtitle?: string;
  summary?: string;
  color?: string;  // Hex color like "#ff6b6b"
  gradientSpread?: number;    // 0-10, controls gradient intensity (default 5)
  gradientInverted?: boolean; // Flip gradient direction
  theme?: 'dark' | 'light';   // Terminal theme for gradient optimization
  versionInfo?: {
    current: string;
    latest: string | null;
  };
}

// Compare semver versions - returns true if latest > current
function isNewerVersion(latest: string, current: string): boolean {
  const parseVersion = (v: string) => v.split('.').map(n => parseInt(n, 10) || 0);
  const [lMajor, lMinor, lPatch] = parseVersion(latest);
  const [cMajor, cMinor, cPatch] = parseVersion(current);

  if (lMajor > cMajor) return true;
  if (lMajor < cMajor) return false;
  if (lMinor > cMinor) return true;
  if (lMinor < cMinor) return false;
  return lPatch > cPatch;
}

// Convert hex to RGB
function hexToRgb(hex: string): { r: number; g: number; b: number } | null {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  return result
    ? {
        r: parseInt(result[1], 16),
        g: parseInt(result[2], 16),
        b: parseInt(result[3], 16),
      }
    : null;
}

// Interpolate between two colors
function interpolateColor(
  color1: { r: number; g: number; b: number },
  color2: { r: number; g: number; b: number },
  factor: number
): string {
  const r = Math.round(color1.r + (color2.r - color1.r) * factor);
  const g = Math.round(color1.g + (color2.g - color1.g) * factor);
  const b = Math.round(color1.b + (color2.b - color1.b) * factor);
  return `rgb(${r},${g},${b})`;
}

// Generate gradient colors for each line
function generateGradient(
  lines: string[],
  baseColor: string,
  spread: number = 5,
  inverted: boolean = false,
  theme: 'dark' | 'light' = 'dark'
): string[] {
  const rgb = hexToRgb(baseColor);
  if (!rgb) return lines.map(() => baseColor);

  // Spread controls how much the gradient changes (0 = no change, 10 = max change)
  // Convert 0-10 scale to a multiplier (0 = 1.0, 10 = 0.1 for darkening factor)
  const spreadFactor = 1 - (spread / 10) * 0.9; // 0->1.0, 5->0.55, 10->0.1

  // Calculate luminance to detect dark colors
  const luminance = (0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b) / 255;

  let startRgb: { r: number; g: number; b: number };
  let endRgb: { r: number; g: number; b: number };

  // Determine gradient direction based on color luminance and theme
  const isDarkColor = luminance < 0.3;
  const shouldLighten = isDarkColor || theme === 'light';

  if (shouldLighten) {
    // Go from a lighter tint down to the base color (or adjusted end)
    const liftFactor = 1 + (spread / 10) * 2; // More spread = more lift
    startRgb = {
      r: Math.min(255, Math.round(rgb.r * liftFactor + spread * 8)),
      g: Math.min(255, Math.round(rgb.g * liftFactor + spread * 8)),
      b: Math.min(255, Math.round(rgb.b * liftFactor + spread * 8)),
    };
    endRgb = rgb;
  } else {
    // Go from base to a darker version
    startRgb = rgb;
    endRgb = {
      r: Math.round(rgb.r * spreadFactor),
      g: Math.round(rgb.g * spreadFactor),
      b: Math.round(rgb.b * spreadFactor),
    };
  }

  // Invert if requested
  if (inverted) {
    [startRgb, endRgb] = [endRgb, startRgb];
  }

  return lines.map((_, i) => {
    const factor = i / Math.max(lines.length - 1, 1);
    return interpolateColor(startRgb, endRgb, factor);
  });
}

// Tumbleweed mascot art
const TUMBLEWEED = [
  ' ░ ░▒░ ░▒░',
  '░▒ · ‿ · ▒░',
  '▒░ ▒░▒░ ░▒',
  ' ░▒░ ░▒░ ░',
];

// Brownish tan color to complement yeehaw gold
const TUMBLEWEED_COLOR = '#b8860b';

export function Header({ text, subtitle, summary, color, gradientSpread, gradientInverted, theme, versionInfo }: HeaderProps) {
  // Use sync figlet to avoid flash on initial render
  const ascii = useMemo(() => {
    try {
      return figlet.textSync(text.toUpperCase(), { font: 'ANSI Shadow' });
    } catch {
      return text.toUpperCase();
    }
  }, [text]);

  const lines = ascii.split('\n').filter(line => line.trim() !== '');
  const baseColor = color || '#f0c040';  // Default yeehaw gold
  const gradientColors = generateGradient(
    lines,
    baseColor,
    gradientSpread ?? 5,
    gradientInverted ?? false,
    theme ?? 'dark'
  );

  // Show tumbleweed only for the main "yeehaw" title
  const showTumbleweed = text.toLowerCase() === 'yeehaw';
  // Vertically center tumbleweed next to ASCII art
  const tumbleweedTopPadding = Math.max(0, Math.floor((lines.length - TUMBLEWEED.length) / 2));

  return (
    <Box flexDirection="column" paddingTop={1} paddingLeft={2}>
      <Box flexDirection="row">
        {showTumbleweed && (
          <Box flexDirection="column" marginRight={2}>
            {Array(tumbleweedTopPadding).fill(null).map((_, i) => (
              <Text key={`pad-${i}`}> </Text>
            ))}
            {TUMBLEWEED.map((line, i) => (
              <Text key={`tumbleweed-${i}`} color={TUMBLEWEED_COLOR} bold>{line}</Text>
            ))}
          </Box>
        )}
        <Box flexDirection="column">
          {lines.map((line, i) => (
            <Text key={i} color={gradientColors[i]}>
              {line}
            </Text>
          ))}
        </Box>
        {versionInfo && showTumbleweed && (
          <Box marginLeft={2} paddingRight={1} alignItems="flex-end" flexGrow={1} justifyContent="flex-end">
            {versionInfo.latest && isNewerVersion(versionInfo.latest, versionInfo.current) ? (
              <Text>
                <Text dimColor>v{versionInfo.current}</Text>
                <Text dimColor> → </Text>
                <Text color="yellow">v{versionInfo.latest}</Text>
              </Text>
            ) : (
              <Text dimColor>v{versionInfo.current}</Text>
            )}
          </Box>
        )}
      </Box>
      {(subtitle || summary) && (
        <Box gap={2}>
          {subtitle && <Text dimColor>{subtitle}</Text>}
          {summary && <Text color="gray">- {summary}</Text>}
        </Box>
      )}
    </Box>
  );
}
