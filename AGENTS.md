# AGENTS.md — mlb-standings-bot

## Project Overview

Rust Discord bot that fetches MLB standings from the balldontlie MLB API and
posts them to a Discord channel via slash commands and a cron scheduler. Built
with poise/serenity for Discord, reqwest for HTTP, tokio for async, and anyhow
for error handling.

## Build / Run / Test Commands

```bash
# Build (debug)
cargo build

# Build (release)
cargo build --release

# Run (requires .env with DISCORD_TOKEN, BALLDONTLIE_API_KEY, etc.)
cargo run

# Format all code (uses default rustfmt settings — no config file)
cargo fmt

# Check formatting without modifying files
cargo fmt -- --check

# Lint with clippy
cargo clippy -- -D warnings

# Run all tests
cargo test

# Run a single test by name (substring match)
cargo test test_name

# Run tests in a specific module
cargo test module_name::

# Run tests with output printed
cargo test -- --nocapture
```

## Project Structure

```
src/
├── main.rs              # Entry point: config, client, cache, framework, scheduler setup
├── config.rs            # Config struct loaded from environment variables
├── api/
│   ├── mod.rs           # Re-exports: client, models
│   ├── client.rs        # HTTP client for balldontlie MLB API (rate-limited, paginated)
│   └── models.rs        # Serde models: ApiResponse<T>, Meta, Team, TeamGameData, Game
├── bot/
│   ├── mod.rs           # Re-exports: commands, scheduler
│   ├── commands.rs      # Poise slash command: /standings [season]
│   └── scheduler.rs     # Cron-based daily standings poster
└── standings/
    ├── mod.rs           # Re-exports: cache, compute, format
    ├── cache.rs         # Thread-safe async cache with TTL + incremental refresh
    ├── compute.rs       # Pure logic: tally wins/losses, split by division, GB calc, wild card
    └── format.rs        # Discord embed builder (6 division + 2 wild card tables)
```

## Code Style Guidelines

### Formatting

- **rustfmt defaults** — no `.rustfmt.toml` exists. Use `cargo fmt` before committing.
- 4-space indentation, standard brace placement, trailing commas in multiline expressions.

### Imports

Organize imports in this order, separated by blank lines:

1. Standard library (`std::`)
2. External crates (`anyhow::`, `tokio::`, `reqwest::`, etc.)
3. Internal crate modules (`crate::api::`, `crate::standings::`, etc.)

```rust
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use crate::api::client::BallDontLieClient;
use crate::standings::cache::StandingsCache;
```

### Naming Conventions

- **snake_case** for functions, variables, modules, file names.
- **PascalCase** for types and structs (e.g., `BallDontLieClient`, `TeamRecord`).
- **SCREAMING_SNAKE_CASE** for constants (e.g., `BASE_URL`, `CACHE_TTL`, `PER_PAGE`).
- Module file names match module names (`client.rs` for the `client` module).
- Module `mod.rs` files are minimal — only `pub mod` re-exports.

### Types

- Strongly typed throughout; no raw JSON manipulation.
- API responses deserialized into structs via `#[derive(Deserialize)]`.
- `Arc<T>` for shared ownership across async tasks.
- `tokio::sync::RwLock` for interior mutability of shared state.
- `Option<T>` for nullable/optional fields, never sentinel values.
- Generic `ApiResponse<T>` for paginated API response shapes.
- Use `#[allow(dead_code)]` on serde model fields that exist for deserialization
  completeness but aren't read in application code.

### Error Handling

- Return `anyhow::Result<T>` from all fallible functions.
- Use `.context("message")` or `.with_context(|| format!(...))` to add context.
- Use `anyhow::bail!` for early-return error conditions.
- Use the `?` operator throughout — avoid `.unwrap()` on fallible operations.
- `.unwrap()` is acceptable only on infallible `Option` values (e.g.,
  `NaiveDate::from_ymd_opt` with hardcoded valid dates).
- The poise error type is `Box<dyn std::error::Error + Send + Sync>`.
- Log errors with `{e:#}` (alternate format) for full error chain display.

### Async Patterns

- `#[tokio::main]` on `main()`.
- Clone `Arc` values before moving into `tokio::spawn` or closures.
- `futures::future::join_all` for concurrent batched operations.
- `tokio::time::sleep` for rate-limit delays.
- `Box::pin(async move { ... })` for async closures in poise/serenity callbacks.

### Logging

- Use the `tracing` crate (not `log`).
- `info!` for operational events, `debug!` for verbose detail, `warn!` for
  retries and degraded conditions, `error!` for failures.
- Use inline variable capture: `info!("Fetched {count} pages")` not
  `info!("Fetched {} pages", count)`.
- Default log level is `info`, configurable via `RUST_LOG` env var.

### Documentation

- `///` doc comments on all public structs, functions, and significant constants.
- Doc comments should explain *what* and *why*, not just restate the name.
- Inline `//` comments for non-obvious logic (rate limiting, cache TTL, ties).

## Environment Variables

Required (see `.env.example`):

| Variable              | Description                                     |
|-----------------------|-------------------------------------------------|
| `DISCORD_TOKEN`       | Discord bot token                               |
| `BALLDONTLIE_API_KEY` | API key for balldontlie.io                      |
| `CHANNEL_ID`          | Discord channel ID for scheduled posts          |
| `CRON_SCHEDULE`       | Cron expression for daily posting schedule      |
| `MLB_SEASON`          | MLB season year (optional, auto-detected)       |
| `RUST_LOG`            | Log level filter (optional, default: `info`)    |

## Key Architecture Notes

- **Rate limiting:** The free balldontlie API tier allows 5 requests/minute.
  The client batches requests in groups of 4 (leaving 1 slot for teams) with
  a 61-second delay between batches.
- **Caching:** In-memory only (no persistence). 1-hour TTL with support for
  incremental refresh (fetches only games since the last known date).
- **Season detection:** `current_mlb_season()` returns current year if month
  >= March, otherwise previous year (MLB seasons run March–October in one year).
- **Discord intents:** No privileged gateway intents required.
- **Scheduler lifecycle:** The cron scheduler is intentionally leaked via
  `std::mem::forget` so it lives for the program's lifetime.
- **Standings structure:** 6 divisions (AL/NL x East/Central/West), 2 wild card
  races (3 spots each), games behind calculated for both division and WC standings.
- **Game status:** Completed MLB games have status `"STATUS_FINAL"` (not `"Final"`).
- **Score extraction:** Runs are nested in `home_team_data.runs` / `away_team_data.runs`.
- **Embeds:** 8 embeds per message (6 divisions + 2 wild card). Discord max is 10.
