# Project: Knot
**Role:** Senior Systems Architect & Rust Engineer
**Objective:** Build a "Memory Pool" MCP server in Rust.

## 1. Core Mission & Integrity Invariants
Knot transforms volatile context into a durable, relational memory graph. The following invariants are NON-NEGOTIABLE:
1. **Success-Only Promotion:** `command_exit_code != 0` → node stays in Session scope, never promoted to Project/Global.
2. **Mandatory Jit-V:** Every recalled node must run Just-in-Time Verification. Stale nodes are flagged `[STALE:MISSING]` or `[STALE:MODIFIED]`, never silently dropped.
3. **Pre-Write Scrutiny:** Privacy scrubber MUST run before any write. No secrets (API keys/env vars) hit SQLite.
4. **Final Gate:** `commit_session` re-runs Jit-V before promotion to ensure no stale nodes escape to Project scope.

## 2. Technical Stack (Phase 1)
- **Language:** Rust (Tokio, SQLx)
- **Storage:** SQLite (Primary store for metadata, relations, and Phase 1 embeddings).
- **Search:** Cosine similarity implemented in pure SQL/Rust (Phase 1).
- **Deferred:** LanceDB (requires `protoc`). Architect `src/engine/` to be swappable for LanceDB in Phase 2.

## 3. Execution Protocol
1. Initialize the `MemoryNode` struct with `exit_code`, `source_path`, and `checksum` fields.
2. Implement the SQLite schema including a `nodes` table and a `relations` table.
3. Build the `Jit-V` engine: a function that hashes the `source_path` and compares it to the stored `checksum`.
4. Implement the MCP tools: `save_wisdom`, `recall_memory`, `verify_state`, and `commit_session`.
