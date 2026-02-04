---
name: yeehaw-development
description: Guide development using Yeehaw's wiki as context source and living documentation. Use when working on any project tracked in Yeehaw - consult wiki before architectural decisions and contribute back when patterns evolve.
---

# Yeehaw Development

You're working on a project tracked in Yeehaw. The project wiki contains curated context to guide development - architecture patterns, conventions, gotchas, and common tasks documented by the project owner and evolved through development.

## Wiki-First Development

Before making architectural decisions or implementing significant features:

1. **Check available sections** with `mcp__yeehaw__get_wiki`
2. **Fetch relevant sections** with `mcp__yeehaw__get_wiki_section`
3. **Respect documented patterns** - they exist for a reason

The wiki is your primary source of project-specific context. It contains knowledge that isn't obvious from the code alone.

## When to Consult the Wiki

| Task | Check These Sections |
|------|---------------------|
| Adding features | Architecture, Conventions, Common Tasks |
| Debugging issues | Gotchas, Domain Context |
| Unsure about patterns | Conventions, Architecture |
| New integrations | Architecture, Gotchas |
| Understanding domain | Domain Context |

If a section exists that's relevant to your task, read it before proceeding.

## Proactive Wiki Updates

The wiki is a living document. After completing significant work, consider whether the wiki should be updated:

**When to update existing sections:**
- A documented pattern has evolved
- You discovered nuances worth capturing
- Steps in Common Tasks have changed

**When to add new sections:**
- Introduced a new architectural pattern
- Discovered gotchas during debugging
- Built a feature type that others might repeat
- Established new conventions

**How to update:**
- `mcp__yeehaw__update_wiki_section` for edits
- `mcp__yeehaw__add_wiki_section` for new content

**Guidelines:**
- Not every session needs to update the wiki
- Only update when something genuinely changes project direction or establishes new patterns
- Keep sections focused and scannable
- Be specific to THIS codebase, not generic advice

## Project Context

Use `mcp__yeehaw__get_project` to understand:
- Project structure and configured paths
- Deployment environments (livestock)
- Available wiki sections

When spawned from a specific livestock, you may be focused on deployment-specific work - check if there's relevant context in the wiki.

## Summary

1. **Before major decisions** → check the wiki
2. **After significant work** → consider updating the wiki
3. **When unsure** → the wiki likely has guidance
