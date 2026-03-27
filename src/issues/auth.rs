use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub linear: Option<LinearAuth>,
    pub slack: Option<SlackAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackAuth {
    #[serde(rename = "botToken")]
    pub bot_token: String,
    #[serde(rename = "appToken")]
    pub app_token: String,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearAuth {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
}

fn auth_file() -> PathBuf {
    config::yeehaw_dir().join("auth.yaml")
}

pub fn load_auth() -> AuthConfig {
    let path = auth_file();
    if !path.exists() {
        return AuthConfig::default();
    }
    match fs::read_to_string(&path) {
        Ok(content) => serde_yaml::from_str(&content).unwrap_or_default(),
        Err(_) => AuthConfig::default(),
    }
}

fn save_auth(auth: &AuthConfig) {
    let path = auth_file();
    if let Ok(content) = serde_yaml::to_string(auth) {
        let _ = fs::write(&path, content);
    }
}

pub fn get_linear_token() -> Option<String> {
    let auth = load_auth();
    let linear = auth.linear?;
    if linear.access_token.is_empty() {
        return None;
    }
    // Check expiration
    if let Some(ref expires) = linear.expires_at {
        if let Ok(exp) = chrono::DateTime::parse_from_rfc3339(expires) {
            if exp <= chrono::Utc::now() {
                return None;
            }
        }
    }
    Some(linear.access_token)
}

pub fn set_linear_token(token: &str) {
    let mut auth = load_auth();
    auth.linear = Some(LinearAuth {
        access_token: token.to_string(),
        expires_at: None,
    });
    save_auth(&auth);
}

pub fn is_linear_authenticated() -> bool {
    get_linear_token().is_some()
}

pub fn validate_linear_api_key(api_key: &str) -> bool {
    let resp = ureq::post("https://api.linear.app/graphql")
        .set("Content-Type", "application/json")
        .set("Authorization", api_key)
        .send_json(serde_json::json!({
            "query": "{ viewer { id } }"
        }));

    match resp {
        Ok(resp) => {
            if let Ok(body) = resp.into_string() {
                if body.starts_with("<!") || body.starts_with("<html") {
                    return false;
                }
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
                    return val.get("data")
                        .and_then(|d| d.get("viewer"))
                        .and_then(|v| v.get("id"))
                        .and_then(|id| id.as_str())
                        .is_some();
                }
            }
            false
        }
        Err(_) => false,
    }
}

pub fn get_slack_tokens() -> Option<SlackAuth> {
    let auth = load_auth();
    let slack = auth.slack?;
    if slack.bot_token.is_empty() || slack.app_token.is_empty() {
        return None;
    }
    Some(slack)
}

pub fn set_slack_tokens(bot_token: &str, app_token: &str, user_id: Option<&str>) {
    let mut auth = load_auth();
    auth.slack = Some(SlackAuth {
        bot_token: bot_token.to_string(),
        app_token: app_token.to_string(),
        user_id: user_id.map(String::from),
    });
    save_auth(&auth);
}

pub fn is_gh_authenticated() -> bool {
    std::process::Command::new("gh")
        .args(["auth", "status"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
