import React from 'react';
import { Box, Text } from 'ink';
import { getHotkeysGrouped, type HotkeyScope, type Hotkey } from '../lib/hotkeys.js';

// Yeehaw brand gold (darker for light mode readability)
const BRAND_COLOR = '#d4a020';

interface HelpOverlayProps {
  scope: HotkeyScope;
  focusedPanel?: string;
}

function HotkeyRow({ hotkey }: { hotkey: Hotkey }) {
  return (
    <Box gap={2}>
      <Box width={12}>
        <Text color={BRAND_COLOR}>{hotkey.key}</Text>
      </Box>
      <Text>{hotkey.description}</Text>
    </Box>
  );
}

function HotkeySection({ title, hotkeys }: { title: string; hotkeys: Hotkey[] }) {
  if (hotkeys.length === 0) return null;

  return (
    <Box flexDirection="column" marginTop={1}>
      <Text bold dimColor>{title}</Text>
      {hotkeys.map((h, i) => (
        <HotkeyRow key={`${h.key}-${i}`} hotkey={h} />
      ))}
    </Box>
  );
}

export function HelpOverlay({ scope, focusedPanel }: HelpOverlayProps) {
  const grouped = getHotkeysGrouped(scope, focusedPanel);

  // Also include list navigation if we're in a view with lists
  const listHotkeys = scope !== 'global' ? getHotkeysGrouped('list').navigation : [];

  return (
    <Box
      flexDirection="column"
      borderStyle="double"
      borderColor={BRAND_COLOR}
      paddingX={2}
      paddingY={1}
    >
      <Text bold color={BRAND_COLOR}>Keyboard Shortcuts</Text>

      <HotkeySection title="Navigation" hotkeys={[...grouped.navigation, ...listHotkeys]} />
      <HotkeySection title="Actions" hotkeys={grouped.action} />
      <HotkeySection title="System" hotkeys={grouped.system} />

      <Box marginTop={1}>
        <Text dimColor>Press ? to close</Text>
      </Box>
    </Box>
  );
}
