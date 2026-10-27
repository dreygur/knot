# Knot vs. Trail-Code: Architectural & Code-Level Comparison

This document provides an exhaustive, code-level comparison between **Knot** and **Trail-Code** (formerly *Graft*). Both systems are engineered to eliminate **"Context Rot"** and cognitive memory loss in AI coding agents (Claude Code, Cursor, OpenCode, Codex, Antigravity, etc.), but their foundational philosophies, code structures, and operational mechanisms differ substantially.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Core Missions & Problem Statements](#2-core-missions--problem-statements)
3. [Architecture & Codebase Overview](#3-architecture--codebase-overview)
   - [Knot Architecture](#knot-architecture)
   - [Trail-Code Architecture](#trail-code-architecture)
4. [Deep Code-Level Breakdown](#4-deep-code-level-breakdown)
   - [Code Comprehension & Representation (AST vs. File Hashing)](#41-code-comprehension--representation-ast-vs-file-hashing)
   - [Storage & Persistence Models (SQLite vs. Git Markdown)](#42-storage--persistence-models-sqlite-vs-git-markdown)
   - [Search, Indexing & Retrieval Engines](#43-search-indexing--retrieval-engines)
   - [Freshness, Staleness & Verification Philosophies](#44-freshness-staleness--verification-philosophies)
   - [Skills & Procedural Workflows](#45-skills--procedural-workflows)
   - [Scoping, Team Collaboration & Multi-Agent Synchronization](#46-scoping-team-collaboration--multi-agent-synchronization)
   - [Security, Privacy & Guardrails](#47-security-privacy--guardrails)
   - [Host Integrations & Ecosystem Tooling](#48-host-integrations--ecosystem-tooling)
5. [Side-by-Side Technical Comparison Matrix](#5-side-by-side-technical-comparison-matrix)
6. [Architectural Trade-Offs & Use-Case Recommendations](#6-architectural-trade-offs--use-case-recommendations)

---

## 1. Executive Summary

- **Knot** is a **runtime memory pool and executable procedure engine** written in **Rust**. It functions as an external, stateful agent vault backed by **SQLite** and a lightweight local vector store. Its signature feature is **Jit-V (Just-In-Time Verification)**, which uses cryptographic BLAKE3 file hashing to verify that memories tied to disk paths are fresh before an agent can recall them or promote them from session to project scope.
- **Trail-Code** is a **codebase knowledge graph and git-native institutional memory system** written in **TypeScript**. It directly parses the codebase's syntax using **Tree-sitter** (across 10+ languages) to build a deterministic graph of definitions, callers, callees, and blast radii. On top of this code map, it layers human and agent **learnings committed to Git**, enabling teams to share decisions, prevent agents from repeating past mistakes, and detect code overlap across unmerged Git branches.

---

## 2. Core Missions & Problem Statements

### The Problem: Context Rot & Blind Agent Exploration
When AI coding agents operate across long-running tasks or separate sessions, two primary failure modes emerge:
1. **Memory Drift & Hallucination**: The agent remembers code or conventions that existed earlier, but the underlying source files were edited, moved, or deleted. Acting on this stale context causes compilation breaks and regression errors.
2. **Context Waste & Redundant Exploration**: The agent spends significant time and token budget grepping files, reading hundreds of lines of code to understand call chains, and re-exploring bugs that human developers or past agent sessions already solved.

### How Knot Solves This
Knot treats knowledge as **verified assertions and executable runbooks**. When an agent learns something about a file, Knot binds a BLAKE3 content hash to that file path. Whenever the memory is queried, Knot verifies that the file hash on disk still matches. Stale memories are quarantined and prevented from promoting into the project baseline.

### How Trail-Code Solves This
Trail-Code provides a **topological map of the codebase combined with version-controlled institutional learnings**. Instead of making the agent grep or read entire files, Trail-Code extracts symbol definitions and call hierarchies deterministically using Tree-sitter. When an agent works through a difficult bug, it records a structured learning into `.trail/learnings/` that is committed to Git, ensuring every teammate and future agent starts from that knowledge.

---

## 3. Architecture & Codebase Overview

### Knot Architecture

```
knot/
├── Cargo.toml                  # Rust dependencies: tokio, sqlx, rmcp, blake3
├── src/
│   ├── main.rs                 # CLI entry point (recall, commit, status) & MCP stdio server
│   ├── hooks.rs                # Shell/PowerShell hooks for Claude Code
│   ├── logging.rs              # Tracing subscriber setup with rotation
│   ├── utils.rs                # BLAKE3 hash computation utilities
│   ├── engine/
│   │   ├── mod.rs              # StorageEngine coordinator
│   │   ├── graph.rs            # SQLite tables (nodes, edges, skills) & queries
│   │   ├── lance.rs            # 128-dim projection embeddings & cosine distance
│   │   ├── memory_ops.rs       # save(), recall(), commit_session()
│   │   ├── skill_ops.rs        # save_skill(), execute_skill() (subprocess runner)
│   │   └── types.rs            # Request/Response data transfer objects
│   ├── jitv/
│   │   └── mod.rs              # Just-in-Time Verification (Jit-V) logic
│   ├── memory/
│   │   ├── mod.rs              # Memory module exports
│   │   ├── node.rs             # KnowledgeNode, MemoryScope, EdgeType definitions
│   │   └── privacy.rs          # Regex pattern scrubber (PRIVACY_RULES.json)
│   ├── skills/
│   │   └── mod.rs              # SkillNode, step interpolation & validation
│   └── tools/
│       └── mod.rs              # rmcp MCP tool handlers (save_wisdom, recall_memory, etc.)
```

#### Key Architectural Traits
- **Language**: Rust (2021 Edition).
- **Runtime**: Async Tokio runtime, single self-contained native binary.
- **MCP Protocol**: Built on `rmcp` (stdio transport).
- **Database**: Embedded SQLite via `sqlx` (WAL mode enabled), default location `~/.knot/knot.db`.

---

### Trail-Code Architecture

```
Trail-Code/
├── package.json                # Dependencies: tree-sitter, @modelcontextprotocol, commander
├── tsconfig.json               # TypeScript configuration (NodeNext / ESM)
├── src/
│   ├── index.ts                # Public library exports
│   ├── cli.ts                  # CLI router (trail ask, note, check, map, callers, etc.)
│   ├── brand.ts                # Branding abstractions (trail vs. graft)
│   ├── engine.ts               # Core orchestration layer
│   ├── ask/                    # Hybrid search (BM25 + Personalized PageRank)
│   │   ├── ask.ts              # Query router & output formatter
│   │   ├── fuse.ts             # Scope ranking & result merging
│   │   ├── graphrank.ts        # Personalized PageRank over code graph
│   │   └── index-file.ts       # Lexical index generation
│   ├── blast/                  # Blast radius / transitive impact analyzer
│   ├── check/                  # Diff verifier against recorded decisions
│   │   ├── local-check.ts      # Checks git diffs against .trail/learnings/
│   │   └── types.ts            # Finding and result interfaces
│   ├── context/                # Markdown card generation (.trail/cache/)
│   ├── graph/                  # Tree-sitter AST extraction & call graph resolution
│   │   ├── build.ts            # Full repository parser
│   │   ├── extract.ts          # Language-specific grammar extractors
│   │   ├── extract-cache.ts    # Incremental file-hash cache
│   │   ├── resolve.ts          # Cross-file symbol reference & call resolver
│   │   ├── types.ts            # NodeV1, EdgeV1, GraphV1 schemas
│   │   └── write.ts            # graph.json and wiring serialization
│   ├── hosts/                  # Integrations for Claude, Cursor, Antigravity, etc.
│   │   ├── antigravity.ts      # Antigravity skill & prompt registration
│   │   ├── claude-global.ts    # Claude Code hooks & settings
│   │   ├── mcp-config.ts       # Host MCP registration
│   │   └── registry.ts         # Host capability registry
│   ├── mcp/                    # MCP server implementation
│   │   ├── server.ts           # JSON-RPC / stdio server
│   │   └── tools.ts            # graft_find_code, graft_trace_calls, graft_repo_map
│   ├── notes/                  # Institutional learnings system
│   │   ├── notes.ts            # Markdown frontmatter parsing & writing
│   │   ├── note-gate.ts        # Quality gate (rejects trivial/status notes)
│   │   ├── branches.ts         # Remote Git branch overlap detection
│   │   └── repo-trail.ts       # .trail/ directory management
│   ├── skills/                 # SKILL.md procedural workflow manager
│   │   ├── skills.ts           # Skill directory manager & takeaway store
│   │   ├── fold.ts             # Compiles takeaways into versioned SKILL.md
│   │   └── skillmd.ts          # Markdown renderer for skills
│   └── trace/                  # Agent telemetry & security
│       ├── tamper-code.ts      # Static analysis against agent tampering
│       └── shell-scan.ts       # Shell command validator
```

#### Key Architectural Traits
- **Language**: TypeScript (Strict Mode, ESM).
- **Runtime**: Node.js (>= 20).
- **AST Parsing**: Tree-sitter (native C/C++ bindings with fallback to WASM grammars).
- **Storage**: Plain-text Git-tracked Markdown files (`.trail/learnings/`) and JSON cache files (`.trail/cache/`).

---

## 4. Deep Code-Level Breakdown

### 4.1. Code Comprehension & Representation (AST vs. File Hashing)

#### Knot
Knot has **no compiler or AST awareness**. To Knot, a source file is simply an opaque path on the filesystem.
- When an agent saves a memory tied to `src/auth.rs`, Knot calls `blake3::hash(file_bytes)` and stores the resulting 32-byte hexadecimal string in the `content_hash` column.
- The edges in Knot’s graph are established between **knowledge nodes**, not between source code entities. These relationships include:
  - `DependsOn`: Knowledge node A requires knowledge node B.
  - `Contradicts`: Knowledge node A conflicts with knowledge node B.
  - `Refines`: Knowledge node A is an updated or more specific version of B.
  - `ParentScope`: Scoping hierarchy linking Session to Project to Global.

#### Trail-Code
Trail-Code builds a **syntactic and semantic graph of code definitions**:
- Uses Tree-sitter to parse source files into concrete syntax trees.
- Extracts individual declarations into `NodeV1` definitions:
  ```typescript
  export interface NodeV1 {
    id: string;               // e.g. "src/auth.ts::validateToken"
    kind: "function" | "class" | "method" | "interface" | "type" | "module";
    name: string;
    signature: string;        // Full type signature
    file: string;             // Relative file path
    span: [number, number];   // Exact start and end lines
    crux?: string;            // Key <= 8 lines capturing the crux of the definition
  }
  ```
- Resolves symbol references into directed call-graph edges:
  ```typescript
  export interface EdgeV1 {
    from: string;             // Source node ID
    to: string;               // Target node ID
    relation: "calls" | "imports" | "implements" | "extends" | "references";
  }
  ```
- Enables graph queries: `trail callers <symbol>` walks incoming edges; `trail callers <symbol> --direction out` walks outgoing edges (callees); `trail blast <symbol>` walks the transitive closure to calculate the full blast radius of a change.

---

### 4.2. Storage & Persistence Models (SQLite vs. Git Markdown)

#### Knot
Knot maintains a centralized relational database in SQLite (`~/.knot/knot.db`).
- **`nodes` Table**:
  - `id` (UUID PK)
  - `content` (Scrubbed text)
  - `tags` (JSON string array)
  - `verification_path` (Nullable path)
  - `content_hash` (Nullable BLAKE3 digest)
  - `utility_score` (Float, defaults to 0.5)
  - `scope_type` (`session` | `project` | `global`)
  - `scope_id` (Session ID or Project Name)
  - `is_stale` (Boolean integer)
  - `parent_id` (UUID self-reference)
  - `origin_agent` (Agent namespace string)
  - `created_at`, `updated_at` (ISO 8601 timestamps)
- **`edges` Table**: Foreign keys to `nodes(id)` with cascade deletion.
- **`embeddings` Table**: Key-value mapping of `node_id` to serialized 128-dimensional float arrays.
- **`skills` Table**: Schema containing procedural definitions, steps, and verification commands.

#### Trail-Code
Trail-Code stores its data in **version-controlled plain text and cached JSON**:
1. **`.trail/cache/` (or `.context/`)**:
   - `graph.json`: Serialized `NodeV1` and `EdgeV1` lists.
   - `index.json`: Lexical token index for BM25 search.
   - Markdown cards: Human-readable markdown files representing code symbols and directories with exact line links.
2. **`.trail/learnings/*.md`**:
   - Each learning is a standalone Markdown file committed to Git:
     ```yaml
     ---
     id: learn-a7b21
     title: "Use transactional batching for SQLite ingestion"
     author: "Dave"
     date: "2026-10-24"
     touches:
       - "src/engine/graph.rs"
     cost:
       minutes: 18
       tokens: 9400
     ---

     ## Decided
     Group single node inserts into transactions of 100 to avoid fsync overhead.

     ## Tried and ruled out
     Prepared statement caching without explicit transactions still hit disk bottlenecks.
     ```

---

### 4.3. Search, Indexing & Retrieval Engines

#### Knot: Deterministic Vector Projections + Cosine Similarity
1. **Projection Algorithm** (`src/engine/lance.rs`):
   - Generates a 128-dimensional L2-normalized float vector without invoking remote LLMs.
   - Tokenizes text, strips stop words, and computes `PROJECTIONS_PER_TOKEN = 3` independent FNV1a hash salts per unigram.
   - Applies position-based weighting (earlier tokens contribute more).
   - Generates adjacent bigram pairs at 0.6× weight to capture local context.
   - Normalizes the resulting vector to unit length.
2. **Search Execution**:
   - Computes query vector.
   - Performs a full scan over SQLite `embeddings`, computing cosine distance:
     $$\text{distance} = 1.0 - \frac{\vec{u} \cdot \vec{v}}{\|\vec{u}\| \|\vec{v}\|}$$
   - Filters candidate nodes through live Jit-V verification.
   - Increments the `utility_score` of verified hits.

#### Trail-Code: Hybrid BM25 + Personalized PageRank + Call Graph
1. **Structural Edge Traversals**:
   - If a query exhibits structural intent (e.g., "what calls `connect`", "callers of `verify`"), Trail-Code skips lexical ranking and directly traverses the pre-indexed graph edges.
2. **Hybrid Lexical & Topological Search** (`src/ask/ask.ts`, `src/ask/graphrank.ts`):
   - Tokenizes query and computes lexical BM25/term-overlap scores across symbol cards and markdown summaries.
   - Computes **Personalized PageRank** over the code wiring graph to identify central structural hubs and hotspots in the codebase.
   - Combines lexical relevance with PageRank centrality to surface the most critical definitions first.
3. **Crux Extraction**:
   - Returns inlined crux spans (at most 8 lines per symbol) rather than dumping full source files, minimizing token usage.

---

### 4.4. Freshness, Staleness & Verification Philosophies

#### Knot: Just-in-Time Verification (Jit-V)
- **Execution Point**: When a memory is recalled via `recall_memory` or promoted via `commit_session`.
- **Logic** (`src/jitv/mod.rs`):
  ```rust
  if !path.exists() {
      return VerificationStatus::StaleMissing;
  }
  match (&node.content_hash, hash_path(path)) {
      (Some(stored), Some(current)) if stored == &current => VerificationStatus::Verified,
      (Some(_), Some(_)) => VerificationStatus::StaleModified,
      _ => VerificationStatus::StaleMissing,
  }
  ```
- **Auto-Healing**: If a path is missing, `relink_stale_wisdom` scans the project directory for any file matching the stored BLAKE3 hash (re-establishing links when files are renamed or moved).
- **Promotion Barrier**: `commit_session` rejects stale nodes at the session boundary, preventing unverified or modified assumptions from entering project scope.

#### Trail-Code: AST-Aware Incremental Parsing & Semantic Git Diff Checks
- **Code Drift**:
  - `extract-cache.ts` stores content hashes per source file. Rebuilding the graph (`trail build`) only re-parses files that have changed on disk.
  - `trail check --graph` reports whether the compiled graph matches current source code.
- **Decision & Learning Drift** (`src/check/local-check.ts`):
  - Inspects the output of `git diff <base>...` against recorded learnings in `.trail/learnings/`.
  - Flags conflicts when:
    1. A numeric value or setting changes in a way that contradicts a recorded decision (e.g., retries changed from 3 to 5 when a learning recorded "keep retries at 3").
    2. An added code block re-introduces an implementation pattern explicitly marked under `## Tried and ruled out`.
    3. An implementation ignores an established architectural directive.

---

### 4.5. Skills & Procedural Workflows

#### Knot: Headless Executable Runbooks
In Knot, a skill is a **structured command sequence executed directly by the host machine**:
- Defined by a `SkillNode` containing:
  - `prerequisites`: Verification checks (e.g. `cmd:cargo` or file paths).
  - `steps`: Array of `{ description, command, working_dir }`.
  - `verification_command`: Post-execution validation command (e.g. `cargo test`).
- Execution (`src/engine/skill_ops.rs`):
  - Knot checks prerequisites via `which` or filesystem inspection.
  - Interpolates arguments with shell-safe escaping (`shell_quote`).
  - Spawns subprocesses using `sh -c "<command>"` and captures `stdout` / `stderr`.
  - Runs the verification command. If successful, increments `success_count` and updates `utility_score`.

#### Trail-Code: Versioned Human/Agent Markdown Skills
In Trail-Code, a skill is a **readable, versioned instructional document** (`SKILL.md`):
- Conforms to agent skill standards (stored in `.claude/skills/<name>/SKILL.md` or `~/.gemini/skills/<name>/SKILL.md`).
- Focuses on guiding the agent's reasoning and tool choices rather than spawning headless subshells.
- **Takeaways & Folding**:
  - When a user corrects an agent's workflow during a session, Trail-Code saves a small "takeaway" note.
  - Running `trail skills fold` merges pending takeaways into the main `SKILL.md` and bumps its semantic version number.
  - Skills can be local to the developer's machine (`~/.trail/`) or published to the repository (`.claude/skills/`) to be shared with teammates.

---

### 4.6. Scoping, Team Collaboration & Multi-Agent Synchronization

#### Knot: Local 3-Tier Hierarchy
- **L1 (Session Scope)**: Tied to an individual agent session ID. Demoted automatically if commands return non-zero exit codes.
- **L2 (Project Scope)**: Bound to a repository. Promoted from L1 via `commit_session` only after passing strict Jit-V verification.
- **L3 (Global Scope)**: Cross-project knowledge promoted when utility score reaches $\ge 0.8$.
- **Collaboration Model**: Storage is restricted to local SQLite (`~/.knot/`). Team sharing requires exporting databases or external sync infrastructure.

#### Trail-Code: Git-Native Distributed Collaboration
- **Committed Learnings**: Project learnings reside in `.trail/learnings/` and are committed directly to Git. Every team member and CI pipeline inherits them upon pulling.
- **Personal vs. Repo Split**:
  - Repo knowledge: Stored in `.trail/learnings/` (committed).
  - Personal preferences / machine configurations: Stored in `~/.trail/repos/<repo>/notes/` (never committed).
- **Teammate Branch Overlap Detection** (`src/notes/branches.ts`):
  - A background process inspects fetched remote refs (`origin/*`) to detect files currently modified by teammates on other branches.
  - Warns the agent when editing code that is actively being modified on another branch, preventing merge conflicts before PR submission.
- **Monorepo Federation**: Handles nested submodules and multi-project workspaces, allowing unified or scoped queries (`trail ask --in <subproject>`).

---

### 4.7. Security, Privacy & Guardrails

#### Knot: Secret Scrubbing
- Implements regex-based redaction on incoming memory strings (`src/memory/privacy.rs`).
- Loads patterns from `PRIVACY_RULES.json` with fallback defaults:
  - Generic API keys (32+ alphanumeric strings).
  - Bearer tokens (`Bearer [a-zA-Z0-9\-\._~\+\/]+=*`).
  - Environment variable definitions (`[A-Z_]{3,15}=\S+`).
- Protects against path traversal by disallowing `..` in verification paths.

#### Trail-Code: Quality Gating & Anti-Tamper Static Analysis
- **Learning Quality Gate** (`src/notes/note-gate.ts`):
  - Rejects trivial or status-only notes ("status: true today, wrong next week").
  - Requires causal explanations (must match `WHY` regex: "because...", "fails with...").
  - Enforces that notes name tracked files or symbols in the repository.
  - Flags duplicate or near-identical learnings.
- **Agent Anti-Tamper Protection** (`src/trace/tamper-code.ts`):
  - Statically analyzes commands and scripts executed by the agent (Bash, Python, Node, etc.).
  - Detects and blocks operations that attempt to overwrite `.trail/` files, session transcripts, or protected repository state.

---

### 4.8. Host Integrations & Ecosystem Tooling

#### Knot
- **MCP Server**: Implements tools: `save_wisdom`, `recall_memory`, `commit_session`, `jit_verify`, `list_nodes`, `link_nodes`, `forget_node`, `knot_status`, `prune_ghosts`, `save_skill`, `execute_skill`, `recall_skills`.
- **Shell Hooks**: Injects hooks into Claude Code (`settings.json`):
  - `pre-tool-call`: Runs `knot recall` automatically when Bash, Edit, or Write tools are called.
  - `stop`: Triggers `knot commit` at session end.
- **CLI**: Standard terminal commands (`knot recall`, `knot commit`, `knot status`).

#### Trail-Code
- **MCP Server**: Implements tools: `graft_find_code`, `graft_trace_calls`, `graft_file_api`, `graft_check_freshness`, `graft_find_all`, `graft_repo_map`.
- **Multi-Host Adapters**: Native integration support across:
  - **Claude Code**: Injects into `CLAUDE.md` and `.claude/skills/`.
  - **Cursor**: Generates `.cursor/rules/graft.mdc`.
  - **Antigravity**: Installs shared skills into `~/.gemini/skills/trail/` and instruction blocks into `GEMINI.md`.
  - **Codex / OpenCode**: Generates instruction blocks in `AGENTS.md`.
  - **Windsurf / AdaL / Copilot**: Generates host-specific configuration files.
- **Visualization**: Built-in interactive D3.js browser visualizer (`trail viz`).
- **Transcript Ingestion**: `trail init` scans past Claude Code transcripts and merged PR comments to automatically bootstrap learnings on day one.

---

## 5. Side-by-Side Technical Comparison Matrix

| Dimension | Knot | Trail-Code |
| :--- | :--- | :--- |
| **Language & Toolchain** | Rust (2021 Edition), Cargo | TypeScript (Strict ESM), Node.js $\ge 20$, esbuild |
| **Primary Code Artifact** | Standalone native binary (`knot`) | Node CLI / npm package (`@trailhq/trail-code`) |
| **AST Code Parsing** | ❌ None (treats code as raw text/bytes) | ✅ Tree-sitter (TS, Py, Go, Java, Kotlin, PHP, Swift, etc.) |
| **Call Graph & Blast Radius** | ❌ Not supported | ✅ Native (`trail callers`, `trail blast`) |
| **Storage Architecture** | SQLite DB (`~/.knot/knot.db`) | Git-committed Markdown (`.trail/learnings/`) + JSON Cache |
| **Retrieval Engine** | 128-dim Projection Vectors + Cosine Sim | Hybrid BM25 Lexical + Personalized PageRank |
| **Staleness Verification** | Jit-V: BLAKE3 cryptographic hash matching | Incremental hash cache + `git diff` rule checking |
| **Procedural Skills** | Executable shell runner (`sh -c`) | Markdown `SKILL.md` with versioned takeaway folding |
| **Team Synchronization** | ❌ Local SQLite DB (single machine) | ✅ Git-native (committed files + branch tracking) |
| **PR Conflict Detection** | ❌ None (blocks stale memory promotion) | ✅ Flags PR diffs contradicting recorded decisions |
| **Data Privacy / Redaction** | Regex scrubbing (`PRIVACY_RULES.json`) | Quality gate enforcing "why" and symbol references |
| **Tamper Protection** | ❌ None | ✅ Static code analysis (`tamper-code.ts`) |
| **Interactive Graph GUI** | ❌ Terminal output only | ✅ Browser-based D3.js visualizer (`trail viz`) |
| **Supported Agent Hosts** | Claude Code, OpenCode | Claude Code, Cursor, Antigravity, Codex, Windsurf, OpenCode |

---

## 6. Architectural Trade-Offs & Use-Case Recommendations

### Choose Knot When:
1. **You need a lightweight, self-contained native binary**: Knot compiles to a single static binary with no dependency on Node.js, npm, or system package managers.
2. **You want automated task execution with strict gates**: If an agent needs to execute multi-step terminal procedures (e.g., formatting, migrations, container restarts) and verify their success based on exit codes, Knot's `SkillNode` runner is specifically designed for this.
3. **Strict session isolation is paramount**: Knot’s L1 $\rightarrow$ L2 promotion firewall guarantees that transient, unverified, or failed trial-and-error thoughts generated in a session never enter project memory.

### Choose Trail-Code When:
1. **Codebase navigation and structural awareness matter**: Trail-Code parses actual code syntax. It instantly answers structural questions ("what calls this method?", "what are all the signatures in this file?", "what is the blast radius of changing this type?") without wasting LLM tokens.
2. **You work in a team with shared Git repositories**: Because Trail-Code stores learnings in `.trail/learnings/` as committed Markdown, every engineer and agent on the team shares institutional knowledge seamlessly through Git pull requests.
3. **You want to protect established architectural decisions**: Trail-Code's `trail check` acts as an automated guardrail, preventing agents from re-introducing approaches that the team previously tried and ruled out.
4. **Token economy is critical**: By providing concise, precomputed crux summaries and PageRank-ranked context packs, Trail-Code reduces uncached token consumption by up to 40%+.

---

### Can They Complement Each Other?
Yes. Because Knot and Trail-Code operate at different layers of the agent stack, they can be used together effectively:
- **Trail-Code** can serve as the **structural code map and institutional memory layer**, providing AST navigation, blast radius analysis, and Git-committed decision guardrails.
- **Knot** can serve as the **local runtime execution vault**, managing transient session memories, executing deterministic multi-step terminal commands, and enforcing cryptographic file freshness on disk.
