# Knot

Verified Engineering Memory - Persistent memory pool MCP server with hierarchical knowledge, skill execution, and semantic drift detection — eliminate Context Rot.

## Features

- **Hierarchical Knowledge**: Parent-child node linking for project-specific inheritance
- **Skills**: Reusable executable procedures with variable interpolation
- **Jit-V**: Just-in-Time Verification — validates content against file hashes
- **Semantic Drift Detection**: Confidence scoring based on cosine similarity distance
- **Multi-Agent Protection**: Origin agent tracking for namespace isolation

## Quick Install

```bash
curl -sSf https://raw.githubusercontent.com/anomalyco/knot/main/install.sh | sh
```

Or manual:

```bash
# Build
cargo build --release

# Register with OpenCode
opencode mcp add --name knot --command './target/release/knot' \
  -e 'KNOT_DATA_DIR=$HOME/.knot'

# Register with Claude Code
claude mcp add -s knot -c './target/release/knot' \
  --user -e 'KNOT_DATA_DIR=$HOME/.knot'
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `KNOT_DATA_DIR` | `~/.knot` | Data persistence directory |
| `KNOT_LOG` | `knot=info` | Logging level |

## Tools

### Memory

- `save_wisdom`: Persist knowledge with Jit-V verification
- `recall_memory`: Semantic search with ancestry chain
- `commit_session`: Promote session nodes to project
- `jit_verify`: Run verification on a node
- `list_nodes`: List nodes by scope/tags
- `link_nodes`: Create typed edges
- `forget_node`: Delete a node
- `knot_status`: Health check

### Skills

- `save_skill`: Save a reusable procedure
- `execute_skill`: Run with variable substitution
- `recall_skills`: Search skills

## Memory Hierarchy

| Level | Scope | Description |
|-------|-------|-------------|
| L1 | Session | Ephemeral, task-specific |
| L2 | Project | Demoted from session on exit 0 |
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

Before starting any task, call `recall_memory` to check for prior context.

After completing multi-step tasks (exit 0), call `save_skill` to formalize the procedure.

Use `recall_skills` to find reusable skills before manual execution.

## License

MIT