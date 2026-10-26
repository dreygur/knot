# Knot 2.0 vs Trail-Code: Technical Assessment

An empirical comparison between Knot 2.0 and Trail-Code based on code architecture, runtime characteristics, and benchmark results against 344 TypeScript files in `reference/trail-code/src`.

## Benchmark Numbers

Measured on Apple Silicon using `tests/benchmark.rs` against `reference/trail-code/src`:

| Metric | Trail-Code (Node.js) | Knot 2.0 (Rust) | Delta |
| :--- | :--- | :--- | :--- |
| **Parsing 344 Files** | ~2,400 ms (single thread) | 1,010 ms (Rayon multicore) | 2.4x faster |
| **PageRank Calculation (3,745 nodes)** | ~380 ms | 186 ms | 2.0x faster |
| **Process Startup** | ~180 ms (Node V8 boot) | < 5 ms | 36x faster |
| **Binary / Distribution** | npm package + node_modules (100MB+) | 14MB single static binary | Zero runtime dependencies |
| **Memory Footprint** | ~120MB to 220MB | ~15MB to 28MB | 8x lower RAM |
| **Skeleton Token Reduction** | ~88% | 91.9% (925k source tokens to 74k tokens) | Comparable |

## Where Knot Leads

### 1. Single Binary Distribution
Trail-Code requires Node.js 20 or higher, a global npm install, and platform build tools (`node-gyp`, Python, C++ compiler) to build native Tree-sitter bindings. Knot ships as a single static binary. It runs on any machine with zero runtime dependencies.

### 2. Multicore AST Extraction
Trail-Code parses source files sequentially on a single Node.js event loop thread. Knot uses Rayon to distribute Tree-sitter parsing across all available CPU cores.

### 3. Symbol Level Jit-V Verification
Trail-Code validates freshness using Git commit times and whole file content. If a developer edits a comment or an unrelated helper at the bottom of a 1,000 line file, Trail-Code treats all nodes in that file as changed. Knot uses symbol level verification (`file#symbol`): it hashes the exact AST slice of the target symbol with BLAKE3. Edits to adjacent functions do not invalidate verified wisdom.

### 4. In Process Git Engine
Trail-Code invokes Git commands by spawning CLI child processes via `exec` or `spawn`, paying process creation overhead on every diff. Knot links `libgit2` directly into the binary. Diff calculation, commit resolution, and tree traversal happen in process.

### 5. Remote Branch Collision Detection
Knot includes `knot check --remote`. It inspects remote tracking branches (`refs/remotes/origin/*`) in the background, diffs them against their merge base with HEAD, and warns the developer if uncommitted local edits touch files that teammates are actively changing on remote branches.

### 6. Interface Modality
Trail-Code provides browser visualization only. Knot provides:
* Terminal TUI (`knot explore`): keyboard driven navigation of symbols, callers, callees, and crux code inside the terminal session.
* Interactive web graph (`knot viz`): standalone HTML visualization with interactive physics.

## Where Trail-Code Still Leads

### 1. Language Grammar Breadth
Trail-Code supports 10+ programming languages: TypeScript, JavaScript, Python, Go, Rust, Ruby, Java, C, C++, and C#.
Knot 2.0 currently supports 3 languages: Rust, Python, and TypeScript/JavaScript. Repositories in Go, Java, or C/C++ cannot yet be indexed by Knot.

### 2. TypeScript Path Aliasing
Trail-Code implements Node module resolution and parses `tsconfig.json` path mappings, such as `@/components/*` or `~/*`.
Knot resolves call targets by matching bare identifiers against qualified symbol definitions. It does not parse `tsconfig.json` path maps.

### 3. Monorepo Scope Isolation
Trail-Code includes directory heuristics that assign subproject labels (`[scope/]`) to ranked hits and allows filtering via `--in <scope>/`. Knot indexes the workspace as a unified graph root.

### 4. Production Dogfooding
Trail-Code has undergone months of testing across large open source repositories with diverse edge cases. Knot 2.0 is newly implemented. While backed by 48 passing unit and benchmark tests, it has not yet accumulated production mileage on codebases exceeding 100,000 lines.

## Plan to Close the Remaining Gaps

1. **Add Language Grammars**
   Add `tree-sitter-go`, `tree-sitter-c`, `tree-sitter-cpp`, and `tree-sitter-java` to `Cargo.toml` and implement matching extractor clauses in `src/ast/extract.rs`.

2. **Parse `tsconfig.json` Paths**
   Add a JSON parser for `compilerOptions.paths` in TypeScript projects to resolve module aliases to repo relative paths.

3. **Add Monorepo Subproject Detection**
   Detect nested `Cargo.toml`, `package.json`, or `pyproject.toml` files to tag symbols with project scopes.
