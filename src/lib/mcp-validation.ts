// src/lib/mcp-validation.ts

import { isString, isNumber, isBoolean } from './errors.js';

/**
 * MCP tool arguments come as Record<string, unknown> | undefined.
 * These helpers safely extract and validate values.
 */

export interface McpArgs {
  [key: string]: unknown;
}

/**
 * Get a required string argument, throwing if missing or wrong type
 */
export function requireString(args: McpArgs | undefined, key: string): string {
  const value = args?.[key];
  if (!isString(value)) {
    throw new Error(`Missing or invalid required parameter: ${key}`);
  }
  return value;
}

/**
 * Get an optional string argument
 */
export function optionalString(args: McpArgs | undefined, key: string): string | undefined {
  const value = args?.[key];
  if (value === undefined || value === null) {
    return undefined;
  }
  if (!isString(value)) {
    throw new Error(`Invalid parameter type for ${key}: expected string`);
  }
  return value;
}

/**
 * Get an optional number argument
 */
export function optionalNumber(args: McpArgs | undefined, key: string): number | undefined {
  const value = args?.[key];
  if (value === undefined || value === null) {
    return undefined;
  }
  if (!isNumber(value)) {
    throw new Error(`Invalid parameter type for ${key}: expected number`);
  }
  return value;
}

/**
 * Get an optional boolean argument with default
 */
export function optionalBoolean(args: McpArgs | undefined, key: string, defaultValue: boolean): boolean {
  const value = args?.[key];
  if (value === undefined || value === null) {
    return defaultValue;
  }
  if (!isBoolean(value)) {
    throw new Error(`Invalid parameter type for ${key}: expected boolean`);
  }
  return value;
}

/**
 * Wrapper to handle validation errors and return MCP error response
 */
export function validateArgs<T>(
  args: McpArgs | undefined,
  validator: (args: McpArgs | undefined) => T
): { data: T; error?: never } | { data?: never; error: string } {
  try {
    return { data: validator(args) };
  } catch (err) {
    return { error: err instanceof Error ? err.message : String(err) };
  }
}
