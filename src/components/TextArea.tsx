import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';

interface TextAreaProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit?: (value: string) => void;
  placeholder?: string;
  height?: number;
}

/**
 * Multiline text input component.
 * - Enter: add newline
 * - Ctrl+S: save/submit
 * - Ctrl+D: also save/submit (alternative)
 * - Backspace: delete character
 * - Arrow keys: navigate
 */
export function TextArea({
  value,
  onChange,
  onSubmit,
  placeholder,
  height = 8,
}: TextAreaProps) {
  const [cursorPos, setCursorPos] = useState(value.length);
  const [scrollOffset, setScrollOffset] = useState(0);

  // Keep cursor in bounds when value changes
  useEffect(() => {
    if (cursorPos > value.length) {
      setCursorPos(value.length);
    }
  }, [value, cursorPos]);

  // Calculate cursor line for auto-scroll
  const getCursorLine = () => {
    const beforeCursor = value.slice(0, cursorPos);
    return (beforeCursor.match(/\n/g) || []).length;
  };

  // Auto-scroll to keep cursor visible
  useEffect(() => {
    const cursorLine = getCursorLine();
    const visibleLines = height - 2;

    if (cursorLine < scrollOffset) {
      setScrollOffset(cursorLine);
    } else if (cursorLine >= scrollOffset + visibleLines) {
      setScrollOffset(cursorLine - visibleLines + 1);
    }
  }, [cursorPos, value, height]);

  useInput((input, key) => {
    // Ctrl+S or Ctrl+D to submit
    if ((key.ctrl && input === 's') || (key.ctrl && input === 'd')) {
      onSubmit?.(value);
      return;
    }

    // Enter adds newline
    if (key.return) {
      const newValue = value.slice(0, cursorPos) + '\n' + value.slice(cursorPos);
      onChange(newValue);
      setCursorPos(cursorPos + 1);
      return;
    }

    // Backspace
    if (key.backspace || key.delete) {
      if (cursorPos > 0) {
        const newValue = value.slice(0, cursorPos - 1) + value.slice(cursorPos);
        onChange(newValue);
        setCursorPos(cursorPos - 1);
      }
      return;
    }

    // Arrow keys for cursor movement
    if (key.leftArrow) {
      setCursorPos(Math.max(0, cursorPos - 1));
      return;
    }
    if (key.rightArrow) {
      setCursorPos(Math.min(value.length, cursorPos + 1));
      return;
    }
    if (key.upArrow) {
      const beforeCursor = value.slice(0, cursorPos);
      const currentLineStart = beforeCursor.lastIndexOf('\n') + 1;
      if (currentLineStart > 0) {
        const posInLine = cursorPos - currentLineStart;
        const prevLineEnd = currentLineStart - 1;
        const prevLineStart = value.lastIndexOf('\n', prevLineEnd - 1) + 1;
        const prevLineLength = prevLineEnd - prevLineStart;
        const newPos = prevLineStart + Math.min(posInLine, prevLineLength);
        setCursorPos(newPos);
      }
      return;
    }
    if (key.downArrow) {
      const nextLineStart = value.indexOf('\n', cursorPos);
      if (nextLineStart !== -1) {
        const currentLineStart = value.lastIndexOf('\n', cursorPos - 1) + 1;
        const posInLine = cursorPos - currentLineStart;
        const nextLineEnd = value.indexOf('\n', nextLineStart + 1);
        const nextLineLength = (nextLineEnd === -1 ? value.length : nextLineEnd) - nextLineStart - 1;
        const newPos = nextLineStart + 1 + Math.min(posInLine, nextLineLength);
        setCursorPos(newPos);
      }
      return;
    }

    // Page up/down for scrolling
    if (key.pageUp) {
      setScrollOffset(Math.max(0, scrollOffset - (height - 2)));
      return;
    }
    if (key.pageDown) {
      const totalLines = (value.match(/\n/g) || []).length + 1;
      setScrollOffset(Math.min(totalLines - 1, scrollOffset + (height - 2)));
      return;
    }

    // Regular character input
    if (input && !key.ctrl && !key.meta && input.charCodeAt(0) >= 32) {
      const newValue = value.slice(0, cursorPos) + input + value.slice(cursorPos);
      onChange(newValue);
      setCursorPos(cursorPos + input.length);
    }
  });

  // Render
  const displayValue = value || '';
  const showPlaceholder = !value && placeholder;
  const lines = (showPlaceholder ? placeholder : displayValue).split('\n');
  const totalLines = lines.length;
  const visibleLines = height - 2;

  // Calculate cursor position
  let cursorLine = 0;
  let cursorCol = 0;
  if (!showPlaceholder && value) {
    const beforeCursor = value.slice(0, cursorPos);
    cursorLine = (beforeCursor.match(/\n/g) || []).length;
    const lastNewline = beforeCursor.lastIndexOf('\n');
    cursorCol = lastNewline === -1 ? cursorPos : cursorPos - lastNewline - 1;
  }

  // Get visible lines with scroll
  const visibleStart = scrollOffset;
  const visibleEnd = Math.min(scrollOffset + visibleLines, totalLines);
  const displayLines = lines.slice(visibleStart, visibleEnd);

  return (
    <Box flexDirection="column" borderStyle="single" borderColor="gray" padding={1} height={height}>
      {displayLines.map((line, displayIdx) => {
        const actualLineIdx = visibleStart + displayIdx;
        const isCursorLine = actualLineIdx === cursorLine && !showPlaceholder;

        if (isCursorLine) {
          const before = line.slice(0, cursorCol);
          const cursorChar = line[cursorCol] || ' ';
          const after = line.slice(cursorCol + 1);

          return (
            <Box key={actualLineIdx}>
              <Text>
                {before}
                <Text inverse>{cursorChar}</Text>
                {after}
              </Text>
            </Box>
          );
        }

        return (
          <Box key={actualLineIdx}>
            <Text dimColor={showPlaceholder ? true : undefined}>
              {line || ' '}
            </Text>
          </Box>
        );
      })}
      {/* Scroll indicator */}
      {totalLines > visibleLines && (
        <Box justifyContent="flex-end">
          <Text dimColor>
            [{visibleStart + 1}-{visibleEnd}/{totalLines}]
          </Text>
        </Box>
      )}
    </Box>
  );
}
