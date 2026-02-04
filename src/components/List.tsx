import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import type { SessionStatus } from '../lib/signals.js';

// Yeehaw brand gold (darker for light mode readability)
const BRAND_COLOR = '#d4a020';

export interface RowAction {
  key: string;      // e.g., 'c', 's'
  label: string;    // e.g., 'claude', 'shell'
}

export interface ListItem {
  id: string;
  label: string;
  status?: 'active' | 'inactive' | 'error';
  meta?: string;
  sessionStatus?: SessionStatus;  // For session-specific coloring
  actions?: RowAction[];  // Row-level actions for this item
  prefix?: React.ReactNode;  // Optional prefix element (e.g., status icon)
}

interface ListProps {
  items: ListItem[];
  focused?: boolean;
  selectedIndex?: number;  // Controlled selection
  onSelect?: (item: ListItem) => void;
  onAction?: (item: ListItem, action: string) => void;  // action is the key pressed
  onHighlight?: (item: ListItem | null) => void;
  onSelectionChange?: (index: number) => void;  // Called when selection changes
}

export function List({ items, focused = false, selectedIndex: controlledIndex, onSelect, onAction, onHighlight, onSelectionChange }: ListProps) {
  const [internalIndex, setInternalIndex] = useState(0);

  // Use controlled index if provided, otherwise internal
  const selectedIndex = controlledIndex ?? internalIndex;
  const setSelectedIndex = (indexOrFn: number | ((prev: number) => number)) => {
    const newIndex = typeof indexOrFn === 'function' ? indexOrFn(selectedIndex) : indexOrFn;
    if (controlledIndex === undefined) {
      setInternalIndex(newIndex);
    }
    onSelectionChange?.(newIndex);
  };

  useEffect(() => {
    if (items.length > 0 && onHighlight) {
      onHighlight(items[selectedIndex] ?? null);
    }
  }, [selectedIndex, items, onHighlight]);

  useInput((input, key) => {
    if (!focused) return;

    if (input === 'j' || key.downArrow) {
      setSelectedIndex((i) => Math.min(i + 1, items.length - 1));
    }
    if (input === 'k' || key.upArrow) {
      setSelectedIndex((i) => Math.max(i - 1, 0));
    }
    if (input === 'g') {
      setSelectedIndex(0);
    }
    if (input === 'G') {
      setSelectedIndex(items.length - 1);
    }
    if (key.return && items[selectedIndex] && onSelect) {
      onSelect(items[selectedIndex]);
    }
    // Handle row-level actions (replaces hardcoded 's' check)
    const currentItem = items[selectedIndex];
    if (currentItem?.actions && onAction) {
      const action = currentItem.actions.find(a => a.key === input);
      if (action) {
        onAction(currentItem, action.key);
      }
    }
  });

  if (items.length === 0) {
    return <Text dimColor>No items</Text>;
  }

  return (
    <Box flexDirection="column">
      {items.map((item, index) => {
        const isSelected = index === selectedIndex && focused;

        // Session status takes priority for meta coloring
        const sessionStatusColor =
          item.sessionStatus === 'waiting' ? 'yellow' :
          item.sessionStatus === 'working' ? 'cyan' :
          item.sessionStatus === 'error' ? 'red' : undefined;

        const statusColor =
          item.status === 'active' ? 'green' :
          item.status === 'error' ? 'red' : 'gray';

        return (
          <Box key={item.id} justifyContent="space-between">
            {/* Selection indicator - fixed width, never shrinks */}
            <Box flexShrink={0} width={2}>
              <Text color={isSelected ? BRAND_COLOR : undefined}>
                {isSelected ? '›' : ' '}
              </Text>
            </Box>
            {/* Left side: prefix + label + status + meta - shrinks to make room for actions */}
            <Box gap={1} flexShrink={1} flexGrow={1} overflow="hidden">
              {item.prefix && (
                <Box flexShrink={0}>{item.prefix}</Box>
              )}
              <Text color={isSelected ? BRAND_COLOR : undefined} bold={isSelected} wrap="truncate">
                {item.label}
              </Text>
              {item.status && (
                <Text color={statusColor}>●</Text>
              )}
              {item.meta && (
                <Text color={sessionStatusColor} dimColor={!sessionStatusColor} wrap="truncate">{item.meta}</Text>
              )}
            </Box>
            {/* Row actions - only shown on selected item, never shrinks */}
            {isSelected && item.actions && item.actions.length > 0 && (
              <Box gap={2} flexShrink={0} marginLeft={1}>
                {item.actions.map((action) => (
                  <Text key={action.key}>
                    <Text color={BRAND_COLOR}>[{action.key}]</Text>
                    <Text dimColor> {action.label}</Text>
                  </Text>
                ))}
              </Box>
            )}
          </Box>
        );
      })}
    </Box>
  );
}
