# Knot — Project Plan

## Mission
Eliminate Context Rot by building a durable, relational memory graph that persists "wisdom" across sessions.
Every architectural decision made during this session is a candidate node.

---

## Architecture Decisions (Locked)

| Concern | Decision | Rationale |
|---|---|---|
| Graph model | Full hierarchical + typed lateral edges | Enables traversal and contradiction detection |
| Jit-V depth | SHA-256 content hash | Balance between correctness and performance |
| Stale handling | Surface to caller with `[STALE]` tag | Caller decides — never silent data loss |
| Success gate | `command_exit_code: Option<i32>` on `save_wisdom` | Only L0→L1 promotion; L1→L2 via utility score |
| Embedding | Simple hash-projection (128-dim) at bootstrap | Replace with candle + MiniLM in Phase 2 |
| Transport | MCP stdio | Zero network surface, local-first |

---

## Memory Hierarchy

```
L0 (Session)  ─── volatile, in-memory first, TTL = session
L1 (Project)  ─── SQLite + LanceDB, exit_code=0 gate
L2 (Global)   ─── SQLite + LanceDB, utility_score ≥ 0.8 gate
```

Promotion path: L0 → L1 on success trigger. L1 → L2 on utility threshold.

---

## Graph Schema

### Nodes (`nodes` table)
```
id              UUID PK
content         TEXT        (privacy-scrubbed)
tags            TEXT        (JSON array)
verification_path TEXT      (nullable)
content_hash    TEXT        (SHA-256 of path content at save-time)
embedding       BLOB        (128-dim f32, stored in LanceDB)
utility_score   REAL        (0.0–1.0, incremented on each recall hit)
scope_type      TEXT        ('global' | 'project' | 'session')
scope_id        TEXT        (project_id or session_id, nullable for global)
is_stale        BOOLEAN
created_at      TEXT        (ISO-8601)
updated_at      TEXT        (ISO-8601)
```

### Edges (`edges` table)
```
id              UUID PK
source_id       UUID FK → nodes.id
target_id       UUID FK → nodes.id
edge_type       TEXT  ('depends_on' | 'contradicts' | 'refines' | 'parent_scope')
created_at      TEXT
```

---

## Jit-V Protocol

```
recall_memory(query) →
  1. Semantic search → candidate node IDs
  2. For each candidate:
     a. If verification_path is None → pass (abstract knowledge)
     b. If path does not exist → mark is_stale=true, tag result [STALE:MISSING]
     c. If path exists, hash ≠ stored hash → tag result [STALE:MODIFIED]
     d. Hash matches → inject with confidence=HIGH
  3. Return results sorted by (is_stale ASC, utility_score DESC)
```

---

## MCP Tools

| Tool | Description |
|---|---|
| `save_wisdom` | Commit a knowledge node; gates on `command_exit_code` |
| `recall_memory` | Semantic search with Jit-V pass |
| `jit_verify` | Force-verify a specific node by ID |
| `commit_session` | Bulk-promote Session→Project nodes |
| `list_nodes` | List nodes filtered by scope/tags |
| `link_nodes` | Create a typed edge between two nodes |
| `forget_node` | Delete a node and its edges |

---

## File Structure

```
knot/
├── Cargo.toml
├── PROJECT_PLAN.md
├── KNOT.md
├── PRIVACY_RULES.json
└── src/
    ├── main.rs               ← MCP server entry + stdio transport
    ├── memory/
    │   ├── mod.rs
    │   ├── node.rs           ← KnowledgeNode, Edge, MemoryScope, EdgeType
    │   └── privacy.rs        ← Regex-based scrubber (PRIVACY_RULES.json)
    ├── engine/
    │   ├── mod.rs            ← StorageEngine orchestrator
    │   ├── lance.rs          ← LanceDB vector ops + embedding
    │   └── graph.rs          ← SQLite CRUD + traversal
    ├── jitv/
    │   └── mod.rs            ← Just-in-Time Verification logic
    └── tools/
        └── mod.rs            ← MCP tool definitions (save, recall, verify, etc.)
```

---

## Phases

### Phase 1 — Bootstrap (this session)
- [x] Architecture decisions locked
- [ ] Cargo.toml with all dependencies
- [ ] Data structures (`memory/node.rs`)
- [ ] Privacy scrubber (`memory/privacy.rs`)
- [ ] SQLite schema + migrations (`engine/graph.rs`)
- [ ] LanceDB integration + hash-projection embedding (`engine/lance.rs`)
- [ ] StorageEngine orchestrator (`engine/mod.rs`)
- [ ] Jit-V logic (`jitv/mod.rs`)
- [ ] MCP tools (`tools/mod.rs`)
- [ ] MCP server entry (`main.rs`)

### Phase 2 — Intelligence
- [ ] Replace hash-projection with candle + MiniLM-L6 embeddings
- [ ] Ratatui TUI for memory graph visualization
- [ ] Auto-contradiction detection on `save_wisdom`
- [ ] Utility score decay (time-weighted)

---

## Key Invariants
1. **No silent data loss** — stale nodes are flagged, not dropped
2. **Privacy-first** — all content scrubbed before persistence
3. **Exit-code gate** — L1 memory only accepts success signals
4. **Hash-verified recall** — every injected memory is verified at retrieval time
