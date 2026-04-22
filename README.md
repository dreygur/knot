# Knot

Verified Engineering Memory - persistent memory pool MCP server with hierarchical knowledge, skill execution, and semantic drift detection. Eliminate Context Rot.

## Features

- **Hierarchical Knowledge**: Parent-child node linking for project-specific inheritance
- **Skills**: Reusable executable procedures with variable interpolation
- **Jit-V**: Just-in-Time Verification - validates content against file hashes
- **Semantic Drift Detection**: Confidence scoring based on cosine similarity distance
- **Multi-Agent Protection**: Origin agent tracking for namespace isolation

## Installation

### Claude Code - via `/plugin`

```
/plugin marketplace add dreygur/knot
/plugin install knot@knot
```

Knot registers itself as an MCP server automatically. The binary is downloaded from the latest GitHub release on first use.

### One-liner (any platform)

```bash
curl -fsSL https://raw.githubusercontent.com/dreygur/knot/main/install.sh | bash
```

Installs the binary to `~/.local/bin/knot` and registers with any detected MCP clients (Claude Code, OpenCode).

### Manual

```bash
# Download binary (replace with your platform)
curl -fsSL https://github.com/dreygur/knot/releases/latest/download/knot-x86_64-unknown-linux-gnu \
  -o ~/.local/bin/knot && chmod +x ~/.local/bin/knot

# Register with Claude Code
claude mcp add --name knot --command ~/.local/bin/knot \
  --scope user -e KNOT_DATA_DIR=$HOME/.knot

# Register with OpenCode
opencode mcp add --name knot --command ~/.local/bin/knot \
  -e KNOT_DATA_DIR=$HOME/.knot
```

Available binaries:

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `knot-x86_64-unknown-linux-gnu` |
| macOS x86_64 | `knot-x86_64-apple-darwin` |
| macOS Apple Silicon | `knot-aarch64-apple-darwin` |

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `KNOT_DATA_DIR` | `~/.knot` | Data persistence directory |
| `KNOT_LOG` | `knot=info` | Logging level |
| `KNOT_READ_ONLY` | - | Set to `1` to disable writes |

## Tools

### Memory

| Tool | Description |
|------|-------------|
| `save_wisdom` | Persist knowledge with Jit-V verification |
| `recall_memory` | Semantic search with ancestry chain |
| `commit_session` | Promote session nodes to project scope |
| `jit_verify` | Run verification on a specific node |
| `list_nodes` | List nodes filtered by scope/tags |
| `link_nodes` | Create typed edges between nodes |
| `forget_node` | Permanently delete a node |
| `knot_status` | Health check: nodes, skills, ghost count |

### Skills

| Tool | Description |
|------|-------------|
| `save_skill` | Save a reusable executable procedure |
| `execute_skill` | Run a skill with variable substitution |
| `recall_skills` | Search skills by name or description |

## Memory Hierarchy

| Level | Scope | Description |
|-------|-------|-------------|
| L1 | Session | Ephemeral, task-specific |
| L2 | Project | Promoted from session on clean exit |
| L3 | Global | Cross-project wisdom |

## Skill Syntax

```json
{
  "name": "add-user-crud",
  "description": "Add CRUD endpoints",
  "prerequisites": ["src/db.rs", "cmd:cargo"],
  "steps": [
    {
      "description": "Create model file",
      "command": "touch {{project}}/src/models/{{entity}}.rs"
    }
  ],
  "verification_command": "cargo test"
}
```

Execute with:
```
execute_skill(skill_name="add-user-crud", variables=[{key: "project", value: "myapp"}, {key: "entity", value: "user"}])
```

## Knot Protocol

Add to your agent rules (`~/.clauderules` or `~/AGENTS.md`):

```markdown
# Knot Protocol
- Before starting any task, call recall_memory to check for prior context.
- After completing multi-step tasks (exit 0), call save_skill to formalize the procedure.
- Use recall_skills to find reusable skills before manual execution.
- Use commit_session to promote session learnings to project scope.
```

The install script injects this automatically.

## License

MIT
