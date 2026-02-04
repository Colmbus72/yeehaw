import React from 'react';
import { Box, Text } from 'ink';

interface Shortcut {
  key: string;
  label: string;
}

const globalShortcuts: Shortcut[] = [
  { key: 'Enter', label: 'select' },
  { key: 'Tab', label: 'switch panel' },
  { key: 'n', label: 'new project' },
  { key: 'c', label: 'claude' },
  { key: '?', label: 'help' },
];

const projectShortcuts: Shortcut[] = [
  { key: 'Enter', label: 'shell' },
  { key: 'c', label: 'claude' },
  { key: 'Tab', label: 'switch panel' },
  { key: '?', label: 'help' },
  { key: 'Esc', label: 'back' },
];

interface StatusBarProps {
  view: 'global' | 'project';
}

export function StatusBar({ view }: StatusBarProps) {
  const shortcuts = view === 'global' ? globalShortcuts : projectShortcuts;

  return (
    <Box
      borderStyle="single"
      borderColor="gray"
      paddingX={1}
      justifyContent="space-between"
    >
      <Box gap={2}>
        {shortcuts.map((shortcut) => (
          <Text key={shortcut.key}>
            <Text color="cyan">[{shortcut.key}]</Text>
            <Text dimColor>{shortcut.label}</Text>
          </Text>
        ))}
      </Box>
      {view === 'global' ? (
        <Box gap={2}>
          <Text>
            <Text color="yellow">q</Text>
            <Text dimColor>:detach</Text>
          </Text>
          <Text>
            <Text color="red">Q</Text>
            <Text dimColor>:quit all</Text>
          </Text>
        </Box>
      ) : (
        <Text>
          <Text color="cyan">q</Text>
          <Text dimColor>:back</Text>
        </Text>
      )}
    </Box>
  );
}
