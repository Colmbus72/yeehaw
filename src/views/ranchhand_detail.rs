use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::components::list::{self, ListItem, ListState, ItemStatus};
use crate::components::panel::Panel;
use crate::components::ranchhand_header;
use crate::types::*;

#[derive(Debug, Clone)]
struct DiscoveredResource {
    id: String,
    label: String,
    meta: String,
    active: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum ViewMode {
    Normal,
    Discovering,
    Error(String),
}

pub enum RanchHandAction {
    None,
    Back,
}

pub struct RanchHandDetailView {
    resources_state: ListState,
    resources: Vec<DiscoveredResource>,
    mode: ViewMode,
}

impl RanchHandDetailView {
    pub fn new() -> Self {
        Self {
            resources_state: ListState::new(),
            resources: Vec::new(),
            mode: ViewMode::Normal,
        }
    }

    pub fn enter(&mut self, ranchhand: &RanchHand) {
        self.resources.clear();
        self.resources_state = ListState::new();
        self.discover(ranchhand);
    }

    fn discover(&mut self, ranchhand: &RanchHand) {
        self.mode = ViewMode::Discovering;
        self.resources.clear();

        match ranchhand.rh_type.as_str() {
            "kubernetes" => self.discover_k8s(ranchhand),
            "terraform" => self.discover_terraform(ranchhand),
            _ => {
                self.mode = ViewMode::Error(format!("Unknown type: {}", ranchhand.rh_type));
            }
        }
    }

    fn discover_k8s(&mut self, ranchhand: &RanchHand) {
        let context = ranchhand.config.get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        let kubeconfig = ranchhand.config.get("kubeconfig")
            .and_then(|v| v.as_str());

        // Discover namespaces
        let args = vec!["get", "namespaces", "-o", "jsonpath={.items[*].metadata.name}"];
        let mut cmd = std::process::Command::new("kubectl");
        cmd.args(&args);
        cmd.arg("--context").arg(context);
        if let Some(kc) = kubeconfig {
            cmd.arg("--kubeconfig").arg(kc);
        }

        match cmd.output() {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let namespaces: Vec<&str> = stdout.split_whitespace().collect();

                for ns in &namespaces {
                    // Get pod count per namespace
                    let pod_count = get_pod_count(context, kubeconfig, ns);
                    self.resources.push(DiscoveredResource {
                        id: ns.to_string(),
                        label: ns.to_string(),
                        meta: format!("{} pods", pod_count),
                        active: true,
                    });
                }

                // Also get nodes
                let nodes = get_k8s_nodes(context, kubeconfig);
                for (name, ip) in &nodes {
                    self.resources.push(DiscoveredResource {
                        id: format!("node:{}", name),
                        label: format!("node: {}", name),
                        meta: ip.clone(),
                        active: true,
                    });
                }

                self.mode = ViewMode::Normal;
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                self.mode = ViewMode::Error(format!("kubectl failed: {}", stderr.trim()));
            }
            Err(e) => {
                self.mode = ViewMode::Error(format!("kubectl not found: {}", e));
            }
        }
    }

    fn discover_terraform(&mut self, ranchhand: &RanchHand) {
        let backend = ranchhand.config.get("backend")
            .and_then(|v| v.as_str())
            .unwrap_or("local");

        if backend == "s3" {
            let bucket = ranchhand.config.get("bucket")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = ranchhand.config.get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let region = ranchhand.config.get("region")
                .and_then(|v| v.as_str());

            // Try to read state from S3
            let mut cmd = std::process::Command::new("aws");
            cmd.args(["s3", "cp", &format!("s3://{}/{}", bucket, key), "-"]);
            if let Some(r) = region {
                cmd.args(["--region", r]);
            }

            match cmd.output() {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    self.parse_terraform_state(&stdout);
                    self.mode = ViewMode::Normal;
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    self.mode = ViewMode::Error(format!("S3 access failed: {}", stderr.trim()));
                }
                Err(e) => {
                    self.mode = ViewMode::Error(format!("aws CLI not found: {}", e));
                }
            }
        } else {
            // Local state file
            let state_path = ranchhand.config.get("local_path")
                .and_then(|v| v.as_str())
                .unwrap_or("terraform.tfstate");

            match std::fs::read_to_string(state_path) {
                Ok(content) => {
                    self.parse_terraform_state(&content);
                    self.mode = ViewMode::Normal;
                }
                Err(e) => {
                    self.mode = ViewMode::Error(format!("Cannot read state: {}", e));
                }
            }
        }
    }

    fn parse_terraform_state(&mut self, state_json: &str) {
        if let Ok(state) = serde_json::from_str::<serde_json::Value>(state_json) {
            if let Some(resources) = state.get("resources").and_then(|r| r.as_array()) {
                for resource in resources {
                    let rtype = resource["type"].as_str().unwrap_or("");
                    let name = resource["name"].as_str().unwrap_or("");
                    let mode = resource["mode"].as_str().unwrap_or("managed");

                    if mode != "managed" {
                        continue;
                    }

                    self.resources.push(DiscoveredResource {
                        id: format!("{}.{}", rtype, name),
                        label: format!("{}.{}", rtype, name),
                        meta: rtype.to_string(),
                        active: true,
                    });
                }
            }
        }
    }

    pub fn handle_input(&mut self, key: KeyCode, ranchhand: &RanchHand) -> RanchHandAction {
        match key {
            KeyCode::Esc => {
                if self.mode != ViewMode::Normal {
                    self.mode = ViewMode::Normal;
                    return RanchHandAction::None;
                }
                return RanchHandAction::Back;
            }
            KeyCode::Char('r') => {
                if self.mode != ViewMode::Discovering {
                    self.discover(ranchhand);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.resources_state.select_next(self.resources.len());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.resources_state.select_prev();
            }
            _ => {}
        }
        RanchHandAction::None
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ranchhand: &RanchHand,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(14), // ASCII header with metadata
                Constraint::Length(1),  // status line
                Constraint::Min(1),    // resources
            ])
            .split(area);

        ranchhand_header::render_ranchhand_header(frame, chunks[0], ranchhand);

        // Status line
        match &self.mode {
            ViewMode::Discovering => {
                let status = Paragraph::new("  Discovering resources...")
                    .style(Style::default().fg(Color::Yellow));
                frame.render_widget(status, chunks[1]);
            }
            ViewMode::Error(msg) => {
                let status = Paragraph::new(format!("  Error: {}", msg))
                    .style(Style::default().fg(Color::Red));
                frame.render_widget(status, chunks[1]);
            }
            ViewMode::Normal => {}
        }

        // Resources panel
        let panel_title = if ranchhand.rh_type == "kubernetes" { "Resources" } else { "Terraform Resources" };
        let panel = Panel {
            title: panel_title,
            focused: true,
            hints: Some("[r] refresh"),
        };
        let inner = panel.render(frame, chunks[2]);

        if self.resources.is_empty() {
            let msg = match &self.mode {
                ViewMode::Discovering => "Discovering...",
                _ => "No resources found. Press [r] to refresh.",
            };
            let text = Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(text, inner);
        } else {
            let items: Vec<ListItem> = self.resources.iter().map(|r| {
                ListItem {
                    id: r.id.clone(),
                    label: r.label.clone(),
                    status: Some(if r.active { ItemStatus::Active } else { ItemStatus::Inactive }),
                    meta: Some(r.meta.clone()),
                    actions: vec![],
                }
            }).collect();

            list::render_list(
                frame, inner, &items,
                &mut self.resources_state,
                true,
                Some(20),
            );
        }
    }

}

fn get_pod_count(context: &str, kubeconfig: Option<&str>, namespace: &str) -> usize {
    let mut cmd = std::process::Command::new("kubectl");
    cmd.args(["get", "pods", "-n", namespace, "-o", "jsonpath={.items[*].metadata.name}", "--context", context]);
    if let Some(kc) = kubeconfig {
        cmd.arg("--kubeconfig").arg(kc);
    }
    match cmd.output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.split_whitespace().count()
        }
        _ => 0,
    }
}

fn get_k8s_nodes(context: &str, kubeconfig: Option<&str>) -> Vec<(String, String)> {
    let mut cmd = std::process::Command::new("kubectl");
    cmd.args(["get", "nodes", "-o", "json", "--context", context]);
    if let Some(kc) = kubeconfig {
        cmd.arg("--kubeconfig").arg(kc);
    }
    match cmd.output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&stdout) {
                if let Some(items) = val["items"].as_array() {
                    return items.iter().map(|item| {
                        let name = item["metadata"]["name"].as_str().unwrap_or("unknown").to_string();
                        let ip = item["status"]["addresses"].as_array()
                            .and_then(|addrs| addrs.iter().find(|a| a["type"].as_str() == Some("InternalIP")))
                            .and_then(|a| a["address"].as_str())
                            .unwrap_or("")
                            .to_string();
                        (name, ip)
                    }).collect();
                }
            }
            vec![]
        }
        _ => vec![],
    }
}
