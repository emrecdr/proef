# Architecture

> Template baseline — the project's own `CLAUDE.md` and documented conventions WIN on any conflict with this file. Tailor it to the project (`/devt:setup`); untailored copies are flagged by `/devt:setup --health`.

Devt-integration document for Rust projects. Read by the `architect` agent and `architecture-health-scanner` skill. Defines layering rules, structural patterns, and the trade-off space agents should respect when making architectural changes.

## Pattern: Clean Architecture (Hexagonal / Ports-and-Adapters)

Same shape as the Python clean-architecture template: pure domain at the center, application use-cases above, infrastructure at the periphery, presentation outside everything.

```
+------------------------------------------+
|         Presentation (api / cli)         |   <- HTTP handlers, CLI commands, gRPC services
|  +----------------------------------+    |
|  |     Application (use cases)      |    |   <- orchestration; depends on domain + ports
|  |  +---------------------------+   |    |
|  |  |       Domain (core)       |   |    |   <- pure types, business invariants, NO deps
|  |  +---------------------------+   |    |
|  +----------------------------------+    |
|        Infrastructure (adapters)         |   <- repository impls, HTTP clients, file I/O
+------------------------------------------+
```

## Layer Responsibilities

### Domain Layer (`crate::domain` or `crates/domain`)

Pure business logic. The domain crate MUST NOT depend on:

- Infrastructure (databases, HTTP clients, file system, env vars)
- Application or Presentation
- Any crate that performs I/O

The domain crate MAY depend on:

- `std` (when no_std support is not a requirement)
- `serde` for serialization (when domain values cross network/storage boundaries)
- Validation crates (`validator`, `garde`) for declarative invariants
- Time crates (`chrono`, `time`) — but inject `Clock` as a trait for testability

Contents:

- Newtype wrappers for primitives that carry semantic meaning (`UserId`, `Email`, `IsoCountryCode`)
- Domain value objects (immutable types with invariants enforced at construction)
- Domain events (structs/enums representing things that happened)
- Domain errors (`thiserror`-based) describing business-rule violations
- Repository TRAITS only — never implementations

### Application Layer (`crate::application` or `crates/application`)

Use cases / interactors orchestrate domain logic with injected ports. The application crate depends on:

- Domain (always)
- Port traits defined locally (or in domain when they describe domain-driven boundaries)

The application crate MUST NOT depend on:

- Infrastructure (concrete adapters)
- Presentation

Contents:

- Use-case structs/functions taking dependencies via trait bounds
- Command and query types (input DTOs)
- Result types (output DTOs)
- Workflow orchestration that composes domain operations

### Infrastructure Layer (`crate::infrastructure` or `crates/infrastructure`)

Concrete adapters for ports. Lives outside the dependency direction — depends on domain + application but is depended upon by NOBODY at compile time (only at runtime via DI wiring at the composition root).

Contents:

- Repository implementations (`SqlxUserRepository`, `RedisCacheRepository`)
- HTTP client adapters
- File system access
- Time provider concrete implementations
- External service clients

Depends on:

- Domain (for entity types)
- Application (for port traits)
- External crates (sqlx, reqwest, etc.)

### Presentation Layer (`crate::api`, `crate::cli`, or `crates/api`)

User-facing surfaces. Depends on application use-cases (NOT on infrastructure directly). Each presentation channel is independent:

- HTTP/REST: route handlers using axum, actix-web, or rocket
- gRPC: tonic services
- CLI: clap-based command parsers
- WebSocket: tokio-tungstenite session handlers
- GraphQL: async-graphql resolvers

Composition root (typically `main.rs`) wires concrete infrastructure adapters into application use-cases.

## Dependency Direction (Hard Invariant)

```
Presentation -> Application -> Domain
Infrastructure -> Application -> Domain
Presentation -> Infrastructure  (only at the composition root, via main.rs DI wiring)
```

Domain depends on NOTHING. Application depends only on Domain. Infrastructure and Presentation depend only on lower layers.

Violations:

- `domain/` importing `infrastructure/` — CRITICAL violation (logic depending on adapters)
- `application/` importing `infrastructure/` — CRITICAL violation (use case depending on concrete DB)
- `domain/` importing `application/` — HIGH violation (core depending on orchestration)
- `presentation/` importing `infrastructure/` outside `main.rs` — HIGH violation (handler escaping DI)

## Crate Organization

### Single-Crate Projects

For libraries and small applications, use a single crate with modules expressing layers:

```
src/
├── lib.rs              # crate root
├── domain.rs           # or domain/
├── application.rs      # or application/
├── infrastructure.rs   # or infrastructure/
└── api.rs              # or api/
```

`lib.rs` re-exports the public API. Private modules stay private.

### Workspace Projects (multi-crate)

For larger applications, prefer a Cargo workspace with one crate per layer:

```
Cargo.toml              # workspace manifest
crates/
├── domain/
│   ├── Cargo.toml      # no external deps beyond std/serde/thiserror
│   └── src/lib.rs
├── application/
│   ├── Cargo.toml      # depends on domain
│   └── src/lib.rs
├── infrastructure/
│   ├── Cargo.toml      # depends on domain + application + external adapters
│   └── src/lib.rs
└── api/                # the main binary
    ├── Cargo.toml      # depends on application + infrastructure
    └── src/main.rs
```

Crate-level enforcement: Cargo refuses cyclic dependencies, so workspace layouts mechanically prevent layer violations. Single-crate layouts rely on convention + reviewer discipline (or the arch-scanner when shipped).

## Structural Patterns

### Newtype

Wrap primitives that carry semantic meaning:

```rust
pub struct UserId(uuid::Uuid);

impl UserId {
    pub fn new() -> Self { Self(uuid::Uuid::new_v4()) }
    pub fn as_uuid(&self) -> &uuid::Uuid { &self.0 }
}
```

Apply when the primitive's identity matters beyond its value: IDs, codes, prefixes, classifiers. Reject when the value is purely numeric (counter, port number, line count).

### Type-State

Encode state machines in the type system. Compile-time prevention of misuse:

```rust
pub struct Request<S> { ... , _state: PhantomData<S> }
pub struct Unvalidated;
pub struct Validated;

impl Request<Unvalidated> {
    pub fn validate(self) -> Result<Request<Validated>, ValidationError> { ... }
}

impl Request<Validated> {
    pub fn execute(self) -> Response { ... }  // only callable after validate
}
```

Apply when a sequence of operations must happen in a specific order. Defer when runtime branching captures the same constraint clearly.

### Builder

Idiomatic for constructing types with many optional fields:

```rust
#[derive(Default)]
pub struct ClientBuilder {
    timeout: Option<Duration>,
    retries: Option<u32>,
}

impl ClientBuilder {
    pub fn timeout(mut self, d: Duration) -> Self { self.timeout = Some(d); self }
    pub fn retries(mut self, n: u32) -> Self { self.retries = Some(n); self }
    pub fn build(self) -> Client { Client { ... } }
}
```

Annotate the builder with `#[must_use = "Builder does nothing until .build()"]`.

### Trait-Based Dependency Injection

Define ports in the application layer; inject concrete impls at the composition root.

```rust
// application/src/ports.rs
pub trait UserRepository: Send + Sync {
    async fn find(&self, id: &UserId) -> Result<Option<User>, RepoError>;
    async fn save(&self, user: &User) -> Result<(), RepoError>;
}

// application/src/use_cases/get_user.rs
pub struct GetUserUseCase<R: UserRepository> {
    repo: Arc<R>,
}

impl<R: UserRepository> GetUserUseCase<R> {
    pub fn new(repo: Arc<R>) -> Self { Self { repo } }
    pub async fn execute(&self, id: UserId) -> Result<User, GetUserError> {
        self.repo.find(&id).await?.ok_or(GetUserError::NotFound)
    }
}
```

### Generics vs `Box<dyn Trait>` Trade-off

| Use generics | Use `Box<dyn Trait>` |
|---|---|
| Hot paths where monomorphization helps | Heterogeneous collections (`Vec<Box<dyn Handler>>`) |
| Library APIs (caller picks the concrete type) | Plugin systems with runtime selection |
| Compile-time guarantees about the impl | Closed extension (sealed trait + impls) |
| When binary size is acceptable | When binary size matters more than dispatch cost |

Default to generics. Switch to `Box<dyn Trait>` when monomorphization explodes the binary or when truly heterogeneous storage is required.

### Sealed Traits

Restrict trait implementation to within your crate:

```rust
mod private {
    pub trait Sealed {}
}

pub trait MyTrait: private::Sealed {
    fn method(&self) -> String;
}

impl private::Sealed for ConcreteType {}
impl MyTrait for ConcreteType { ... }
```

Apply when the trait is your API but you want to control all implementations (e.g., to add methods without breaking changes).

## Async + Concurrency Patterns

- **Send + Sync bounds**: trait objects crossing tasks need `Send + Sync` bounds. Define traits as `pub trait X: Send + Sync` when they will be passed across `.await` points.
- **Arc vs Rc**: use `Arc<T>` for shared ownership across tasks; `Rc<T>` only for single-threaded contexts.
- **Mutex selection**: `tokio::sync::Mutex` only when holding the lock across `.await`; otherwise `std::sync::Mutex` (faster, no async overhead).
- **Cancellation**: every async fn should be cancellation-safe — document at top of file when not.
- **Backpressure**: `tokio::sync::mpsc::channel(buffer)` instead of `unbounded_channel()` unless the unbounded case is justified.

## Error Categorization

Domain errors are not application errors are not infrastructure errors:

- **Domain errors**: business rule violations (`EmailAlreadyTaken`, `InsufficientBalance`). Surface to callers as-is.
- **Application errors**: use-case-specific composition (`GetUserError::NotFound`, `GetUserError::RepoFailure`). Wrap domain errors via `#[from]`.
- **Infrastructure errors**: I/O failures, network timeouts, serialization errors. Wrap as `Repo::Failure(source)` — never leak the concrete `sqlx::Error` upward.

The `thiserror` derive expresses these layers cleanly:

```rust
#[derive(Debug, thiserror::Error)]
pub enum GetUserError {
    #[error("user not found")]
    NotFound,
    #[error(transparent)]
    Repo(#[from] RepoError),
}
```

## Workspace Configuration

For workspace projects, root `Cargo.toml` enforces consistency:

```toml
[workspace.package]
edition = "2024"
rust-version = "1.83"
license = "MIT OR Apache-2.0"

[workspace.lints.clippy]
pedantic = "warn"
nursery = "warn"
cargo = "warn"

[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"
```

Member crates inherit via `package.lints.workspace = true`.

## Forbidden Patterns

- Domain crate depending on a database driver (`sqlx`, `diesel`, `mongodb`)
- Application crate depending on a specific web framework (`axum`, `actix-web`)
- Public function returning `Result<T, Box<dyn Error>>` from a library
- `unwrap()` / `expect("invariant")` outside `tests/`, `main.rs`, or `lib.rs::tests`
- Cyclic module dependencies (Cargo prevents inter-crate cycles; same discipline applies intra-crate)
- Re-exporting infrastructure types from the domain crate via `pub use`
