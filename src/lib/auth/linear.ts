// src/lib/auth/linear.ts
// Linear authentication using Personal API Keys
// Users create keys at: https://linear.app/settings/api

import { setLinearToken, getLinearToken, clearLinearToken } from './storage.js';

export const LINEAR_API_KEY_URL = 'https://linear.app/settings/api';

export { clearLinearToken };

/**
 * Check if Linear is currently authenticated.
 */
export function isLinearAuthenticated(): boolean {
  return getLinearToken() !== null;
}

/**
 * Save a Linear API key.
 */
export function saveLinearApiKey(apiKey: string): void {
  setLinearToken(apiKey);
}

/**
 * Validate a Linear API key by making a test request.
 * Returns true if valid, false otherwise.
 */
export async function validateLinearApiKey(apiKey: string): Promise<boolean> {
  try {
    const response = await fetch('https://api.linear.app/graphql', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': apiKey,
      },
      body: JSON.stringify({
        query: '{ viewer { id } }',
      }),
    });

    if (!response.ok) {
      return false;
    }

    const text = await response.text();

    // Check if we got HTML instead of JSON (indicates auth/routing issue)
    if (text.startsWith('<!') || text.startsWith('<html')) {
      return false;
    }

    const result = JSON.parse(text) as { data?: { viewer?: { id: string } }; errors?: unknown[] };
    return !!(result.data?.viewer?.id);
  } catch {
    return false;
  }
}

/**
 * Make an authenticated request to Linear's GraphQL API.
 */
export async function linearGraphQL<T>(
  query: string,
  variables?: Record<string, unknown>
): Promise<T> {
  const token = getLinearToken();
  if (!token) {
    throw new Error('Not authenticated with Linear');
  }

  const response = await fetch('https://api.linear.app/graphql', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': token,
    },
    body: JSON.stringify({ query, variables }),
  });

  if (!response.ok) {
    throw new Error(`Linear API error: ${response.statusText}`);
  }

  const text = await response.text();

  // Check if we got HTML instead of JSON
  if (text.startsWith('<!') || text.startsWith('<html')) {
    throw new Error('Linear API returned HTML instead of JSON - check your API key');
  }

  const result = JSON.parse(text) as {
    data?: T;
    errors?: Array<{ message: string }>;
  };

  if (result.errors?.length) {
    throw new Error(result.errors[0].message);
  }

  return result.data as T;
}
