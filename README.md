# Knot

**Persistent memory pool MCP server** - eliminate Context Rot with verified, hierarchical knowledge.

Every memory Knot stores is **verified against the actual file on disk**. If the file changes or disappears, the memory is flagged stale before it can mislead you. This is Jit-V: Just-in-Time Verification.

> Built in Dhaka for the global developer community.
> Knot is dedicated to keeping code human-verified in an AI world.

---

## First 5 Minutes

Install, then open Claude Code and try:

```
save_wisdom(content="SQLite WAL mode prevents reader/writer blocking", tags=["sqlite", "performance"])
recall_memory(query="SQLite concurrency")
```

That's it. Knot is running. From here:
- `save_wisdom` - persist anything you learn
- `recall_memory` - retrieve it semantically later
- `commit_session` - promote today's session memories to the project permanently

---

## Installation

### Claude Code - via `/plugin`

```
/plugin marketplace add dreygur/knot
/plugin install knot@knot
```

Knot registers itself as an MCP server and wires up Claude Code hooks automatically on first start.

### One-liner

```bash
curl -fsSL https://raw.githubusercontent.com/dreygur/knot/main/install.sh | bash
```

Installs the binary, registers with Claude Code and OpenCode (whichever is detected), and injects the Knot Protocol into your agent rules.

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

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `knot-x86_64-unknown-linux-gnu` |
| macOS x86_64 | `knot-x86_64-apple-darwin` |
| macOS Apple Silicon | `knot-aarch64-apple-darwin` |
| Windows x86_64 | `knot-x86_64-pc-windows-msvc.exe` |

---

## How Jit-V Works

When you save a memory tied to a file:

```
save_wisdom(content="auth middleware validates JWT in 3 steps",
            tags=["auth"], verification_path="/src/middleware/auth.rs")
```

Knot hashes the file at save time. On every future `recall_memory`, it re-hashes the file live:

- **Match** - memory is `[VERIFIED]`, utility score increases
- **Changed** - memory is `[STALE:MODIFIED]`, flagged but still returned
- **Missing** - memory is `[STALE:MISSING]`, flagged for review

Stale memories are never silently promoted to project scope. `commit_session` rejects them at the boundary.

---

## Memory Hierarchy

| Level | Scope | Lifetime |
|-------|-------|----------|
| L1 | Session | Current conversation only |
| L2 | Project | Promoted from session via `commit_session` |
| L3 | Global | Cross-project, long-term knowledge |

---

## Tools

### Memory

| Tool | Description |
|------|-------------|
| `save_wisdom` | Persist knowledge with optional file verification |
| `recall_memory` | Semantic search - returns verified results with confidence score |
| `commit_session` | Promote L1 session nodes to L2 project scope (stale nodes rejected) |
| `jit_verify` | Re-run verification on a specific node |
| `list_nodes` | List nodes filtered by scope and tags |
| `link_nodes` | Create typed edges: `depends_on`, `contradicts`, `refines` |
| `forget_node` | Permanently delete a node |
| `knot_status` | Health check: node counts, ghost count, DB status |
| `prune_ghosts` | Remove nodes whose source files no longer exist |

### Skills

| Tool | Description |
|------|-------------|
| `save_skill` | Save a reusable executable procedure with steps and verification |
| `execute_skill` | Run a saved skill with variable substitution |
| `recall_skills` | Search skills by name or description |

---

## Skills

Skills are reusable procedures with variable placeholders. Save once, run anywhere.

### Community Standard Style (template)

Copy this as a starting point for your own skills:

```json
{
  "name": "community-standard-style",
  "description": "Apply consistent code style to a module",
  "prerequisites": ["cmd:cargo"],
  "steps": [
    {
      "description": "Format the module",
      "command": "cargo fmt -- {{file_path}}"
    },
    {
      "description": "Run linter",
      "command": "cargo clippy -- -D warnings"
    }
  ],
  "verification_command": "cargo test"
}
```

Save it:
```
save_skill(name="community-standard-style", description="Apply consistent code style",
           prerequisites=["cmd:cargo"],
           steps=[{description:"Format", command:"cargo fmt -- {{file_path}}"},
                  {description:"Lint",   command:"cargo clippy -- -D warnings"}],
           verification_command="cargo test")
```

Run it:
```
execute_skill(skill_name="community-standard-style",
              variables=[{key:"file_path", value:"src/main.rs"}])
```

---

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `KNOT_DATA_DIR` | `~/.knot` | Data persistence directory |
| `KNOT_LOG` | `knot=info` | Logging level (`knot=debug` for verbose) |
| `KNOT_READ_ONLY` | - | Set to `1` to disable all writes |

---

## Knot Protocol

The install script injects this into `~/.clauderules` / `~/AGENTS.md` automatically:

```markdown
# Knot Protocol
- Before starting any task, call recall_memory to check for prior context.
- After completing multi-step tasks (exit 0), call save_skill to formalize the procedure.
- Use recall_skills to find reusable skills before manual execution.
- Use commit_session to promote session learnings to project scope.
```

---

## CLI

The binary also works as a CLI for scripting and hooks:

```bash
knot recall "SQLite WAL mode"          # semantic search
knot commit <session_id> <project>     # commit session to project
knot status                            # vault health
knot --help
```

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

---

## License

MIT
