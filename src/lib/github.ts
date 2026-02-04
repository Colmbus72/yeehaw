import { execaSync } from 'execa';

export interface GitHubIssue {
  number: number;
  title: string;
  state: 'open' | 'closed';
  author: string;
  labels: string[];
  createdAt: string;
  updatedAt: string;
  commentsCount: number;
  body: string | null;
  url: string;
}

export interface GitHubRepo {
  owner: string;
  repo: string;
}

/**
 * Parse a GitHub URL to extract owner and repo.
 * Handles formats like:
 * - https://github.com/owner/repo
 * - https://github.com/owner/repo.git
 * - git@github.com:owner/repo.git
 */
export function parseGitHubUrl(url: string): GitHubRepo | null {
  // HTTPS format
  const httpsMatch = url.match(/github\.com\/([^/]+)\/([^/.\s]+)/);
  if (httpsMatch) {
    return { owner: httpsMatch[1], repo: httpsMatch[2].replace(/\.git$/, '') };
  }

  // SSH format
  const sshMatch = url.match(/github\.com:([^/]+)\/([^/.\s]+)/);
  if (sshMatch) {
    return { owner: sshMatch[1], repo: sshMatch[2].replace(/\.git$/, '') };
  }

  return null;
}

/**
 * Check if gh CLI is installed and authenticated.
 */
export function isGhCliAvailable(): boolean {
  try {
    execaSync('gh', ['auth', 'status']);
    return true;
  } catch {
    return false;
  }
}

/**
 * Fetch issues for a GitHub repository using gh CLI.
 */
export function fetchGitHubIssues(
  owner: string,
  repo: string,
  options: {
    state?: 'open' | 'closed' | 'all';
    limit?: number;
  } = {}
): GitHubIssue[] {
  const { state = 'open', limit = 50 } = options;

  try {
    const result = execaSync('gh', [
      'issue', 'list',
      '--repo', `${owner}/${repo}`,
      '--state', state,
      '--limit', String(limit),
      '--json', 'number,title,state,author,labels,createdAt,updatedAt,comments,body,url',
    ]);

    const issues = JSON.parse(result.stdout) as Array<{
      number: number;
      title: string;
      state: string;
      author: { login: string };
      labels: Array<{ name: string }>;
      createdAt: string;
      updatedAt: string;
      comments: Array<unknown>;
      body: string;
      url: string;
    }>;

    return issues.map((issue) => ({
      number: issue.number,
      title: issue.title,
      state: issue.state.toLowerCase() as 'open' | 'closed',
      author: issue.author.login,
      labels: issue.labels.map((l) => l.name),
      createdAt: issue.createdAt,
      updatedAt: issue.updatedAt,
      commentsCount: issue.comments.length,
      body: issue.body || null,
      url: issue.url,
    }));
  } catch (err) {
    console.error('[github] Failed to fetch issues:', err);
    return [];
  }
}

/**
 * Fetch a single issue with full details.
 */
export function fetchGitHubIssue(
  owner: string,
  repo: string,
  issueNumber: number
): GitHubIssue | null {
  try {
    const result = execaSync('gh', [
      'issue', 'view', String(issueNumber),
      '--repo', `${owner}/${repo}`,
      '--json', 'number,title,state,author,labels,createdAt,updatedAt,comments,body,url',
    ]);

    const issue = JSON.parse(result.stdout) as {
      number: number;
      title: string;
      state: string;
      author: { login: string };
      labels: Array<{ name: string }>;
      createdAt: string;
      updatedAt: string;
      comments: Array<unknown>;
      body: string;
      url: string;
    };

    return {
      number: issue.number,
      title: issue.title,
      state: issue.state.toLowerCase() as 'open' | 'closed',
      author: issue.author.login,
      labels: issue.labels.map((l) => l.name),
      createdAt: issue.createdAt,
      updatedAt: issue.updatedAt,
      commentsCount: issue.comments.length,
      body: issue.body || null,
      url: issue.url,
    };
  } catch (err) {
    console.error('[github] Failed to fetch issue:', err);
    return null;
  }
}

/**
 * Open an issue in the default browser.
 */
export function openIssueInBrowser(url: string): void {
  try {
    execaSync('open', [url]);
  } catch {
    // Fallback for Linux
    try {
      execaSync('xdg-open', [url]);
    } catch {
      // Ignore if can't open
    }
  }
}
