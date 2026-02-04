// src/lib/issues/github.ts
import { execaSync } from 'execa';
import type { Issue, IssueComment, IssueProvider, FetchIssuesOptions } from './types.js';
import type { Livestock } from '../../types.js';

interface GitHubRepo {
  owner: string;
  repo: string;
  livestockName: string;
  livestockPath: string;
}

/**
 * Parse a GitHub URL to extract owner and repo.
 */
function parseGitHubUrl(url: string): { owner: string; repo: string } | null {
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

export class GitHubProvider implements IssueProvider {
  readonly type = 'github' as const;
  private repos: GitHubRepo[] = [];

  constructor(livestock: Livestock[]) {
    // Extract GitHub repos from local livestock only
    const localLivestock = livestock.filter((l) => !l.barn && l.repo);
    const seen = new Set<string>();

    for (const l of localLivestock) {
      if (!l.repo) continue;
      const parsed = parseGitHubUrl(l.repo);
      if (parsed) {
        const key = `${parsed.owner}/${parsed.repo}`;
        if (!seen.has(key)) {
          seen.add(key);
          this.repos.push({
            ...parsed,
            livestockName: l.name,
            livestockPath: l.path,
          });
        }
      }
    }
  }

  async isAuthenticated(): Promise<boolean> {
    try {
      execaSync('gh', ['auth', 'status']);
      return true;
    } catch {
      return false;
    }
  }

  async authenticate(): Promise<void> {
    // GitHub auth is handled externally via `gh auth login`
    // This is a no-op - the view should display instructions
    throw new Error('Run `gh auth login` in your terminal to authenticate with GitHub');
  }

  async fetchIssues(options: FetchIssuesOptions = {}): Promise<Issue[]> {
    const { state = 'open', limit = 50 } = options;

    if (this.repos.length === 0) {
      return [];
    }

    const allIssues: Issue[] = [];

    for (const repo of this.repos) {
      try {
        const result = execaSync('gh', [
          'issue', 'list',
          '--repo', `${repo.owner}/${repo.repo}`,
          '--state', state,
          '--limit', String(limit),
          '--json', 'number,title,state,author,labels,createdAt,updatedAt,body,url,comments',
        ]);

        const issues = JSON.parse(result.stdout) as Array<{
          number: number;
          title: string;
          state: string;
          author: { login: string };
          labels: Array<{ name: string }>;
          createdAt: string;
          updatedAt: string;
          body: string;
          url: string;
          comments: Array<{ author: { login: string }; body: string; createdAt: string }>;
        }>;

        for (const issue of issues) {
          allIssues.push({
            id: `${repo.owner}/${repo.repo}#${issue.number}`,
            identifier: `#${issue.number}`,
            title: issue.title,
            state: issue.state.toLowerCase(),
            isOpen: issue.state.toLowerCase() === 'open',
            author: issue.author.login,
            body: issue.body || '',
            labels: issue.labels.map((l) => l.name),
            url: issue.url,
            createdAt: issue.createdAt,
            updatedAt: issue.updatedAt,
            comments: issue.comments.map((c) => ({
              id: `${issue.number}-${c.createdAt}`,
              author: c.author.login,
              body: c.body,
              createdAt: c.createdAt,
            })),
            source: {
              type: 'github',
              repo: `${repo.owner}/${repo.repo}`,
              livestockName: repo.livestockName,
              livestockPath: repo.livestockPath,
            },
          });
        }
      } catch (err) {
        console.error(`[github] Failed to fetch issues for ${repo.owner}/${repo.repo}:`, err);
      }
    }

    // Sort by updated date (most recent first)
    allIssues.sort((a, b) =>
      new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime()
    );

    return allIssues;
  }

  async getIssue(id: string): Promise<Issue> {
    // Parse id format: "owner/repo#number"
    const match = id.match(/^([^/]+)\/([^#]+)#(\d+)$/);
    if (!match) {
      throw new Error(`Invalid issue ID format: ${id}`);
    }

    const [, owner, repo, numberStr] = match;
    const issueNumber = parseInt(numberStr, 10);

    // Find the livestock for this repo
    const repoInfo = this.repos.find((r) => r.owner === owner && r.repo === repo);
    if (!repoInfo) {
      throw new Error(`Repo not found in livestock: ${owner}/${repo}`);
    }

    const result = execaSync('gh', [
      'issue', 'view', String(issueNumber),
      '--repo', `${owner}/${repo}`,
      '--json', 'number,title,state,author,labels,createdAt,updatedAt,body,url,comments',
    ]);

    const issue = JSON.parse(result.stdout) as {
      number: number;
      title: string;
      state: string;
      author: { login: string };
      labels: Array<{ name: string }>;
      createdAt: string;
      updatedAt: string;
      body: string;
      url: string;
      comments: Array<{ author: { login: string }; body: string; createdAt: string }>;
    };

    return {
      id,
      identifier: `#${issue.number}`,
      title: issue.title,
      state: issue.state.toLowerCase(),
      isOpen: issue.state.toLowerCase() === 'open',
      author: issue.author.login,
      body: issue.body || '',
      labels: issue.labels.map((l) => l.name),
      url: issue.url,
      createdAt: issue.createdAt,
      updatedAt: issue.updatedAt,
      comments: issue.comments.map((c) => ({
        id: `${issue.number}-${c.createdAt}`,
        author: c.author.login,
        body: c.body,
        createdAt: c.createdAt,
      })),
      source: {
        type: 'github',
        repo: `${owner}/${repo}`,
        livestockName: repoInfo.livestockName,
        livestockPath: repoInfo.livestockPath,
      },
    };
  }
}
