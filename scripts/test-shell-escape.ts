#!/usr/bin/env npx tsx

import { shellEscape, shellCommand, shellEscapeDouble } from '../src/lib/shell.js';

function assert(condition: boolean, message: string) {
  if (!condition) {
    console.error(`FAIL: ${message}`);
    process.exit(1);
  }
  console.log(`PASS: ${message}`);
}

// Test shellEscape
assert(shellEscape('simple') === "'simple'", 'simple string');
assert(shellEscape('') === "''", 'empty string');
assert(shellEscape("it's") === "'it'\\''s'", 'string with single quote');
assert(shellEscape('a"b') === "'a\"b'", 'string with double quote');
assert(shellEscape('$HOME') === "'$HOME'", 'string with dollar sign (no expansion)');
assert(shellEscape('`whoami`') === "'`whoami`'", 'string with backticks');
assert(shellEscape('/path/with spaces/file') === "'/path/with spaces/file'", 'path with spaces');
assert(shellEscape("'; rm -rf / #") === "''\\''; rm -rf / #'", 'injection attempt');

// Test shellCommand
assert(
  shellCommand('cat {0}', '/path/to/file') === "cat '/path/to/file'",
  'shellCommand with simple path'
);
assert(
  shellCommand('grep {0} {1}', 'pattern', '/file') === "grep 'pattern' '/file'",
  'shellCommand with two args'
);

// Test shellEscapeDouble
assert(shellEscapeDouble('$HOME') === '\\$HOME', 'escape dollar sign');
assert(shellEscapeDouble('`cmd`') === '\\`cmd\\`', 'escape backticks');
assert(shellEscapeDouble('a"b') === 'a\\"b', 'escape double quote');

console.log('\nAll tests passed!');
