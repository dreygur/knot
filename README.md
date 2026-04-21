# Knot

Persistent memory pool MCP server — eliminate Context Rot.

Knot transforms volatile session context into a durable, integrity-verified knowledge graph. Save architectural decisions, recall them semantically, and trust that stale information is surfaced rather than silently served.

## What it does

- **Save** knowledge nodes (wisdom) with optional filesystem verification paths
- **Recall** relevant nodes via semantic search, verified on retrieval
- **Promote** session learnings to project or global scope
- **Flag** stale nodes when the underlying file changes or disappears — never drop them silently
- **Link** nodes with typed, directed edges (depends_on, contradicts, refines, parent_scope)

## Memory tiers

| Tier | Scope | Promotion gate |
|------|-------|----------------|
| Session (L0) | Current invocation only | — |
| Project (L1) | Durable, project-scoped | `command_exit_code = 0` |
| Global (L2) | Universal | `utility_score ≥ 0.8` |

Only successful commands earn durable memory. Utility score increments +0.05 per recall hit; nodes that prove value over time graduate to Global.

## MCP tools

| Tool | Description |
|------|-------------|
| `save_wisdom` | Persist a knowledge node |
| `recall_memory` | Semantic search with Jit-V verification |
| `jit_verify` | Force-verify a single node |
| `commit_session` | Promote Session → Project, with Jit-V firewall |
| `list_nodes` | List nodes filtered by scope/tags |
| `link_nodes` | Create a typed edge between nodes |
| `forget_node` | Permanently delete a node and its edges |

## Just-in-Time Verification (Jit-V)

Every recalled node with a `verification_path` is verified at retrieval time using BLAKE3. If the file has changed or disappeared the node is tagged `[STALE:MODIFIED]` or `[STALE:MISSING]` and returned — never silently dropped. The same firewall runs at `commit_session` to prevent stale knowledge from escaping to durable tiers.

## Privacy

All content is scrubbed before being written to storage. API keys, bearer tokens, env var assignments, and private key headers are replaced with `[REDACTED:<rule_name>]`. Custom patterns can be defined in a `PRIVACY_RULES.json` file in the project root.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `KNOT_DATA_DIR` | `$HOME/.knot` | Directory for `knot.db` |
| `KNOT_LOG` | `knot=info` | Tracing filter (e.g. `knot=debug`) |

## Installation

**Linux / macOS**

```bash
git clone https://github.com/yourname/knot
cd knot
./install.sh
```

**Windows**

```powershell
git clone https://github.com/yourname/knot
cd knot
.\install.ps1
```

Both scripts build the release binary and register Knot with Claude Code automatically (`claude mcp add --scope user`). If the `claude` CLI is not found, they print the manual JSON snippet to add to `~/.claude/settings.json` instead.

You can override the data directory:

```bash
KNOT_DATA_DIR=/custom/path ./install.sh   # Linux/macOS
.\install.ps1 -KnotDataDir C:\custom\path  # Windows
```

After installation, restart Claude Code and confirm:

```bash
claude mcp list   # knot should appear as connected
```

The 7 tools (`save_wisdom`, `recall_memory`, etc.) will then be available to Claude in every session.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `KNOT_DATA_DIR` | `$HOME/.knot` | Directory for `knot.db` |
| `KNOT_LOG` | `knot=info` | Tracing filter (e.g. `knot=debug`) |

## Test

```bash
cargo test
```

## Architecture

```
MCP client (Claude / other)
         │ stdio JSON-RPC
    ┌────▼────┐
    │KnotServer│  (src/tools/)
    └────┬────┘
         │
    ┌────▼──────────┐
    │ StorageEngine │  (src/engine/)
    └──┬────────┬───┘
       │        │
  ┌────▼──┐ ┌───▼──────┐
  │ Graph │ │  Vector  │
  │ Store │ │  Store   │
  └───┬───┘ └────┬─────┘
      │           │
      └─────┬─────┘
         SQLite (WAL)
```

- **GraphStore** — SQLite metadata: nodes, edges, utility scores, stale flags
- **VectorStore** — Phase 1: 128-dim hash-projection embeddings stored as JSON in SQLite. Phase 2: LanceDB + MiniLM-L6-v2 (pending protoc availability)
- **Jit-V** — BLAKE3 hash comparison at retrieval and promotion time (`src/jitv/`)
- **Privacy scrubber** — regex-based redaction before every write (`src/memory/privacy.rs`)
- **Hashing** — `src/utils.rs` `calculate_hash` using BLAKE3

## License

MIT
