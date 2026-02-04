// src/lib/auth/storage.ts
import { readFileSync, writeFileSync, existsSync } from 'fs';
import YAML from 'js-yaml';
import { AUTH_FILE, YEEHAW_DIR } from '../paths.js';
import { mkdirSync } from 'fs';

export interface LinearAuth {
  accessToken: string;
  expiresAt?: string;  // ISO date string
}

export interface AuthConfig {
  linear?: LinearAuth;
}

export function loadAuth(): AuthConfig {
  if (!existsSync(YEEHAW_DIR)) {
    mkdirSync(YEEHAW_DIR, { recursive: true });
  }

  if (!existsSync(AUTH_FILE)) {
    return {};
  }

  try {
    const content = readFileSync(AUTH_FILE, 'utf-8');
    return (YAML.load(content) as AuthConfig) || {};
  } catch {
    return {};
  }
}

export function saveAuth(auth: AuthConfig): void {
  if (!existsSync(YEEHAW_DIR)) {
    mkdirSync(YEEHAW_DIR, { recursive: true });
  }
  writeFileSync(AUTH_FILE, YAML.dump(auth), 'utf-8');
}

export function getLinearToken(): string | null {
  const auth = loadAuth();
  if (!auth.linear?.accessToken) {
    return null;
  }

  // Check expiration if set
  if (auth.linear.expiresAt) {
    const expiresAt = new Date(auth.linear.expiresAt);
    if (expiresAt <= new Date()) {
      return null;  // Token expired
    }
  }

  return auth.linear.accessToken;
}

export function setLinearToken(accessToken: string, expiresAt?: Date): void {
  const auth = loadAuth();
  auth.linear = {
    accessToken,
    expiresAt: expiresAt?.toISOString(),
  };
  saveAuth(auth);
}

export function clearLinearToken(): void {
  const auth = loadAuth();
  delete auth.linear;
  saveAuth(auth);
}
