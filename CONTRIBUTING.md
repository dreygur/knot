# Contributing to Knot

Knot is an open project built for the global developer community. Contributions are welcome.

## Adding a New Memory Tool

Tools are MCP-exposed methods on `KnotServer` in `src/tools/mod.rs`. Each tool needs:

1. **An input struct** with `#[derive(Debug, Deserialize, JsonSchema)]`
2. **A `#[tool(...)]` method** on `KnotServer`
3. **Engine logic** in `src/engine/memory_ops.rs` or `src/engine/skill_ops.rs`

### Minimal example

```rust
// 1. Input struct in src/tools/mod.rs
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MyToolInput {
    pub node_id: String,
}

// 2. Tool method on KnotServer
#[tool(description = "What this tool does.")]
async fn my_tool(
    &self,
    #[tool(aggr)] input: MyToolInput,
) -> Result<CallToolResult, McpError> {
    let engine = self.engine.lock().await;
    let result = engine.my_operation(&input.node_id).await.map_err(mcp_err)?;
    Ok(CallToolResult::success(vec![Content::text(format!("{result}"))]))
}

// 3. Engine method in src/engine/memory_ops.rs
impl StorageEngine {
    pub async fn my_operation(&self, node_id: &str) -> Result<String> {
        // interact with self.graph and self.vectors
        Ok("done".into())
    }
}
```

### Rules

- **Read tools** - no write permission check needed
- **Write tools** - check `self.read_only` and return early with `[KNOT] WARN: Vault is locked`
- **Destructive tools** - prefix the description with `[DESTRUCTIVE]`
- **Tests** - add an integration test in the `#[cfg(test)]` block at the bottom of `src/engine/mod.rs`

## Adding a Graph Query

Low-level SQL lives in `src/engine/graph.rs`. Add new queries there as `impl GraphStore` methods, then call them from `memory_ops.rs` or `skill_ops.rs`.

## Running Tests

```bash
cargo test
```

Tests use in-memory SQLite - no setup needed.

## Submitting Changes

1. Fork the repo
2. Create a branch: `git checkout -b feat/my-tool`
3. Run `cargo test` and `cargo clippy`
4. Open a pull request against `main`

## Code Style

- No comments explaining what code does - only why, when it's non-obvious
- No `unwrap()` in non-test code - use `?` or `map_err`
- Log format: `[KNOT] LEVEL  message` (see `src/logging.rs`)
- All log output goes to stderr - stdout is owned by the MCP transport
