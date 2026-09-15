use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;

/// The contents of `~/.yeehaw/auth.yaml`.
///
/// No `deny_unknown_fields`, and that is load-bearing rather than incidental:
/// the file used to carry a `slack:` block beside `linear:`, and every file
/// written before the Slack integration was removed still does. Serde's default
/// is to ignore a key it has no field for, so those files keep deserializing and
/// the Linear token keeps being found —
/// `tests::an_auth_file_with_a_leftover_slack_block_still_yields_the_linear_token`
/// is the measurement. The stale block does not survive the next `save_auth`,
/// which serializes this struct; that is the credentials of a removed feature
/// going away, not data anything still reads.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub linear: Option<LinearAuth>,
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

pub fn is_gh_authenticated() -> bool {
    std::process::Command::new("gh")
        .args(["auth", "status"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    /// `auth.yaml` held Slack's two tokens beside Linear's, and every file
    /// written before the Slack integration was removed still does.
    ///
    /// `load_auth` swallows a deserialize error into `AuthConfig::default()`, so
    /// if serde refused the now-unknown `slack:` key the symptom would not be an
    /// error — it would be `get_linear_token()` answering `None` and the user
    /// being asked to re-authenticate Linear for no reason they could see. The
    /// assertion is therefore on the token, which is only reachable if the parse
    /// actually succeeded.
    #[test]
    fn an_auth_file_with_a_leftover_slack_block_still_yields_the_linear_token() {
        testing::with_temp_ranch(|_| {
            let expires = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
            fs::write(
                auth_file(),
                format!(
                    "linear:\n  accessToken: lin_api_abc123\n  expiresAt: '{expires}'\n\
                     slack:\n  botToken: xoxb-dead\n  appToken: xapp-beef\n  userId: U12345\n"
                ),
            )
            .unwrap();

            assert_eq!(get_linear_token().as_deref(), Some("lin_api_abc123"));
            assert!(is_linear_authenticated());
        });
    }

    /// The write side of the same file. `save_auth` serializes `AuthConfig`,
    /// which no longer has a `slack` field, so the stale block goes away the next
    /// time a Linear token is set — and Linear's own value has to land.
    #[test]
    fn setting_a_linear_token_rewrites_the_file_without_the_slack_block() {
        testing::with_temp_ranch(|_| {
            fs::write(
                auth_file(),
                "linear:\n  accessToken: old\nslack:\n  botToken: xoxb-dead\n  appToken: xapp-beef\n",
            )
            .unwrap();

            set_linear_token("lin_api_new");

            let written = fs::read_to_string(auth_file()).unwrap();
            assert!(
                !written.contains("slack") && !written.contains("xoxb"),
                "the removed block came back:\n{written}"
            );
            assert_eq!(get_linear_token().as_deref(), Some("lin_api_new"));
        });
    }

    /// An `auth.yaml` that has *only* the removed feature's block is, to this
    /// build, an empty file. It must read as "no Linear token", not as a parse
    /// failure that some caller mistakes for something else.
    #[test]
    fn an_auth_file_holding_only_a_slack_block_reads_as_no_linear_token() {
        testing::with_temp_ranch(|_| {
            fs::write(
                auth_file(),
                "slack:\n  botToken: xoxb-dead\n  appToken: xapp-beef\n",
            )
            .unwrap();

            assert!(load_auth().linear.is_none());
            assert_eq!(get_linear_token(), None);
            assert!(!is_linear_authenticated());
        });
    }
}
