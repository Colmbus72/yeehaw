use super::auth;
use super::types::*;

const TEAMS_QUERY: &str = r#"
query {
    teams {
        nodes { id name key }
    }
}
"#;

const CYCLES_QUERY: &str = r#"
query($teamId: String!) {
    team(id: $teamId) {
        cycles(first: 50) {
            nodes { id name number }
        }
        activeCycle { id name number }
    }
}
"#;

const ASSIGNEES_QUERY: &str = r#"
query($teamId: String!) {
    team(id: $teamId) {
        members(first: 100) {
            nodes { id name displayName }
        }
    }
}
"#;

const ISSUES_QUERY: &str = r#"
query($teamId: String!, $first: Int, $filter: IssueFilter) {
    team(id: $teamId) {
        issues(first: $first, filter: $filter, orderBy: updatedAt) {
            nodes {
                id identifier title description url
                createdAt updatedAt priority estimate
                state { name type }
                creator { name }
                assignee { id name displayName }
                cycle { id name number }
                labels { nodes { name } }
                comments { nodes { id body createdAt user { name } } }
            }
        }
    }
}
"#;

fn graphql_request(query: &str, variables: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let token = auth::get_linear_token().ok_or("Not authenticated with Linear")?;

    let mut body = serde_json::json!({ "query": query });
    if let Some(vars) = variables {
        body["variables"] = vars;
    }

    let resp = ureq::post("https://api.linear.app/graphql")
        .set("Content-Type", "application/json")
        .set("Authorization", &token)
        .send_json(body)
        .map_err(|e| format!("Linear API error: {}", e))?;

    let text = resp.into_string().map_err(|e| format!("Read error: {}", e))?;

    if text.starts_with("<!") || text.starts_with("<html") {
        return Err("Linear API returned HTML - check your API key".into());
    }

    let result: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {}", e))?;

    if let Some(errors) = result.get("errors").and_then(|e| e.as_array()) {
        if let Some(first) = errors.first() {
            return Err(first["message"].as_str().unwrap_or("Unknown error").to_string());
        }
    }

    result.get("data").cloned().ok_or("No data in response".into())
}

pub fn fetch_teams() -> Result<Vec<LinearTeam>, String> {
    let data = graphql_request(TEAMS_QUERY, None)?;
    let nodes = data["teams"]["nodes"]
        .as_array()
        .ok_or("Invalid teams response")?;

    Ok(nodes
        .iter()
        .map(|n| LinearTeam {
            id: n["id"].as_str().unwrap_or("").to_string(),
            name: n["name"].as_str().unwrap_or("").to_string(),
            key: n["key"].as_str().unwrap_or("").to_string(),
        })
        .collect())
}

pub fn fetch_cycles(team_id: &str) -> Result<(Vec<LinearCycle>, Option<String>), String> {
    let data = graphql_request(
        CYCLES_QUERY,
        Some(serde_json::json!({ "teamId": team_id })),
    )?;

    let active_id = data["team"]["activeCycle"]["id"]
        .as_str()
        .map(String::from);

    let nodes = data["team"]["cycles"]["nodes"]
        .as_array()
        .ok_or("Invalid cycles response")?;

    let cycles = nodes
        .iter()
        .map(|n| LinearCycle {
            id: n["id"].as_str().unwrap_or("").to_string(),
            name: {
                let name = n["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    format!("Cycle {}", n["number"].as_u64().unwrap_or(0))
                } else {
                    name.to_string()
                }
            },
            number: n["number"].as_u64().unwrap_or(0) as u32,
        })
        .collect();

    Ok((cycles, active_id))
}

pub fn fetch_assignees(team_id: &str) -> Result<Vec<LinearAssignee>, String> {
    let data = graphql_request(
        ASSIGNEES_QUERY,
        Some(serde_json::json!({ "teamId": team_id })),
    )?;

    let nodes = data["team"]["members"]["nodes"]
        .as_array()
        .ok_or("Invalid assignees response")?;

    Ok(nodes
        .iter()
        .map(|n| LinearAssignee {
            id: n["id"].as_str().unwrap_or("").to_string(),
            name: n["name"].as_str().unwrap_or("").to_string(),
            display_name: n["displayName"].as_str().unwrap_or("").to_string(),
        })
        .collect())
}

pub fn fetch_issues(
    team_id: &str,
    state: IssueState,
    filter: &LinearIssueFilter,
    limit: u32,
) -> Result<Vec<Issue>, String> {
    let mut gql_filter = serde_json::Map::new();

    // State filter
    let state_types = if let Some(ref types) = filter.state_types {
        types.clone()
    } else {
        match state {
            IssueState::Open => vec!["backlog".into(), "unstarted".into(), "started".into()],
            IssueState::Closed => vec!["completed".into(), "canceled".into()],
            IssueState::All => vec![],
        }
    };
    if !state_types.is_empty() {
        gql_filter.insert(
            "state".into(),
            serde_json::json!({ "type": { "in": state_types } }),
        );
    }

    // Assignee filter
    if filter.assignee_is_me {
        gql_filter.insert(
            "assignee".into(),
            serde_json::json!({ "isMe": { "eq": true } }),
        );
    } else if let Some(ref assignee_opt) = filter.assignee_id {
        match assignee_opt {
            None => {
                gql_filter.insert("assignee".into(), serde_json::json!({ "null": true }));
            }
            Some(id) => {
                gql_filter.insert(
                    "assignee".into(),
                    serde_json::json!({ "id": { "eq": id } }),
                );
            }
        }
    }

    // Cycle filter
    if let Some(ref cycle_id) = filter.cycle_id {
        gql_filter.insert(
            "cycle".into(),
            serde_json::json!({ "id": { "eq": cycle_id } }),
        );
    }

    let filter_val = if gql_filter.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(gql_filter)
    };

    let vars = serde_json::json!({
        "teamId": team_id,
        "first": limit,
        "filter": filter_val,
    });

    let data = graphql_request(ISSUES_QUERY, Some(vars))?;
    let nodes = data["team"]["issues"]["nodes"]
        .as_array()
        .ok_or("Invalid issues response")?;

    let mut issues: Vec<Issue> = nodes.iter().map(|n| normalize_issue(n, team_id)).collect();

    // Client-side sorting
    match filter.sort_by {
        SortBy::Priority => {
            issues.sort_by(|a, b| {
                let a_pri = match a.priority {
                    Some(0) | None => 5,
                    Some(p) => p as i32,
                };
                let b_pri = match b.priority {
                    Some(0) | None => 5,
                    Some(p) => p as i32,
                };
                if filter.sort_direction == SortDirection::Desc {
                    a_pri.cmp(&b_pri) // lower number = higher priority first
                } else {
                    b_pri.cmp(&a_pri)
                }
            });
        }
        SortBy::CreatedAt => {
            issues.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        }
        SortBy::UpdatedAt => {
            // Already sorted by API
        }
    }

    Ok(issues)
}

fn normalize_issue(node: &serde_json::Value, _team_id: &str) -> Issue {
    let state_name = node["state"]["name"].as_str().unwrap_or("").to_string();
    let state_type_str = node["state"]["type"].as_str().unwrap_or("");
    let state_type = LinearStateType::from_str(state_type_str);
    let is_open = state_type.map(|st| st.is_open()).unwrap_or(false);

    let labels: Vec<String> = node["labels"]["nodes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let comments: Vec<IssueComment> = node["comments"]["nodes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| IssueComment {
                    id: c["id"].as_str().unwrap_or("").to_string(),
                    author: c["user"]["name"].as_str().unwrap_or("Unknown").to_string(),
                    body: c["body"].as_str().unwrap_or("").to_string(),
                    created_at: c["createdAt"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let assignee = if node["assignee"].is_object() && !node["assignee"].is_null() {
        Some(LinearAssignee {
            id: node["assignee"]["id"].as_str().unwrap_or("").to_string(),
            name: node["assignee"]["name"].as_str().unwrap_or("").to_string(),
            display_name: node["assignee"]["displayName"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        })
    } else {
        None
    };

    let cycle = if node["cycle"].is_object() && !node["cycle"].is_null() {
        let number = node["cycle"]["number"].as_u64().unwrap_or(0) as u32;
        let name = node["cycle"]["name"].as_str().unwrap_or("");
        Some(LinearCycle {
            id: node["cycle"]["id"].as_str().unwrap_or("").to_string(),
            name: if name.is_empty() {
                format!("Cycle {}", number)
            } else {
                name.to_string()
            },
            number,
        })
    } else {
        None
    };

    let priority_raw = node["priority"].as_u64().map(|p| p as u8);

    Issue {
        id: node["id"].as_str().unwrap_or("").to_string(),
        identifier: node["identifier"].as_str().unwrap_or("").to_string(),
        title: node["title"].as_str().unwrap_or("").to_string(),
        state: state_name,
        state_type,
        is_open,
        author: node["creator"]["name"]
            .as_str()
            .unwrap_or("Unknown")
            .to_string(),
        body: node["description"].as_str().unwrap_or("").to_string(),
        labels,
        url: node["url"].as_str().unwrap_or("").to_string(),
        created_at: node["createdAt"].as_str().unwrap_or("").to_string(),
        updated_at: node["updatedAt"].as_str().unwrap_or("").to_string(),
        comments,
        source: IssueSource::Linear {
            team: node["team"]["name"]
                .as_str()
                .unwrap_or("Unknown")
                .to_string(),
        },
        priority: priority_raw,
        estimate: node["estimate"].as_f64(),
        assignee,
        cycle,
    }
}
