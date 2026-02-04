---
name: yeehaw-project-setup
description: Configure a Yeehaw project with an auto-generated summary, brand color, and wiki sections. Use when a user has created a Yeehaw project with livestock configured and wants to populate the project metadata and wiki. Triggers on requests like "set up this project in yeehaw", "configure yeehaw for this project", "populate my yeehaw wiki", or "initialize yeehaw project settings".
---

# Yeehaw Project Setup

Autonomously configure a Yeehaw project by exploring the codebase to generate a summary, discover a brand color, and create structured wiki sections that guide both humans and AI agents.

## Prerequisites

Before running this skill:
- Project must exist in Yeehaw (verify with `mcp__yeehaw__get_project`)
- Local livestock should be configured with the project path
- Be in the project's working directory or know its path

## Workflow

Execute these steps in order without user confirmation between steps.

### 1. Verify Project State

```
mcp__yeehaw__get_project(name: "<project-name>")
```

Confirm the project exists and note:
- Current summary (if any)
- Current color (if any)
- Configured livestock and their paths

If project doesn't exist, stop and inform the user to create it first via the Yeehaw app.

### 2. Deep Codebase Exploration

Use the Task tool with `subagent_type: "Explore"` for thorough analysis. Gather:

**Structure & Architecture:**
- Directory structure and organization
- Key entry points and main files
- Architectural patterns (MVC, microservices, etc.)
- External integrations and dependencies

**Conventions & Patterns:**
- Code style (check for prettier, eslint, editorconfig)
- Naming conventions
- Testing patterns and locations
- Import organization

**Commands:**
- Package manager (package.json, requirements.txt, Cargo.toml, etc.)
- Available scripts/commands
- Build and test commands

**Domain:**
- README and documentation
- Key models/entities
- Business logic patterns

**Take detailed notes** - this information populates the wiki sections.

### 3. Color Discovery

Find a brand color following priority order (see `references/color-discovery.md` for details):

1. **Explicit brand colors** - Check tailwind.config, CSS variables, theme files
2. **UI framework colors** - MUI theme, Chakra theme, etc.
3. **Logo/asset colors** - Extract from SVG logos if present
4. **Technology association** - Use framework's brand color (React blue, Vue green, etc.)
5. **Domain association** - Finance->navy, healthcare->teal, etc.

Return a 6-character hex code with `#` prefix (e.g., `#FF6B6B`).

### 4. Generate Summary

Write a 1-2 sentence summary that captures:
- What the project does (purpose)
- Key technologies used
- Primary domain/audience

Keep it concise - this appears in project listings.

### 5. Update Project

```
mcp__yeehaw__update_project(
  name: "<project-name>",
  summary: "<generated-summary>",
  color: "<discovered-color>"
)
```

### 6. Create Wiki Sections

Use `references/wiki-templates.md` as a guide for structure. Create each section with `mcp__yeehaw__add_wiki_section`.

**Required sections:**

| Section | Content Focus |
|---------|---------------|
| Architecture | Directory structure, key components, data flow, integrations |
| Conventions | Code style, naming, patterns, testing approach |
| Commands | Dev, test, build, deploy commands in table format |
| Domain Context | Business purpose, key entities, terminology, rules |
| Common Tasks | Step-by-step guides for adding features, debugging |
| Gotchas | Known issues, environment quirks, common mistakes |

For each section:
```
mcp__yeehaw__add_wiki_section(
  project: "<project-name>",
  title: "<section-title>",
  content: "<markdown-content>"
)
```

**Content quality guidelines:**
- Be specific to THIS codebase, not generic advice
- Include actual file paths and command examples
- Reference real patterns found during exploration
- Keep each section focused and scannable

## Completion

After all sections are created, summarize what was configured:
- Project summary
- Selected color (and why)
- Wiki sections created
- Any areas that need user refinement (e.g., business rules only the user knows)
