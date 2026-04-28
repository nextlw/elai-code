# Contributing

Thanks for contributing to Elai Code.

## Development setup

- Install the stable Rust toolchain.
- Work from the repository root in this Rust workspace. If you started from the parent repo root, `cd rust/` first.

## Build

```bash
cargo build
cargo build --release
```

## Test and verify

Run the full Rust verification set before you open a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
cargo test --workspace
```

If you change behavior, add or update the relevant tests in the same pull request.

## Code style

- Follow the existing patterns in the touched crate instead of introducing a new style.
- Format code with `rustfmt`.
- Keep `clippy` clean for the workspace targets you changed.
- Prefer focused diffs over drive-by refactors.

## Project conventions

- **TUI-safe progress**: long-running operations MUST be wrapped in
  `runtime::with_task_default` (or `with_task`) and never use `eprintln!`,
  `println!`, or `indicatif` directly. See `docs/progress-pattern.md` for the
  full rationale, sinks, and code samples. New code paths that emit progress
  through any other mechanism will be rejected during review.
- **Slash commands**: register new commands in
  `crates/commands/src/lib.rs::SLASH_COMMAND_SPECS` (with a `SlashCategory`)
  and add the matching parser arm to `SlashCommand`. Tests in `commands` count
  the registered specs — bump the expected count when you add one.
- **No new top-level docs without need**: prefer extending `README.md` or the
  files under `docs/` over creating new top-level markdown files.

## Pull requests

- Branch from `main`.
- Keep each pull request scoped to one clear change.
- Explain the motivation, the implementation summary, and the verification you ran.
- Make sure local checks pass before requesting review.
- If review feedback changes behavior, rerun the relevant verification commands.
