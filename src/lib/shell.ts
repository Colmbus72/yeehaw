// src/lib/shell.ts

/**
 * Escape a string for safe use in a single-quoted shell argument.
 * Single quotes are the safest quoting mechanism in shell - they prevent
 * all interpretation except for single quotes themselves.
 *
 * Strategy: Replace ' with '\'' (end quote, escaped quote, start quote)
 * Then wrap the whole thing in single quotes.
 *
 * Example: "foo'bar" becomes "'foo'\''bar'"
 */
export function shellEscape(str: string): string {
  // Handle empty string
  if (str === '') {
    return "''";
  }

  // If string contains no special characters, we can use it as-is
  // But for safety, always quote
  const escaped = str.replace(/'/g, "'\\''");
  return `'${escaped}'`;
}

/**
 * Escape multiple arguments and join with spaces
 */
export function shellEscapeArgs(...args: string[]): string {
  return args.map(shellEscape).join(' ');
}

/**
 * Build a safe shell command from a template and arguments.
 * Replaces {0}, {1}, etc. with escaped versions of the arguments.
 *
 * Example:
 *   shellCommand('cat {0}', '/path/with spaces')
 *   // Returns: "cat '/path/with spaces'"
 *
 *   shellCommand('grep -i {0} {1}', 'pattern', '/path')
 *   // Returns: "grep -i 'pattern' '/path'"
 */
export function shellCommand(template: string, ...args: string[]): string {
  return template.replace(/\{(\d+)\}/g, (_, index) => {
    const i = parseInt(index, 10);
    if (i < args.length) {
      return shellEscape(args[i]);
    }
    return `{${index}}`; // Leave unreplaced if no matching arg
  });
}

/**
 * Escape a value for safe use in double quotes (for cases where we need variable expansion)
 * This is less safe than single quotes but sometimes necessary.
 *
 * Escapes: $ ` \ " ! (and newlines)
 */
export function shellEscapeDouble(str: string): string {
  return str
    .replace(/\\/g, '\\\\')
    .replace(/"/g, '\\"')
    .replace(/\$/g, '\\$')
    .replace(/`/g, '\\`')
    .replace(/!/g, '\\!');
}
