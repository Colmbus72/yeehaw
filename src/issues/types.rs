use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearAssignee {
    pub id: String,
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearCycle {
    pub id: String,
    pub name: String,
    pub number: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearTeam {
    pub id: String,
    pub name: String,
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinearStateType {
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
    Triage,
}

impl LinearStateType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "backlog" => Some(Self::Backlog),
            "unstarted" => Some(Self::Unstarted),
            "started" => Some(Self::Started),
            "completed" => Some(Self::Completed),
            "canceled" | "cancelled" => Some(Self::Canceled),
            "triage" => Some(Self::Triage),
            _ => None,
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self, Self::Backlog | Self::Unstarted | Self::Started)
    }

    pub fn status_char(&self) -> char {
        match self {
            Self::Backlog => '░',
            Self::Unstarted => '▒',
            Self::Started => '█',
            Self::Completed => '✓',
            Self::Canceled => '✗',
            Self::Triage => '?',
        }
    }
}

#[derive(Debug, Clone)]
pub enum IssueSource {
    GitHub { repo: String, livestock_name: String },
    Linear { team: String },
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub state_type: Option<LinearStateType>,
    pub is_open: bool,
    pub author: String,
    pub body: String,
    pub labels: Vec<String>,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
    pub comments: Vec<IssueComment>,
    pub source: IssueSource,
    // Linear-specific
    pub priority: Option<u8>,
    pub estimate: Option<f64>,
    pub assignee: Option<LinearAssignee>,
    pub cycle: Option<LinearCycle>,
}

impl Issue {
    pub fn priority_char(&self) -> &str {
        match self.priority {
            Some(1) => "!",
            Some(2) => ":",
            Some(3) => ".",
            Some(4) => "_",
            _ => " ",
        }
    }

    pub fn assignee_initials(&self) -> String {
        if let Some(ref a) = self.assignee {
            a.display_name
                .split_whitespace()
                .filter_map(|w| w.chars().next())
                .take(2)
                .collect::<String>()
                .to_uppercase()
        } else {
            String::new()
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinearIssueFilter {
    pub assignee_is_me: bool,
    pub assignee_id: Option<Option<String>>, // None=any, Some(None)=unassigned, Some(Some(id))=specific
    pub cycle_id: Option<String>,
    pub state_types: Option<Vec<String>>,
    pub sort_by: SortBy,
    pub sort_direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortBy {
    Priority,
    UpdatedAt,
    CreatedAt,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl Default for LinearIssueFilter {
    fn default() -> Self {
        Self {
            assignee_is_me: true,
            assignee_id: None,
            cycle_id: None, // will be set to active cycle
            state_types: Some(vec![
                "backlog".into(),
                "unstarted".into(),
                "started".into(),
            ]),
            sort_by: SortBy::Priority,
            sort_direction: SortDirection::Desc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IssueState {
    Open,
    Closed,
    All,
}

impl IssueState {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}
