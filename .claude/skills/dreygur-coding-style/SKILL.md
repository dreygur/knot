---
name: dreygur-coding-style
description: Use when writing any code for dreygur in Go, TypeScript, JavaScript, Rust, Python, or PHP/Laravel/WordPress. Trigger on "write code in my style", "follow my patterns", "use my architecture", "create a new project", "add a service/repository/handler", when ordering functions within a file, when writing tests or a commit message, when writing a README, doc, comment, or PR body that should not read as machine-written, or when reviewing code for style violations. Also applies the Karpathy behavior guidelines: think before coding, simplicity first, surgical changes, goal driven execution.
version: 2.10.0
---

# dreygur Coding Style

## The Shape of a File

Poetry and a story at the same time. Poetry: nothing wasted, the shape on the page carries meaning. Story: it reads in order, each part following from the last. Ordering is the argument, not an accident.

### 1. Dependencies Above, Composite Last
A function's dependencies are defined **above** it, never below. Write the composite, then place everything it calls above it. Reading top to bottom, you never meet a name before its parts.

```
constants -> leaf helpers -> mid-level helpers -> public entry point
```

No forward references. New helper + new caller means the helper goes first. This is the **opposite** of the common Rust/Go habit of parking helpers at the bottom. Do not follow that habit here.

Exception: when inserting into an existing section that already reads caller-first, keep the local convention rather than reordering existing code.

### 2. Constants Are the Rules of the Play
Timeouts, ports, limits, fixed strings sit at the very top of the file, before any behavior.

### 3. One File, One Purpose
Prefer a new file over growing an existing one when the concern is distinct.

### 4. Small Parts, Small Cast
Each part sets up only what it needs, with a cast small enough to hold in mind at once.

### 5. Never Label the Code
No section-divider comments, no banner comments with dashes, no block labels, not even plain descriptive ones. The shape is meant to be **felt in how the code reads, never announced in the text**.

```ts
// ---------- Helpers ----------        // never
// Scene: token refresh                 // never
// The composite                        // never
```

A comment earns its place only by explaining **why** something is the way it is, or a non-obvious constraint. Doc comments on exported items still apply (Universal Rule 8); those describe contract and intent, not layout.

---

## Universal Rules

### 1. Types First
Contracts before implementations.
- TS/JS: `types.ts` | Go: `models/` | Rust: `types.rs` | Python: base class | PHP: `src/Contracts/`

### 2. Layered Architecture

| Layer | TS/JS | Go | Rust | Python | PHP |
|---|---|---|---|---|---|
| Types | `types.ts` | `models/` | `types.rs` | base class | `src/Support/` |
| Services | `*.service.ts` | `methods/` | `*/manager.rs` | class methods | `src/Services/` |
| Repositories | `*.repository.ts` | `db/`, `repo/` | `*/store.rs` | class+DB | Eloquent+Concerns |
| Controllers | `*.handler.ts` | `handlers/` | `src/cli/` | route fns | `src/Http/Controllers/` |
| Middleware | `*.middleware.ts` | `hooks/` | `*/middleware.rs` | decorators | `src/Http/Middleware/` |
| Utils | `utils/` | `utils/`, `lib/` | `src/util/` | helpers | `src/Support/` |
| Config | `constants.ts` | `common/vars.go` | `src/config/` | `constants.py` | `config/*.php` |

### 3. Typed Errors, Never Raw Strings
- TS: class hierarchy extending `Error`
- Go: sentinel vars `var ErrX = errors.New(...)`
- Rust: `thiserror` enum
- Python: custom `Exception` subclasses
- PHP: `\InvalidArgumentException` or project-prefixed class

### 4. Singleton Exports
```ts
export const tokenService = new TokenService();
```
```go
var Collection = &db.MongoDB{Address: os.Getenv("DB_URI")}
```

### 5. Leveled Logging, Never Raw print/console.log in Library Code
| Language | Tool |
|---|---|
| Go | custom `PrintLog` + `fatih/color` (`[+]`/`[-]`/`[!]`/`[*]`) |
| TypeScript | `debugLog`/`infoLog`/`warnLog` wrappers or Winston |
| Rust | `tracing` (`info!`/`warn!`/`error!`/`debug!`) |
| Python | stdlib `logging` |

### 6. No Magic Values: constants/config files only
### 7. Never Swallow Errors: propagate with context
### 8. Doc Comments on every exported item: comment the *why*, never the layout
### 9. Filenames
- TS/JS: `kebab-case` + layer suffix, as in `token.service.ts`, `auth.middleware.ts`
- Go: `snake_case.go`, as in `token.go`, `cmd_handler.go`
- Rust: `snake_case.rs`, as in `error.rs`, `config_watcher.rs`
- Python: `snake_case.py`
- PHP: `PascalCase.php` matching class name

### 10. No Casts to Silence the Type Checker
Never wrap an expression to make an error go away: `/** @type {X} */ (expr)` in JS, `as any`/`as unknown as X` in TS. Every such cast is masking a real defect: a missing runtime guard, dead config, or a dynamic dispatch that should be explicit branches.

Fix it by narrowing at runtime or fixing the declaration. Annotating a `const` on its own line is fine. Types belong in `types.ts` / `types/*.d.ts`, not inline at the call site.

Only tolerated at a generic base-class boundary the type system genuinely cannot express (see `BaseRepository` below).

### 11. Tests Prove Behavior, Not Signatures
A clean `cargo build`, `tsc --noEmit`, or `go vet` is **not** evidence of correctness. Neither is a `no_run` doctest; it type-checks a signature and proves nothing.

- Every new function gets a test that **executes** it and asserts on real behavior, including error paths.
- Prefer hermetic tests that run in the default runner (`cargo test`, `pnpm test`, `go test ./...`) over ones needing live services; add a gated/`#[ignore]` integration test when the real contract matters.
- Never report work as done on the strength of a successful compile.

### 12. Test Doubles Are `mock`, Never `fake`
`mock_cdp`, `MockTokenStore`, `mock_server.go`. Not `fake_*`.

### 13. Bug Fixes Start With a Failing Test
When the task is fixing a bug: write the test that reproduces it first, run it, watch it fail, then write the fix, then watch it pass. Never write the fix before the test exists. This is the one case where test-first is mandatory rather than a preference.

---

## AGENTS.md Rules

General conventions dreygur runs across every language. These sit alongside Universal Rules above; where the two overlap, this section is the more specific instruction.

### Say Less
For anything a human reads (comment, commit message, reply to a prompt), use as few words as possible. Pick every word on purpose. Down to the point. Less is more.

### No Superlatives, No Praise
Never tell dreygur he's right, absolutely right, or anything else that sounds like flattery. State the cold hard truth: what's wrong, what's missing, what breaks.

### Constants Over Magic Values
Extract a recurring or meaningful literal into a named `const` or enum. Leave a self-explanatory one-off value inline, that's not clutter, it's a value used once. A value that comes from a spec (HTTP `200`, a protocol magic byte, a fixed header size) always gets a constant, even if it's used once, because the spec is the reason it can't change.

### Flatten Nesting
Avoid the arrow anti-pattern. Reach for early return and early continue before adding another level of `if`.

```go
// wrong
func process(item *Item) error {
    if item != nil {
        if item.Valid() {
            if item.Ready() {
                return commit(item)
            }
        }
    }
    return nil
}

// right
func process(item *Item) error {
    if item == nil || !item.Valid() || !item.Ready() {
        return nil
    }
    return commit(item)
}
```

### Short Function Names
Keep a function name under 30 characters. If it doesn't fit, the function is probably doing too much or the name is carrying detail that belongs in a doc comment instead.

### Enums Over Boolean Parameters
A bare `true`/`false` at a call site tells the reader nothing. Use an enum:

```ts
// wrong
scheduleJob(job, true);

// right
scheduleJob(job, Priority.High);
```

### Let the Code Breathe
Blank line between logical blocks: setup, then the operation, then the return. Not between every two statements, between distinct steps.

### Comment the Block, Not the Line
A block-level comment states what the block does and why, with an example or a small ASCII drawing when a system's shape is the point. This is about summarizing a block's purpose, not narrating individual lines, rule 7 (no line restates the line below it) still applies within the block.

```go
// Coalesces concurrent refreshes into one outbound request.
//
//   caller A ─┐
//   caller B ─┼─> mutex.Lock() -> single refresh -> broadcast to all
//   caller C ─┘
func refreshToken() (*Token, error) { ... }
```

### Private by Default, Public by Approval
Every field and function starts private. Widening a private member to internal or public is a design decision, not a formatting one, ask dreygur before making that change. Treat it as a breaking shift to the type's contract even if the compiler stays quiet.

### Encapsulate the Low Level
Raw hardware I/O, sector parsing, direct socket streams: these live behind a dedicated driver or abstraction layer. The rest of the application calls a clean, high-level API that speaks in domain concepts (`ReadSector(lba)`, not `ioctl(fd, ...)` scattered through business logic).

### Layers Talk Only to Their Neighbor
A controller or UI component never calls a database query, a raw hardware driver, or a low-level network client directly. It goes through the service layer immediately below it. No skipping a layer to save a hop, that hop is the boundary that makes the layer swappable.

### Touch Only What the Task Requires
Don't edit, comment, or reformat a block you didn't write or need to change for this feature. Minimize the diff. A drive-by comment on code you didn't touch is scope creep, not helpfulness.

### Braces Always
Even a one-line `if` gets `{}`. No exceptions for brevity.

```go
// wrong
if err != nil
    return err

// right
if err != nil {
    return err
}
```

### Commit Message Format

1. Blank line between subject and body, always.
2. Subject line under 50 characters, 72 is the hard ceiling.
3. Capitalize the first letter of the subject.
4. No period at the end of the subject line.
5. Imperative mood in the subject: `Fix bug`, `Add feature`, never `Fixed` or `Adds`. Test: the subject must complete "If applied, this commit will ___."
6. Wrap the body by hand at 72 characters.
7. The body explains what and why, not how, the diff already shows how. State the context and reasoning that isn't visible in the code.

---

## Behavior: Karpathy Guidelines

Behavioral guidelines against the common LLM coding mistakes, from [Andrej Karpathy's observations](https://x.com/karpathy/status/2015883857489522876) (MIT, adapted from `multica-ai/andrej-karpathy-skills`). Full wrong/right examples for every principle: `references/behavior-examples.md`.

The mistakes they target, in Karpathy's words:

> The models make wrong assumptions on your behalf and just run along with them without checking. They don't manage their confusion, don't seek clarifications, don't surface inconsistencies, don't present tradeoffs, don't push back when they should.

> They really like to overcomplicate code and APIs, bloat abstractions, don't clean up dead code... implement a bloated construction over 1000 lines when 100 would do.

> They still sometimes change/remove comments and code they don't sufficiently understand as side effects, even if orthogonal to the task.

**Tradeoff:** these bias toward caution over speed. For trivial tasks (a typo fix, an obvious one liner) use judgment; not every change needs the full rigor. The goal is fewer costly mistakes on non trivial work, not slower simple work.

| Principle | Addresses |
|---|---|
| Think Before Coding | Wrong assumptions, hidden confusion, missing tradeoffs |
| Simplicity First | Overcomplication, bloated abstractions |
| Surgical Changes | Orthogonal edits, touching code you shouldn't |
| Goal Driven Execution | Leverage through tests first, verifiable success criteria |

### 1. Think Before Coding
**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask rather than guess.
- If multiple interpretations exist, present them. Don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

### 2. Simplicity First
**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

The test: would a senior engineer say this is overcomplicated? If yes, simplify. The overbuilt version is usually not wrong, it follows real patterns. The problem is timing: it adds complexity before it is needed, which makes the code harder to read, test, and get right. Solve today's problem simply, refactor when the second requirement actually arrives.

### 3. Surgical Changes
**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently (quote style, type hints, spacing, return shape).
- If you notice unrelated dead code, mention it. Don't delete it.

When your changes create orphans:
- Remove imports, variables, and functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: every changed line traces directly to the user's request. Pairs with Touch Only What the Task Requires above.

### 4. Goal Driven Execution
**Define success criteria. Loop until verified.**

Turn imperative tasks into verifiable goals:

| Instead of | Transform to |
|---|---|
| "Add validation" | "Write tests for invalid inputs, then make them pass" |
| "Fix the bug" | "Write a test that reproduces it, then make it pass" |
| "Refactor X" | "Ensure tests pass before and after" |

For multi step tasks, state a brief plan:

```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") force constant clarification. Karpathy: "LLMs are exceptionally good at looping until they meet specific goals... Don't tell it what to do, give it success criteria and watch it go." Pairs with Universal Rule 13 (bug fixes start with a failing test).

### Signs It Works
- Fewer unnecessary changes in diffs; only requested changes appear.
- Fewer rewrites from overcomplication; the code is simple the first time.
- Clarifying questions come before implementation, not after mistakes.
- Clean, minimal PRs with no drive by refactoring or "improvements".

### Project Specific Additions
These guidelines merge with project instructions. A project may add its own on top (strict mode, tests on every endpoint, follow the error pattern in `src/utils/errors.ts`). Project rules win where they conflict.

---

## TypeScript / JavaScript

### Structure
```
src/
├── index.ts        # barrel exports
├── types.ts
├── constants.ts
├── config.ts
├── errors.ts
├── services/       # *.service.ts
├── repositories/   # *.repository.ts
├── strategies/     # *.strategy.ts
├── middleware/     # *.middleware.ts
├── handlers/       # *.handler.ts
└── utils/
    ├── logger.ts
    └── mutex.ts
```

### Error Classes
```ts
export class AppError extends Error {
  constructor(
    message: string,
    public code: string,
    public recoverable = true,
    public userMessage?: string,
  ) {
    super(message);
    this.name = "AppError";
    Error.captureStackTrace(this, this.constructor);
  }
}
export class NetworkError extends AppError {
  constructor(message: string, userMessage?: string) {
    super(message, "NETWORK_ERROR", true, userMessage || "Network request failed.");
    this.name = "NetworkError";
  }
}
```

```ts
export function isRecoverableError(e: unknown): boolean {
  return e instanceof AppError ? e.recoverable : false;
}
export function getUserMessage(e: unknown): string {
  if (e instanceof AppError && e.userMessage) return e.userMessage;
  if (e instanceof Error) return e.message;
  return "An unexpected error occurred";
}
```

### Rules
- **Always semicolons**
- `import type` for type-only imports
- `async/await` everywhere, no `.then()/.catch()` chains
- `index.ts` re-exports everything (`export * from "./services/token.service.js"`)
- Classes: `PascalCase` + suffix, as in `TokenService`, `CredentialRepository`
- Instances: `camelCase`; Constants: `SCREAMING_SNAKE_CASE`

### Log Prefix (even without logger)
```ts
console.log(`[+] Waiting...`);
console.log(`[-] ${err.message}`);
```

### Graceful Shutdown
```ts
process.on('SIGINT', (signal) => { console.log(`[+] ${signal} Exiting...`); process.exit(0); });
process.on('SIGHUP', (signal) => { console.log(`[+] ${signal} Exiting...`); process.exit(0); });
```

### Script Entry (IIFE)
```ts
(async () => await main())();
```

### Branded Types for Entity IDs
```ts
export type Brand<T, B extends string> = T & { __brand: B };
export function asBrand<T extends Brand<any, any>>(value: any): T { return value as T; }
export function assertValidId<T extends Brand<string, any>>(value: unknown, idType: string): T {
  if (!isValidUUID(value)) throw new Error(`Invalid ${idType}: expected UUID`);
  return value as T;
}
export type UserId = Brand<string, 'UserId'>;
export type OrganizationId = Brand<string, 'OrganizationId'>;
```
Never use plain `string` for entity IDs. Always the specific branded type.

### Drizzle ORM Schema
```ts
export const users = pgTable('user', {
  id: uuid('id').notNull().primaryKey().defaultRandom(),
  email: varchar('email').notNull().unique(),
  ...timestamps,
});
export type TUser = typeof users.$inferSelect;
export type TUserInsert = typeof users.$inferInsert;
export const ZUserCreate = createInsertSchema(users);
export const ZUserUpdate = createUpdateSchema(users);
export const ZUserSelect = createSelectSchema(users);
```

### Base Repository (Drizzle)
```ts
export abstract class BaseRepository<T extends AnyPgTable & { id: any }, TId extends string = string> {
  constructor(protected readonly db: NodePgDatabase, protected readonly table: T) {}

  async findById(id: TId): Promise<T['$inferSelect']> { ... }
  async create(data: Partial<T['$inferInsert']>): Promise<TId> { ... }
  async update(id: TId, values: Partial<T['$inferInsert']>): Promise<TId> { ... }

  async delete(id: TId, tx?: ConnectionType) {
    return (tx ?? this.db).update(this.table).set({ deleted: true } as any)
      .where(eq(this.table.id, id as string)).execute();
  }
  async restore(id: TId, tx?: ConnectionType) {
    return (tx ?? this.db).update(this.table).set({ deleted: false } as any)
      .where(eq(this.table.id, id as string)).execute();
  }
  async withTransaction<R>(fn: (tx: ConnectionType) => Promise<R>): Promise<R> {
    return this.db.transaction(fn as any);
  }
}
```
Always soft delete (`deleted: true`). Optional `tx?` on mutating methods.

### tRPC Setup
```ts
export const t = initTRPC.context<Context>().meta<OpenApiMeta>().create({
  transformer: superjson,
  errorFormatter: ({ shape }) => ({
    ...shape,
    data: { ...shape.data, stack: process.env.NODE_ENV === 'development' ? shape.data?.stack : undefined },
  }),
});
export const router = t.router;
export const middleware = t.middleware;
export const createCallerFactory = t.createCallerFactory;
```

### Project-Prefixed Errors
```ts
export class PoachApiError extends Error {
  constructor(message: string, public success = false, public operational = true, stack = '') {
    super(message);
    this.name = 'PoachApiError';
    if (stack) this.stack = stack;
    else Error.captureStackTrace(this, this.constructor);
  }
}
export class PTRPCError extends TRPCError {
  public timestamp = new Date();
  public requestId?: string;
  constructor(params: ConstructorParameters<typeof TRPCError>[0] & { requestId?: string }) {
    super(params);
    this.requestId = params.requestId;
  }
}
```

### Winston Logger with Custom Levels
```ts
declare module 'winston' { interface Logger { db(msg: string | Error, ...meta: any[]): Logger; } }

const customLevels = {
  levels: { error: 0, warn: 1, info: 2, db: 3, debug: 4 },
  colors: { error: 'red', warn: 'yellow', info: 'green', db: 'magenta', debug: 'blue' },
};
winston.addColors(customLevels.colors);

export const logger = winston.createLogger({
  levels: customLevels.levels,
  level: process.env.NODE_ENV === 'development' ? 'debug' : 'db',
  format: winston.format.combine(
    winston.format.timestamp({ format: 'YYYY-MM-DD HH:mm:ss' }),
    winston.format.printf(({ timestamp, level, message }) => `[${level}] ${timestamp}: ${message}`),
  ),
  transports: [new winston.transports.Console({ stderrLevels: ['error'] })],
});
```

### Monorepo (Turborepo + pnpm)
```
project/
├── apps/api/           # Express + tRPC
├── apps/ui/            # Next.js App Router
├── packages/
│   ├── schema/         # Drizzle tables + drizzle-zod
│   ├── repository/     # BaseRepository + per-entity
│   ├── service/        # *.service.ts per entity
│   ├── trpc/           # router + context + middleware
│   ├── shared/         # branded-types, errors, constants
│   ├── validator/      # Zod validators
│   ├── logger/         # Winston
│   ├── email/          # provider pattern
│   └── telemetry/      # OpenTelemetry
├── cdk/                # AWS CDK (if cloud)
├── turbo.json
└── pnpm-workspace.yaml
```
Package naming: `@projectname/schema`, `@projectname/repository`, etc.

---

## Go

### Structure
**Libraries:**
```
project/
├── main.go  ├── go.mod  ├── Makefile  ├── sample.env
├── models/  ├── methods/interface.go  ├── hooks/  ├── handlers/
├── lib/  ├── utils/  ├── db/  └── tests/
```
**Applications:**
```
project/
├── main.go  ├── go.mod  ├── Makefile  ├── .env.example
└── internal/
    ├── models/  ├── config/  ├── ai/  ├── discord/
    ├── google/  ├── store/  └── worker/
```

### Interface First
```go
// methods/interface.go
type BkashService interface {
    GetToken() (*models.TokenResponse, error)
    CreatePayment(*models.CreateRequest, *models.TokenResponse) (*models.CreatePaymentResponse, error)
}
// Constructor returns interface, not concrete type
func GetBkash(appKey, appSecret string, isLive bool) methods.BkashService {
    return &methods.Bkash{AppKey: appKey, IsLiveStore: isLive}
}
```

### Error Handling
```go
if !utils.RequireNonEmpty(b.AppKey, b.AppSecret) {
    return nil, common.ErrEmptyRequiredField
}
if err != nil {
    return fmt.Errorf("fetch reviews: %w", err)
}
```
Bare `return nil, err` when no context adds value; `fmt.Errorf("context: %w", err)` when it does.

### Typed String Constants
```go
type ApprovalStatus string
const (
    StatusPending  ApprovalStatus = "pending"
    StatusApproved ApprovalStatus = "approved"
    StatusRejected ApprovalStatus = "rejected"
)
```

### Constructors That Can Fail
```go
func NewBot(token, channelID string) (*Bot, error) {
    session, err := discordgo.New("Bot " + token)
    if err != nil { return nil, fmt.Errorf("create discord session: %w", err) }
    return &Bot{session: session, channelID: channelID}, nil
}
```

### Concurrent-Safe Structs
```go
type Bot struct {
    session *discordgo.Session
    mu      sync.RWMutex
    pending map[string]*models.PendingApproval
}
func (b *Bot) isUserAllowed(id string) bool {
    b.mu.RLock(); defer b.mu.RUnlock()
    return b.allowedUsers[id]
}
```

### Primary + Fallbacks
```go
type Generator struct { providers map[string]Provider; primary string; fallbacks []string }

for _, name := range append([]string{g.primary}, g.fallbacks...) {
    if resp, err := providers[name].Generate(ctx, req); err == nil { return resp, nil }
}
```

### Map-Based Dispatch
```go
var Handlers = map[string]func(*discordgo.Session, *discordgo.InteractionCreate){
    "help": Help, "points": Points,
}
if fn, ok := Handlers[name]; ok { fn(s, i) }
```

### Graceful Shutdown
```go
sig := make(chan os.Signal, 1)
signal.Notify(sig, os.Interrupt)
<-sig
```

### Logging
```go
func PrintLog(text, status string) {
    switch status {
    case "info":  color.New(color.FgGreen).Println("[+] " + text)
    case "error": color.New(color.FgRed).Println("[-] " + text)
    case "warn":  color.New(color.FgYellow).Println("[!] " + text)
    default:      color.New(color.FgCyan).Println("[*] " + text)
    }
}
```

---

## Rust

### Structure
```
project/
├── Cargo.toml  ├── src/main.rs  ├── src/lib.rs  ├── src/error.rs
├── src/config/mod.rs  └── crates/*/src/{lib.rs,error.rs}
```

### Error Types (`thiserror` always)
```rust
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Transport: {0}")] Transport(String),
    #[error("IO: {0}")] Io(#[from] std::io::Error),
    #[error("JSON: {0}")] Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, AppError>;
```
`anyhow`/`eyre` only in binary entry points. Library crates use typed `Result` alias.

### Binaries
```rust
#[tokio::main]
async fn main() -> eyre::Result<()> { ... }  // preferred in newer projects
// or anyhow::Result<()>
```

### Shared State
```rust
let store = Arc::new(Store::new());
tokio::spawn(async move {
    let store = Arc::clone(&store);  // explicit Arc::clone, not .clone()
    handle(stream, store).await
});
```

### tokio::select!
```rust
loop {
    tokio::select! {
        Some(event) = events.next() => { ... }
        _ = sleep(Duration::from_secs(30)) => { /* tick */ }
    }
}
```

### CLI (clap)
```rust
#[derive(Parser)]
#[command(name = "app", version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)] command: Option<Commands>,
    #[arg(long, global = true)] debug: bool,
}
#[derive(Subcommand)]
enum Commands { Init { #[arg(long)] force: bool }, Serve }
```

### lib.rs Re-exports
```rust
pub mod error; pub mod client;
pub use error::{AppError, Result};
pub use client::Client;
```

### Logging
```rust
use tracing::{debug, error, info, warn};
info!("started"); warn!("retry {attempt}"); error!("{err}");
```

---

## Python

### Structure
```
package/
├── __init__.py  ├── base.py  ├── payment.py  └── utils.py
Tests/
└── test_general.py
```

### Rules
- Shebang: `#!/usr/bin/env python`
- **2-space indent** (deliberate)
- Type hints on every method signature
- Docstrings with `Args:` / `Returns:` on every public method
- `@staticmethod` for pure utilities

### Base + Subclass + Accumulator
```python
class SSLCommerz:
  def __init__(self, is_sandbox: bool = True) -> None:
    self.integration_data: Dict[str, str] = {}

class SSLCSession(SSLCommerz):
  def set_customer_info(self, name: str, email: str) -> None:
    self.integration_data.update({'cus_name': name, 'cus_email': email})

  def init_payment(self) -> Dict:
    return requests.post(self.session_api, self.integration_data).json()
```

---

## PHP

### WordPress Plugins

#### Plugin Header + Security Guard
```php
<?php
/**
 * Plugin Name:    MyPlugin
 * Version:        1.0.0
 * Author:         Name
 * Text Domain:    myplugin
 * Domain Path:    /languages
 */
defined('ABSPATH') || exit;  // top of EVERY PHP file
```

#### Complex Plugin: `final` Singleton
```php
final class MyPlugin {
    private static $instance = null;
    public static function get_instance(): self {
        if (null === self::$instance) self::$instance = new self();
        return self::$instance;
    }
    private function __construct() { $this->init(); }
    private function init(): void {
        $this->load_dependencies();
        $this->define_constants();
        $this->init_hooks();
    }
    private function load_dependencies(): void { require_once __DIR__ . '/vendor/autoload.php'; }
    private function define_constants(): void {
        define('MYPLUGIN_VERSION', '1.0.0');
        define('MYPLUGIN_URL', plugin_dir_url(__FILE__));
        define('MYPLUGIN_PATH', plugin_dir_path(__FILE__));
    }
    private function init_hooks(): void {
        add_action('plugins_loaded', [$this, 'init_feature'], 100);
    }
}
MyPlugin::get_instance();
```

#### Minimal Plugin: Direct `new`
Single-responsibility plugins skip singleton:
```php
class SingleSession {
    public function __construct() {
        add_action('wp_login', [$this, 'force_single_session'], 10, 2);
    }
    public function force_single_session($login, $user) {
        if (is_a($user, 'WP_User')) {
            $sessions = get_user_meta($user->ID, 'session_tokens', true);
            if ($sessions && is_array($sessions))
                update_user_meta($user->ID, 'session_tokens', array_slice($sessions, -1));
        }
    }
}
new SingleSession();
```

#### `final class Init` Per Feature
All hooks in constructor:
```php
final class Init {
    public function __construct() {
        add_filter('plugin_gateways', [self::class, 'add_gateways']);
        add_filter('plugin_payment_methods', [$this, 'add_method'], 100);
        add_action('init', [$this, 'process_form']);
    }
}
```

#### `private const` for Config + Status Maps
```php
private const STATUS_MAP = ['VALID' => 'paid', 'FAILED' => 'failed', 'CANCELLED' => 'cancelled'];
private function mapStatus(string $s): string { return self::STATUS_MAP[$s] ?? 'failed'; }
```

#### WP HTTP API (not curl/Guzzle)
```php
$response = wp_remote_post($url, ['timeout' => 60, 'sslverify' => !$isSandbox, 'body' => $data]);
if (is_wp_error($response)) return ['status' => 'FAILED', 'reason' => $response->get_error_message()];
$code = wp_remote_retrieve_response_code($response);
$body = wp_remote_retrieve_body($response);
```

#### Input + i18n
```php
// Sanitize all $_POST
$val = sanitize_text_field(wp_unslash($_POST['key']));
// All user-facing strings
throw new \InvalidArgumentException(__('Order ID required', 'myplugin'));
```

#### Error Handling
```php
// Catch Throwable not Exception (PHP 7+)
try { $this->process($data); } catch (Throwable $e) {
    if (defined('WP_DEBUG') && WP_DEBUG) error_log('Error: ' . $e->getMessage());
    $result->status = 'failed'; $result->reason = $e->getMessage(); return $result;
}
// Validate early
if (empty($data->order_id)) throw new \InvalidArgumentException(__('Order ID required', 'myplugin'));
```

#### Vue 3 + Vite Admin UI
```php
// In plugin class __construct():
add_action('admin_enqueue_scripts', [$this, 'loadAssets']);
add_action('admin_menu', [$this, 'adminMenu']);
add_filter('script_loader_tag', [$this, 'asModule'], 10, 3);  // WP can't do type="module" natively

function asModule($tag, $handle, $src) {
    if ('my-handle' !== $handle) return $tag;
    return '<script type="module" src="' . esc_url($src) . '"></script>';
}
function loadAssets() {
    // Dev: Vite HMR. Prod: dist/assets/main.js
    wp_enqueue_script('my-handle', '//localhost:5173/src/main.js', [], time(), true);
    wp_localize_script('my-handle', 'myplugin', [
        'url' => plugin_dir_url(__FILE__), 'nonce' => wp_create_nonce('wp_rest'), 'api' => get_rest_url(),
    ]);
}
```
Admin page template: just `<div class="wrap"><div id="app"></div></div>`

Vue entry (`src/main.js`):
```js
import { createApp } from 'vue';
import App from './App.vue';
createApp(App).mount('#app');
```
Access localized data: `window.myplugin.url`. `<script setup>` in all SFCs.

`vite.config.js`:
```js
import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
export default defineConfig({ plugins: [vue()] });
```

#### Plugin Structures
```
# Complex:                          # Minimal:
plugin/                             plugin/
├── plugin.php (singleton)          └── plugin.php (header + new ClassName())
├── integration/Init.php
├── payments/Provider/Provider.php
├── assets/
└── vendor/
```

---

### PHP CLI (Symfony Console)

#### Structure
```
tool/
├── bin/toolname        # #!/usr/bin/env php entry
├── src/
│   ├── Commands/       # one class per command
│   └── Extensions/     # custom Application
└── composer.json       # "bin": ["bin/toolname"]
```

#### CLI Entry
```php
#!/usr/bin/env php
<?php
set_time_limit(0);

foreach ([__DIR__.'/../../vendor/autoload.php', __DIR__.'/../../../../autoload.php', 'vendor/autoload.php'] as $f) {
    if (file_exists($f)) { require $f; break; }
}

$app = new ToolApplication('My CLI', 'v1.0.0');
$app->add(new InstallCommand);
$app->run();
```
`composer.json`: `"bin": ["bin/toolname"]`, `"require": {"symfony/console": "^5.2"}`

#### Custom Application: Strip Unused Options
```php
class ToolApplication extends Application {
    protected function getDefaultInputDefinition(): InputDefinition {
        return new InputDefinition([
            new InputArgument('command', InputArgument::REQUIRED, 'Command'),
            new InputOption('--help', '-h', InputOption::VALUE_NONE, 'Help'),
        ]);
    }
}
```

#### Command Pattern
```php
class InstallCommand extends Command {
    const Q_INPUT = 1; const Q_CONFIRM = 2; const Q_CHOICE = 3;
    private $input; private $output;

    protected function configure(): void {
        $this->setName('install')->setDescription('Install project')
             ->addArgument('name', InputArgument::REQUIRED, 'Name');
    }
    protected function execute(InputInterface $input, OutputInterface $output): int {
        $this->input = $input; $this->output = $output;
        $name = $this->ask('Project name:', self::Q_INPUT, 'myproject');
        if ($this->ask('Proceed?', self::Q_CONFIRM) === 'yes') {
            $output->writeln('Installing...');
        }
        return Command::SUCCESS;
    }
    private function ask(string $q, int $type, string $default = '', array $opts = []): string {
        $helper = $this->getHelper('question');
        $question = match($type) {
            self::Q_INPUT   => new Question($q, $default),
            self::Q_CONFIRM => new ChoiceQuestion($q, ['yes','no'], $default),
            self::Q_CHOICE  => new ChoiceQuestion($q, $opts, $default),
        };
        return $helper->ask($this->input, $this->output, $question);
    }
}
```
`$output->writeln()`, never `echo`. Return `Command::SUCCESS`/`Command::FAILURE`.

#### Cross-Platform + State
```php
$isWindows = in_array(PHP_OS, ['WIN32', 'Windows', 'WINNT']);
passthru($isWindows ? "php -S {$domain}:80" : "sudo php -S {$domain}:80 -t {$domain}/");

// State as JSON
file_put_contents("{$name}.json", json_encode(['domain' => $domain], JSON_PRETTY_PRINT));
if (file_exists("{$name}.json")) { /* already exists */ }
```

---

### Laravel Packages

#### Structure
```
package/
├── src/
│   ├── Http/{Controllers/,Middleware/}
│   ├── Providers/PackageServiceProvider.php
│   ├── Concerns/     # Eloquent traits (HasCrud)
│   ├── Traits/       # general PHP traits
│   └── Support/
├── config/package.php
├── database/migrations/
├── resources/views/
├── routes/web.php
└── composer.json
```

#### ServiceProvider
```php
class PackageServiceProvider extends ServiceProvider {
    public function register(): void {
        $this->mergeConfigFrom(__DIR__.'/../../config/package.php', 'package');
    }
    public function boot(): void {
        $this->registerPublishing();
        $this->loadMigrationsFrom(__DIR__.'/../../database/migrations');
        $this->registerRoutes();
        $this->registerViews();
        $this->registerViewComposers();
        $this->registerMiddleware();
        $this->registerCommands();
        $this->registerEventListeners();
    }
    // one protected method per concern above
}
```

#### Config-Driven
```php
$model = config('package.user_model', 'App\\Models\\User');
$roles = config('package.admin_roles', ['admin', 'super-admin']);
$disk  = config('package.uploads.disk', 'public');
```

#### Abstract BaseController
```php
abstract class BaseController extends Controller {
    protected function isAdmin(): bool {
        $user = auth()->user();
        if (!$user || !method_exists($user, 'roleSlugs')) return false;
        return !empty(array_intersect(config('package.admin_roles', ['admin']), $user->roleSlugs()));
    }
    protected function getViewData(array $data = []): array {
        return array_merge(['isAdmin' => $this->isAdmin(), 'user' => auth()->user()], $data);
    }
}
```

#### HasCrud Concern
```php
trait HasCrud {
    public static function getResourceConfig(): array {
        $i = new static;
        return [
            'model'    => static::class,
            'title'    => $i->resourceTitle ?? Str::title(Str::plural(Str::snake(class_basename(static::class)))),
            'fields'   => $i->resourceFields ?? static::getCachedFieldsOrGenerate($i),
            'roles'    => $i->resourceRoles ?? [],
            'readonly' => $i->resourceReadonly ?? [],
        ];
    }
    public static function getResourceKey(): string {
        $i = new static;
        return $i->resourceKey ?? Str::plural(Str::snake(class_basename(static::class)));
    }
}
```
Cache fields with content-hash key: `Cache::get('pkg_'.md5($class).'_'.md5(serialize($fillable)))`.

#### Routes
```php
Route::group([
    'prefix' => config('package.routes.prefix', 'dashboard'),
    'middleware' => config('package.routes.middleware', ['web', 'auth']),
], fn() => $this->loadRoutesFrom(__DIR__.'/../../routes/web.php'));

// Dev/example routes
if (!app()->environment('production')) {
    Route::get('/components', [ComponentsController::class, 'components']);
}
```

#### Granular Publishing
```php
$this->publishes([__DIR__.'/../../config/package.php' => config_path('package.php')], 'package-config');
$this->publishes([$views => resource_path('views/vendor/package')], 'package-views');
$this->publishes([$views.'/partials/styles.blade.php' => ...], 'package-styles');
// + combined 'package' group
```

#### Middleware + View Composers + Commands
```php
protected function registerMiddleware(): void {
    $this->app['router']->aliasMiddleware('package.admin', EnsureIsAdmin::class);
    $this->app['router']->pushMiddlewareToGroup('web', HandleImpersonation::class);
}
View::composer(['package::*'], fn($v) => $v->with('user', auth()->user()));
protected function registerCommands(): void {
    if (!$this->app->runningInConsole()) return;
    $this->commands([InstallCommand::class, MakeResourceCommand::class]);
}
```

#### Naming
- Classes: `PascalCase` + role, as in `ResourceController`, `EnsureIsAdmin`, `HasCrud`
- Config keys: `kebab-case.dot.notation`, as in `package.admin_roles`
- Blade: `kebab-case.blade.php` namespaced, as in `package::partials.flash-messages`
- PHPDoc `@param`/`@return` on every public method

---

## Writing Like a Human

Every character that ships is text someone reads: comments, doc comments, README and docs, commit messages, PR bodies, error strings, CLI output, variable names, test names. All of it should read as if dreygur typed it. Generated-sounding prose is as much a defect as a swallowed error.

### 1. No Dashes
Never `—`, never `–`, never a hyphen used as punctuation or as a word-joiner. Not in code, comments, commits, docs, PR bodies, or chat about the work. It is the loudest machine tell there is.

Rewrite instead:

| Instead of | Write |
|---|---|
| `token expired — refresh first` | `token expired, refresh first` |
| `Two modes — sandbox and live` | `Two modes: sandbox and live` |
| `It fails silently — the caller never knows` | `It fails silently. The caller never knows.` |
| `The retry — when enabled — wraps fetch` | `The retry, when enabled, wraps fetch` |
| `a well-known trade-off` | `a widely known tradeoff` |
| `state-of-the-art` | `cutting edge` |
| `a fine-grained check` | `a granular check` |

Comma, colon, semicolon, parentheses, or a full stop. One of those always fits. For a compound word, either close it up (`tradeoff`, `nonblocking`) or say it as two words (`widely known`); never join it with a hyphen.

Also out: ` -- ` standing in for an em dash, `…` (write three periods, or nothing), curly quotes `“” ‘’` anywhere near code or config.

This is about prose, not code syntax. Structural hyphens stay exactly where the rest of this document already puts them: `kebab-case` filenames (Universal Rule 9), CLI flags like `--force`, and numeric ranges like `13-120`. Anywhere a hyphen is part of an identifier or a documented naming convention, it stays. Anywhere it is standing in for a comma, a colon, or gluing two words into an adjective, it goes.

### 2. Cut the Stock Vocabulary
These words are not wrong, they are just what a model reaches for first. Reach past them.

| Avoid | Use |
|---|---|
| delve into, dive into, explore | look at, read, check |
| leverage, utilize | use |
| robust, comprehensive, powerful | say what it actually does |
| seamless, effortless, elegant | drop the adjective |
| ensure that | make sure, or just the imperative |
| it's worth noting, it's important to note | state the thing |
| crucial, vital, essential | needed, required, or drop it |
| in order to | to |
| a wide range of, a variety of | name the range |
| unlock, elevate, streamline, supercharge | describe the change |
| additionally, furthermore, moreover | and, also, or start the sentence |

### 3. No Antithesis Reflex
Stop reaching for the contrast frame. `not just X, but Y`, `it isn't A, it's B`, `X isn't about A. It's about B.` Once in a long document is fine. Twice in a paragraph is a signature.

```
// wrong
// This isn't a cache, it's a coalescer.
// right
// Coalesces concurrent refreshes so only one request goes out.
```

### 4. No Padding
- No preamble restating the question or the task.
- No closing paragraph summarizing what the reader just read.
- No "Overview" / "Conclusion" / "Key Takeaways" headings on a document under two pages.
- No hedging stack: pick a position, or say plainly that it depends and on what.
- No apology or self-congratulation in commits, comments, or docs.

### 5. Vary the Shape
Uniform output reads generated even when every word is fine.

- Do not make every list exactly three items.
- Do not give every bullet a bolded lead-in and a colon.
- Do not open consecutive sentences with the same construction.
- Mix sentence lengths. A short one lands harder after a long one.
- Prose is allowed. Not every explanation needs to become a table or a bullet list.

### 6. No Emoji, No Decoration
None in code, comments, commit messages, docs, or CLI output. The log prefixes `[+]` `[-]` `[!]` `[*]` are the whole visual vocabulary. No checkmark bullets, no banner ASCII art, no `🚀` in a README.

### 7. Comments Are Notes, Not Narration
A comment records why, or a constraint that is not visible in the code. It never restates the line below it, and it never announces structure (see The Shape of a File, rule 5).

```go
// wrong
// Loop over the users and send each one an email.
for _, u := range users { ... }

// right
// bKash rate-limits to 5 req/s per app key; batching keeps us under it.
for _, batch := range chunk(users, 5) { ... }
```

Write them in the same clipped register as the code. Sentence fragments are fine. Trailing periods optional, but stay consistent within a file.

### 8. Commit Messages Sound Like a Person
- Imperative mood, present tense: `fix token refresh race`, not `Fixed` or `Fixes`.
- Say what changed and, when it is not obvious, why. Skip the body entirely for a one-line change.
- Never `This commit ...`, never a bulleted essay for a two-line diff, never a "Summary of changes" heading.
- No dashes in the subject or body: no em dash, no en dash, no hyphenated compound adjective.
- No agent attribution of any kind (see Commits & Attribution).

```
# wrong
feat: implement a comprehensive solution for token refresh — ensures seamless auth

# right
fix: coalesce concurrent token refreshes

Two requests racing on an expired token each triggered a refresh and the
second overwrote the first. Guard with the shared mutex.
```

### 9. Docs Assume a Reader With the Repo Open
Skip the marketing paragraph. Skip "In today's fast-paced world". Open with what the thing does and the first command that runs it. Show real values in examples, not `foo` and `bar`. If a section only exists because documents usually have one, delete it.

### 10. Identifiers and Strings Carry the Same Rule
No thesaurus names (`orchestrateComprehensiveSync`). Name it what it does: `syncPlaylist`. Error messages state the failure and the input, with no apology and no exclamation mark: `invalid order id: %q`, not `Oops! Something went wrong with the order!`.

---

## Reviewing Code: Flag These

- Missing types/interfaces for exported items
- DB calls in handlers (wrong layer)
- Raw error strings instead of typed errors
- Magic values not in constants
- Swallowed errors (`catch {}`, ignored `Result`, `except: pass`)
- Missing doc comments on exports
- Wrong filename casing
- Missing TS/JS semicolons
- 4-space indent in Python
- Raw `console.log`/`print`/`println!` in library code
- Services instantiated per-call instead of singleton export
- `$_POST` without sanitization in PHP
- Missing `defined('ABSPATH') || exit` in WP files
- Helper defined *below* its caller
- Constants buried mid-file instead of at the top
- Section-divider or block-label comments (`// ---- Helpers ----`)
- Casts that silence the type checker (`as any`, `/** @type {X} */ (expr)`)
- New function with no test that actually runs it
- Test doubles named `fake_*` instead of `mock_*`
- Any AI/agent attribution in commits, comments, or docs
- Dashes anywhere in prose: em dash, en dash, or a hyphen joining two words into a compound (code, comment, doc, commit, PR body)
- Curly quotes or `...` characters in text
- Stock model vocabulary (delve, leverage, robust, seamless, ensure that, it's worth noting)
- `not just X, but Y` phrasing, or any repeated antithesis frame
- Comment that restates the line below it
- Commit subject in past tense, or a bulleted essay on a two-line diff
- Emoji in code, commits, docs, or CLI output
- Preamble or a summary paragraph on a doc that does not need one
- Nested `if` three or more levels deep where early return would flatten it
- Boolean parameter at a call site instead of an enum
- Function name at or over 30 characters
- A field or method made `public`/`internal` without being asked
- A controller, handler, or UI component calling a DB query, driver, or raw socket directly, skipping the service layer
- Missing `{}` on a single-line `if`
- Comment or edit touching code unrelated to the current task
- Bug-fix diff with no failing test that preceded it
- Silent assumption or silently chosen interpretation of an ambiguous request
- Abstraction, option, or feature beyond what was asked (single use strategy classes, unrequested config, error handling for impossible cases)
- Drive by edit: reformatted lines, changed quote style, added type hints or docstrings, rewritten comments outside the task
- Pre-existing dead code deleted, or orphans from the change left behind
- Task with no verifiable success criteria or no plan with verify steps

## Commits & Attribution

- **Never** add agent attribution anywhere. No `Co-Authored-By: Claude`, no "Generated with Claude Code", no robot footers. Not in commit messages, PR bodies, code comments, or docs. The history reads as dreygur's own work. Where the repo supports it, set `attribution.commit` and `attribution.pr` to `""` in `.claude/settings.json` rather than relying on memory.
- Finish every outstanding request **before** committing. A commit is a checkpoint; reaching it with an open ask captures the wrong state.
- No unrequested edits to `README` files. Requested prose goes in chat unless a file is named.

## Additional Resources
- **`references/patterns.md`**: extended patterns and anti-patterns per language, including before/after rewrites for machine-sounding prose
- **`references/behavior-examples.md`**: wrong/right examples for each Karpathy principle
