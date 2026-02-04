# Wiki Section Templates

Templates and guidance for creating effective wiki sections that serve both humans and AI agents.

## Architecture

**Purpose:** Help Claude understand codebase structure, key directories, and how pieces connect.

**Template:**
```markdown
## Project Structure

- `src/` - [Main source code, organized by...]
- `tests/` - [Test files, organized by...]
- `config/` - [Configuration files for...]

## Key Components

### [Component Name]
- **Location:** `path/to/component`
- **Purpose:** [What it does]
- **Dependencies:** [What it relies on]
- **Used by:** [What uses it]

### [Component Name]
...

## Data Flow

[Describe how data moves through the system: entry points -> processing -> storage -> output]

## External Integrations

- **[Service Name]:** [What it's used for, where it's configured]
```

**What to capture:**
- Directory structure with purpose of each major folder
- Key architectural patterns (MVC, microservices, monolith, etc.)
- Entry points (main files, index files, routers)
- How modules/packages are organized
- Database/storage layer structure
- External service integrations

---

## Conventions

**Purpose:** Ensure Claude follows established patterns when writing code.

**Template:**
```markdown
## Code Style

- **Formatting:** [Prettier/ESLint/Black with config at...]
- **Naming:** [camelCase/snake_case for variables, PascalCase for classes, etc.]
- **File naming:** [kebab-case.ts, PascalCase.tsx, etc.]

## Patterns

### [Pattern Name]
- **When to use:** [Scenario]
- **Example:** [File path or code snippet]

### Error Handling
[How errors are handled in this codebase]

### State Management
[How state is managed, what patterns are used]

## Import Organization

[How imports should be ordered/grouped]

## Testing Conventions

- **Test file location:** [Same directory/__tests__/separate tests folder]
- **Naming:** [*.test.ts, *.spec.ts, test_*.py]
- **Patterns:** [Describe testing patterns used]
```

**What to capture:**
- Formatting tools and their config locations
- Naming conventions for files, functions, classes, variables
- Common patterns used (factories, repositories, services, etc.)
- Error handling approach
- Import organization style
- Testing patterns

---

## Commands

**Purpose:** Know how to build, test, lint, deploy without guessing.

**Template:**
```markdown
## Development

| Command | Description |
|---------|-------------|
| `[command]` | Start development server |
| `[command]` | Run in watch mode |

## Testing

| Command | Description |
|---------|-------------|
| `[command]` | Run all tests |
| `[command]` | Run tests in watch mode |
| `[command]` | Run specific test file |
| `[command]` | Run with coverage |

## Build & Deploy

| Command | Description |
|---------|-------------|
| `[command]` | Build for production |
| `[command]` | Deploy to [environment] |

## Utilities

| Command | Description |
|---------|-------------|
| `[command]` | Lint code |
| `[command]` | Format code |
| `[command]` | Generate types/migrations/etc. |
```

**What to capture:**
- Package manager used (npm, yarn, pnpm, pip, cargo, etc.)
- Dev server commands
- Test commands (all, watch, coverage, specific)
- Build commands
- Deployment commands
- Database migration commands
- Code generation commands
- Linting and formatting commands

---

## Domain Context

**Purpose:** Help Claude understand business terms, entities, and relationships.

**Template:**
```markdown
## Business Domain

[1-2 sentences describing what this application does in business terms]

## Key Entities

### [Entity Name]
- **What it is:** [Business definition]
- **Represented as:** [How it appears in code - model/table/type name]
- **Key attributes:** [Important fields]
- **Relationships:** [How it relates to other entities]

### [Entity Name]
...

## Terminology

| Term | Meaning |
|------|---------|
| [Domain term] | [What it means in this context] |

## Business Rules

- [Important rule about how the system should behave]
- [Another rule]

## User Roles

- **[Role]:** [What they can do, their perspective]
```

**What to capture:**
- What the application does (business purpose)
- Key domain entities and their relationships
- Domain-specific terminology
- Important business rules or constraints
- User types and their permissions/perspectives

---

## Common Tasks

**Purpose:** Provide step-by-step guides for frequent operations.

**Template:**
```markdown
## Adding a New [Feature Type]

1. Create [file] at `path/`
2. Register in `path/to/registry`
3. Add tests at `path/to/tests`
4. Update [related config/documentation]

## Modifying [Component Type]

1. [Step]
2. [Step]
3. [Step]

## Debugging [Common Issue Type]

1. Check [location/log]
2. Verify [configuration]
3. Common causes: [list]

## Database Changes

1. [How to create migrations]
2. [How to run migrations]
3. [How to rollback]
```

**What to capture:**
- Adding new features/components/endpoints
- Modifying existing patterns
- Common debugging workflows
- Database/schema changes
- Environment setup steps
- Deployment procedures

---

## Gotchas

**Purpose:** Help Claude avoid known pitfalls and edge cases.

**Template:**
```markdown
## Known Issues

### [Issue Title]
- **Symptom:** [What happens]
- **Cause:** [Why it happens]
- **Solution:** [How to fix/avoid]

## Things to Watch Out For

- **[Area]:** [What to be careful about]
- **[Area]:** [What to be careful about]

## Environment-Specific

- **Local:** [Things specific to local development]
- **Staging:** [Things specific to staging]
- **Production:** [Things specific to production]

## Common Mistakes

- [Mistake]: [Correct approach]
- [Mistake]: [Correct approach]
```

**What to capture:**
- Known bugs or quirks
- Environment-specific considerations
- Common mistakes and how to avoid them
- Performance considerations
- Security considerations
- Things that look wrong but are intentional
