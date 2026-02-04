import React from 'react';
import { Text } from 'ink';
import { marked } from 'marked';
// @ts-ignore - no types available
import TerminalRenderer from 'marked-terminal';

interface MarkdownProps {
  children: string;
}

// Configure marked to use terminal renderer
marked.setOptions({
  renderer: new TerminalRenderer({
    // Customize terminal rendering options
    showSectionPrefix: false,
    reflowText: true,
    width: 80,
  }),
});

/**
 * Render markdown content in the terminal with formatting.
 */
export function Markdown({ children }: MarkdownProps) {
  const rendered = marked.parse(children);

  // marked-terminal returns a string with ANSI codes
  // Ink's Text component will render these correctly
  return <Text>{String(rendered).trim()}</Text>;
}
