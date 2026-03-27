use crate::types::Project;

pub fn build_project_context(project: &Project) -> String {
    let mut lines = Vec::new();

    lines.push(format!("Project: {}", project.name));

    if let Some(ref summary) = project.summary {
        lines.push(format!("Summary: {}", summary));
    }

    if !project.wiki.is_empty() {
        lines.push(String::new());
        lines.push("Available wiki sections (use mcp__yeehaw__get_wiki_section to read):".to_string());
        for section in &project.wiki {
            lines.push(format!("  - {}", section.title));
        }
    }

    lines.join("\n")
}

pub fn build_livestock_context(project: &Project, livestock_name: &str) -> String {
    let mut context = build_project_context(project);

    if let Some(ls) = project.livestock.iter().find(|l| l.name == livestock_name) {
        context.push_str(&format!("\n\nLivestock: {}", ls.name));
        if let Some(ref branch) = ls.branch {
            context.push_str(&format!("\nBranch: {}", branch));
        }
        if let Some(ref repo) = ls.repo {
            context.push_str(&format!("\nRepo: {}", repo));
        }
    }

    context
}
