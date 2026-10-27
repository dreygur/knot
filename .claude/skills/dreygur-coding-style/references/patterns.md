# Extended Patterns & Anti-Patterns

## Universal: File Ordering

Dependencies live above the function that uses them; the composite is last.

```ts
// wrong: caller first, helpers trailing
export async function syncPlaylist(id: PlaylistId) {
  const tracks = await fetchTracks(id);
  return dedupe(tracks);
}
function fetchTracks(id: PlaylistId) { ... }
function dedupe(tracks: Track[]) { ... }
```

```ts
// right: constants, then leaves, then the composite
const SYNC_TIMEOUT_MS = 30_000;

function dedupe(tracks: Track[]) { ... }
function fetchTracks(id: PlaylistId) { ... }

export async function syncPlaylist(id: PlaylistId) {
  const tracks = await fetchTracks(id);
  return dedupe(tracks);
}
```

Same in Rust and Go. Do **not** follow the local habit of parking helpers at the bottom of the module.

### Anti-Patterns to Avoid (Universal)

| Avoid | Do instead |
|---|---|
| Helper below its caller | Helper above; composite last |
| Constants declared where first used | All constants at file top |
| `// ---------- Helpers ----------` | No label; let the ordering say it |
| `// Scene:` / `// The composite` | Nothing; the shape is felt, not announced |
| One catch-all module | New file per distinct concern |
| `as any` to clear a type error | Runtime guard, or fix the declaration |
| `fake_server`, `FakeStore` | `mock_server`, `MockStore` |
| "Compiles cleanly, shipping it" | Test that executes the function and asserts behavior |


## TypeScript / JavaScript

### Mutex for Concurrent Operations

Use a `Mutex` utility to prevent race conditions in async flows (e.g., token refresh):

```ts
// utils/mutex.ts
export class Mutex {
  private queue: Array<() => void> = [];
  private locked = false;

  isLocked(): boolean { return this.locked; }

  async runExclusive<T>(fn: () => Promise<T>): Promise<T> {
    await this.acquire();
    try { return await fn(); }
    finally { this.release(); }
  }
}
```

### Retry Middleware

Wrap fetch with retry logic in a dedicated middleware file, not inline:

```ts
// middleware/retry.middleware.ts
export async function fetchWithRetry(
  url: string,
  init: RequestInit,
  options: { maxRetries: number; baseDelay: number; timeout: number },
): Promise<Response> { ... }
```

### Logger Wrapper (not raw console)

```ts
// utils/logger.ts
export const debugLog = (message: string, data?: Record<string, unknown>) =>
  client.app.log({ body: { level: "debug", message, extra: data } }).catch(() => {});
export const infoLog  = (message: string, data?: Record<string, unknown>) => ...;
export const warnLog  = (message: string, data?: Record<string, unknown>) => ...;
```

### Branded ID Types

One branded type per entity, defined in `shared/branded-types.ts`. Use `assertValidId<T>` at HTTP boundaries (req.params), `asBrand<T>` only at trusted internal casting points:

```ts
// Define
export type UserId = Brand<string, 'UserId'>;

// Validate at boundary
const userId = assertValidId<UserId>(req.params.id, 'UserId');

// Cast trusted internal value
const userId = asBrand<UserId>(dbRow.id);
```

Never use `string` for entity IDs in service/repository signatures.

### Drizzle + drizzle-zod Schema Co-location

Every table file exports: the table, `TEntity`, `TEntityInsert`, and three Zod schemas:

```ts
export const jobs = pgTable('job', { ... });
export type TJob = typeof jobs.$inferSelect;
export type TJobInsert = typeof jobs.$inferInsert;
export const ZJobCreate = createInsertSchema(jobs);
export const ZJobUpdate = createUpdateSchema(jobs);
export const ZJobSelect = createSelectSchema(jobs);
```

### Soft Delete Pattern

Never hard delete. `BaseRepository.delete()` sets `{ deleted: true }`. Add a `restore()` that sets `{ deleted: false }`. All list queries must filter `where(eq(table.deleted, false))`.

### tRPC + Express Integration

Register two route namespaces: `/v1` for REST, `/trpc` for tRPC. Dev-only routes (swagger, trpc-playground) gated on `process.env.NODE_ENV === 'development'`:

```ts
app.use('/v1', routes);
app.use('/trpc', createExpressMiddleware({ router: appRouter, createContext: createTRPCContext }));
if (process.env.NODE_ENV === 'development') {
  app.use('/api-docs', swaggerUi.serve, swaggerUi.setup(specs));
}
```

### CORS from Env

Never hardcode CORS origins. Parse from env, support comma-separated list:

```ts
const corsOrigins = (process.env.CORS_ORIGIN || 'http://localhost:3000')
  .split(',').map(o => o.trim());
app.use(cors({ origin: corsOrigins, credentials: true }));
```

### Anti-Patterns to Avoid (TS/JS)

- `catch (e) {}`: never swallow
- `any` type without a comment explaining why
- Default exports except for plugin entry points
- Mixing `require()` and `import`
- Missing semicolons
- Hardcoded URLs, tokens, or secrets
- Business logic inside `index.ts`: it's only a barrel
- Plain `string` for entity IDs: use branded types
- Hard deletes: always soft delete with `deleted` flag

---

---

## PHP / Laravel

### Field Type Auto-Detection

In `HasCrud`, guess field type from name patterns first, then fall back to DB column type. Priority order:

1. `*_id` suffix → `select` (relationship)
2. `email` / `password` / `url` → typed input
3. `is_*` / `has_*` / `can_*` → `boolean`
4. contains `description`/`bio`/`body` → `textarea`, `hide_in_index: true`
5. contains `image`/`photo`/`file` → `file`, `hide_in_index: true`
6. DB column type → `boolean`/`integer`/`decimal`/`text`/`date`/`datetime`
7. Default → `text`

### Reflection-Based Model Discovery

Scan `app/Models/` with `RecursiveIteratorIterator` to find models using a trait. Cache the result in a `static` variable:

```php
protected function getModelsWithTrait($trait): array {
    static $cachedModels = null;
    if ($cachedModels !== null) return $cachedModels;
    // scan app_path('Models') ...
    return $cachedModels = $models;
}
```

### Anti-Patterns to Avoid (PHP / Laravel)

- Business logic in `register()`: only bindings there
- Hardcoded strings where config key + default should be used
- Single monolithic `publishes()` group: split by concern
- Commands defined inline: always a separate class per command
- Auth/role checks duplicated across controllers: move to BaseController or middleware
- `echo` / `var_dump` in library code
- No PHPDoc on public methods
- Missing `runningInConsole()` guard before registering commands

### WordPress Plugin Patterns

#### Hook Registration Location

All hooks/filters in `__construct()` of the `Init` class or `init_hooks()` of the main class, never scattered across files:

```php
public function __construct() {
    add_filter('tutor_gateways_with_class', [self::class, 'add_gateways']);
    add_filter('tutor_payment_methods', [$this, 'add_method'], 100);
    add_action('init', [$this, 'process_form']);
}
```

#### Static vs Instance Methods on Hooks

- `[self::class, 'method']`: for static methods on filters that need no instance state
- `[$this, 'method']`: for instance methods that need object properties

#### Plugin Constants Pattern

```php
define('PLUGIN_VERSION', '1.0.0');
define('PLUGIN_URL', plugin_dir_url(__FILE__));
define('PLUGIN_PATH', plugin_dir_path(__FILE__));
```

Prefix all constants with plugin slug: `TSPAY_VERSION`, `TSPAY_URL`, `TSPAY_PATH`.

### WordPress + Vue 3 Script Module Pattern

WP's `wp_enqueue_script` outputs `<script src>` not `<script type="module" src>`. Use `script_loader_tag` filter to fix:

```php
add_filter('script_loader_tag', function($tag, $handle, $src) {
    if ('my-handle' !== $handle) return $tag;
    return '<script type="module" src="' . esc_url($src) . '"></script>';
}, 10, 3);
```

### Dev vs Prod Asset URL

```php
$isDev = defined('WP_DEBUG') && WP_DEBUG;
$src = $isDev
    ? '//localhost:5173/src/main.js'       // Vite HMR
    : plugin_dir_url(__FILE__) . 'dist/assets/main.js'; // Built
wp_enqueue_script('my-handle', $src, [], time(), true);
```

### Anti-Patterns to Avoid (PHP / WordPress)

- Missing `defined('ABSPATH') || exit` at top of file
- Raw `$_POST`/`$_GET`: always `sanitize_text_field(wp_unslash(...))`
- User-facing strings without `__('string', 'text-domain')`
- `curl_*` / Guzzle for HTTP: use `wp_remote_post()` / `wp_remote_get()`
- `error_log()` without `WP_DEBUG` guard
- `catch (Exception $e)`: use `catch (Throwable $e)` for PHP 7+
- Non-final main plugin class (for complex plugins)
- Multiple plugin singletons: one main class, one `get_instance()`
- Hooks registered outside `__construct()` or `init_hooks()`
- Hardcoded URLs or nonces in JS: always `wp_localize_script`
- Missing `type="module"` on Vite/ES module scripts: use the `script_loader_tag` filter
- Using `Options API` in Vue SFCs: always `<script setup>` (Composition API)

### PHP CLI (Symfony Console) Patterns

#### Multi-path Autoload Discovery

```php
foreach ([
    __DIR__ . '/../../vendor/autoload.php',
    __DIR__ . '/../../../../autoload.php',
    'vendor/autoload.php',
] as $file) {
    if (file_exists($file)) { require $file; break; }
}
```

Handles both global (`~/.composer/vendor`) and local installs.

#### Typed Question Dispatch

Never string-compare question type. Use class constants:

```php
const QUESTION_INPUT        = 1;
const QUESTION_CONFIRMATION = 2;
const QUESTION_CHOICE       = 3;
```

Private `ask()` method switches on the constant, so callers never touch Symfony Question classes directly.

### Anti-Patterns to Avoid (PHP CLI)

- `echo` in commands: use `$output->writeln()`
- Missing `set_time_limit(0)` in CLI entry
- No shebang (`#!/usr/bin/env php`) in bin file
- bin file not listed in `composer.json` `"bin"` key
- Question logic scattered inline: always a private `ask()` helper
- Not storing `$input`/`$output` as properties when helpers need them
- Not stripping unused Symfony Console options (override `getDefaultInputDefinition()`)

---

## Go

### Variadic Field Validator

```go
func RequireNonEmpty(fields ...string) bool {
    for _, f := range fields {
        if f == "" { return false }
    }
    return true
}
```

### URL Construction

Use `url.ParseRequestURI` + path append, never raw string interpolation:

```go
u, _ := url.ParseRequestURI(baseURL)
u.Path += endpoint
return u.String()
```

### Request Abstraction

Centralize all outbound HTTP in a `hooks.Request` struct to avoid scattered HTTP boilerplate:

```go
payload := &hooks.Request{
    Debug:   b.debug,
    Payload: data,
    Url:     hooks.GenerateURI(b.IsLiveStore, common.GRANT_TOKEN_URI),
}
body, err := hooks.DoRequest(payload)
```

### Panic Recovery in main

```go
defer func() {
    if r := recover(); r != nil {
        fmt.Println("Error: ", r)
    }
}()
```

### Anti-Patterns to Avoid (Go)

- `log.Fatal` outside of `main` or `cmd/`
- Business logic directly in `main.go`
- Switch statements for command dispatch: use `map[string]func`
- Prefix-based Discord commands: always slash commands
- Hard-coded credentials: use `os.Getenv` + `sample.env`

---

## Rust

### Error Enum with From Derives

```rust
#[derive(Error, Debug)]
pub enum AppError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),        // automatic From<io::Error>
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),   // automatic From<serde_json::Error>
    #[error("Custom: {0}")]
    Custom(String),                    // manual construction
}
pub type Result<T> = std::result::Result<T, AppError>;
```

### Conditional Debug Writer

Only write logs when debug mode is on:

```rust
struct ConditionalWriter { debug: bool }
impl Write for ConditionalWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.debug { io::stderr().write(buf) } else { Ok(buf.len()) }
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.debug { io::stderr().flush() } else { Ok(()) }
    }
}
```

### Workspace Crate Naming

Crates are named by function: `mcp-client`, `mcp-server`, `mcp-types`, `mcp-proxy`, `mcp-config`. The main binary crate is the project name.

### Anti-Patterns to Avoid (Rust)

- `.unwrap()` in library code: use `?` or explicit error handling
- `println!` in library crates: use `tracing`
- Putting all code in `main.rs`: split into modules
- Skipping `thiserror` and using `Box<dyn Error>` in library code

---

## Python

### 2-Space Indentation

This is intentional. All Python files use 2 spaces, not 4:

```python
class SSLCSession(SSLCommerz):
  def __init__(self, is_sandbox: bool = True) -> None:
    super().__init__(is_sandbox)

  def set_urls(self, success_url: str, fail_url: str) -> None:
    self.integration_data.update({
      'success_url': success_url,
      'fail_url': fail_url,
    })
```

### Type Hint Imports

```python
from typing import Dict, Optional, List
from decimal import Decimal
from uuid import uuid4
```

### Anti-Patterns to Avoid (Python)

- 4-space indentation
- Missing type hints on method signatures
- Missing docstrings on public methods
- `except Exception: pass`: never swallow
- Mutable default arguments (`def f(x=[])`)
- Raw `print()` in library code: use `logging`


## Prose: Rewriting Machine-Sounding Text

The rules live in SKILL.md under "Writing Like a Human". This is the drill.

### Doc Comment

```ts
// wrong
/**
 * A robust and comprehensive utility that seamlessly handles token refresh —
 * it's worth noting that this ensures concurrent callers are properly serialized.
 */

// right
/**
 * Refreshes the access token. Concurrent callers share one refresh; the rest
 * wait on the mutex and read the new token.
 */
```

What changed: no em dash, no `robust`/`comprehensive`/`seamlessly`/`ensures`, no `it's worth noting`. The second version says what the reader needs and stops.

### README Opening

````md
<!-- wrong -->
# TokenKit

In today's fast-paced development landscape, managing authentication tokens
can be a crucial challenge. TokenKit is a powerful, lightweight library that
empowers developers to seamlessly handle token lifecycles — unlocking a
frictionless auth experience.

<!-- right -->
# TokenKit

Refreshes OAuth tokens and keeps concurrent callers from racing on it.

```bash
pnpm add tokenkit
```
````

### Commit

```
# wrong
feat: Added comprehensive error handling improvements

This commit implements a robust solution that ensures errors are properly
handled throughout the codebase. Key changes:
- Added error handling
- Improved reliability

# right
fix: wrap store errors with the failing key

Callers got a bare "not found" with no way to tell which lookup failed.
```

### Chat and PR Bodies

Same rules. State the change, the reason, and anything the reviewer would otherwise have to discover. No opening restatement of the ticket, no closing summary of the diff they can read.

### Anti-Patterns to Avoid (Prose)

| Avoid | Do instead |
|---|---|
| Em dash, en dash, or hyphenated compound word | Comma, colon, semicolon, parentheses, a new sentence, or say the compound as two words / one closed word |
| `...` character, curly quotes | ASCII, or restructure |
| `not just X, but Y` | Say Y |
| `delve`, `leverage`, `robust`, `seamless`, `comprehensive` | Concrete verbs and nouns |
| `it's worth noting that P` | `P` |
| `In order to X` | `To X` |
| Comment restating the next line | Comment the why, or delete it |
| Every list exactly three items | However many there are |
| Every bullet with a **bold lead-in:** | Bold only where it earns attention |
| Closing "In summary" paragraph | End at the last real sentence |
| Emoji in code, commits, docs, CLI output | `[+]` `[-]` `[!]` `[*]` and nothing else |
| `Oops! Something went wrong!` | `invalid order id: %q` |
