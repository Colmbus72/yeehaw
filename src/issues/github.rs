use std::process::Command;

use super::types::*;
use crate::types::Livestock;

struct GitHubRepo {
    owner: String,
    repo: String,
    livestock_name: String,
}

fn parse_github_url(url: &str) -> Option<(String, String)> {
    // HTTPS: https://github.com/owner/repo or https://github.com/owner/repo.git
    if let Some(caps) = url.find("github.com/") {
        let after = &url[caps + 11..];
        let parts: Vec<&str> = after.splitn(3, '/').collect();
        if parts.len() >= 2 {
            let owner = parts[0].to_string();
            let repo = parts[1].trim_end_matches(".git").to_string();
            if !owner.is_empty() && !repo.is_empty() {
                return Some((owner, repo));
            }
        }
    }
    // SSH: git@github.com:owner/repo.git
    if let Some(caps) = url.find("github.com:") {
        let after = &url[caps + 11..];
        let parts: Vec<&str> = after.splitn(3, '/').collect();
        if parts.len() >= 2 {
            let owner = parts[0].to_string();
            let repo = parts[1].trim_end_matches(".git").to_string();
            if !owner.is_empty() && !repo.is_empty() {
                return Some((owner, repo));
            }
        }
    }
    None
}

fn extract_repos(livestock: &[Livestock]) -> Vec<GitHubRepo> {
    let mut repos = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for l in livestock {
        if l.barn.is_some() {
            continue; // Only local livestock
        }
        if let Some(ref repo_url) = l.repo {
            if let Some((owner, repo)) = parse_github_url(repo_url) {
                let key = format!("{}/{}", owner, repo);
                if seen.insert(key) {
                    repos.push(GitHubRepo {
                        owner,
                        repo,
                        livestock_name: l.name.clone(),
                    });
                }
            }
        }
    }
    repos
}

pub fn fetch_github_issues(livestock: &[Livestock], state: IssueState) -> Result<Vec<Issue>, String> {
    let repos = extract_repos(livestock);
    if repos.is_empty() {
        return Ok(vec![]);
    }

    let mut all_issues = Vec::new();

    for repo in &repos {
        let full_repo = format!("{}/{}", repo.owner, repo.repo);
        let output = Command::new("gh")
            .args([
                "issue", "list",
                "--repo", &full_repo,
                "--state", state.as_str(),
                "--limit", "50",
                "--json", "number,title,state,author,labels,createdAt,updatedAt,body,url,comments",
            ])
            .output();

        let output = match output {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                eprintln!("[github] Failed for {}: {}", full_repo, stderr);
                continue;
            }
            Err(e) => {
                eprintln!("[github] Failed to run gh: {}", e);
                continue;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let raw: Vec<serde_json::Value> = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(_) => continue,
        };

        for issue in &raw {
            let number = issue["number"].as_u64().unwrap_or(0);
            let title = issue["title"].as_str().unwrap_or("").to_string();
            let state_str = issue["state"].as_str().unwrap_or("").to_lowercase();
            let author = issue["author"]["login"].as_str().unwrap_or("unknown").to_string();
            let body = issue["body"].as_str().unwrap_or("").to_string();
            let url = issue["url"].as_str().unwrap_or("").to_string();
            let created_at = issue["createdAt"].as_str().unwrap_or("").to_string();
            let updated_at = issue["updatedAt"].as_str().unwrap_or("").to_string();

            let labels: Vec<String> = issue["labels"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let comments: Vec<IssueComment> = issue["comments"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|c| IssueComment {
                            id: format!("{}-{}", number, c["createdAt"].as_str().unwrap_or("")),
                            author: c["author"]["login"].as_str().unwrap_or("unknown").to_string(),
                            body: c["body"].as_str().unwrap_or("").to_string(),
                            created_at: c["createdAt"].as_str().unwrap_or("").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            all_issues.push(Issue {
                id: format!("{}#{}", full_repo, number),
                identifier: format!("#{}", number),
                title,
                state: state_str.clone(),
                state_type: None,
                is_open: state_str == "open",
                author,
                body,
                labels,
                url,
                created_at,
                updated_at,
                comments,
                source: IssueSource::GitHub {
                    repo: full_repo.clone(),
                    livestock_name: repo.livestock_name.clone(),
                },
                priority: None,
                estimate: None,
                assignee: None,
                cycle: None,
            });
        }
    }

    // Sort by updated date (most recent first)
    all_issues.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(all_issues)
}
