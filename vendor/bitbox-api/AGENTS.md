# Repository Guidelines

This repo contains both a Rust library and a NPM library.

## Versioning
- The Rust client version is in Cargo.toml. The changelog is in CHANGELOG-rust.md. The readme is in README-rust.md
- The NPM client version is in NPM_VERSION. The changelog is in CHANGELOG-npm.md. The readme is in README-npm.md.

## Project Structure & Module Organization
- `src/`: core Rust library (transport, protocol, coin-specific modules).
- `tests/`: integration tests; prefer adding new end-to-end flows here.
- `examples/`: runnable usage samples (`cargo run --example …`) for common flows.
- `messages/`: protobuf definitions; regenerated via `make build-protos`.
- `sandbox/`: browser demo for the WASM/JS API, used with WebHID.
- `scripts/`: build utilities (for example, protocol buffer generation).

## Build, Test, and Development Commands
- `cargo build` – build the Rust crate.
- `cargo test` – run unit and integration tests.
- `cargo fmt` – format Rust code using the repo’s `rustfmt` settings.
- `make build-protos` – regenerate Rust code from `messages/*.proto`.
- `make wasm` – build the WASM/JS package (`pkg/`) via `wasm-pack`.
- `make build-sandbox` - build the WebHID sandbox in `sandbox/`.
- `make run-sandbox` – start the WebHID sandbox in `sandbox/` for manual testing.

## Coding Style & Naming Conventions
- Rust edition 2021; rely on `cargo fmt` for formatting (4-space indents, standard rustfmt defaults).
- Modules and files: `snake_case`; types and traits: `PascalCase`; functions and methods: `snake_case`.
- Prefer explicit, typed errors using `thiserror`; avoid `unwrap`/`expect` in library code.

## Testing Guidelines
- Place integration tests in `tests/` and unit tests next to the code in `src/`.
- Name tests after the behavior under test (for example, `sign_psbt_roundtrip`).
- Run `cargo test` before opening a PR; for deeper changes, consider `cargo tarpaulin --features=simulator,tokio --out Html`.
- Check README-rust.md for how to run simulator tests.
- Check ./ci/ci.sh for how to run tests
- `cargo test` will output a lot of warnings, that is normal without the right features selected (see ci.sh). Don't attempt to fix them if not prompted to do so.

## Commit & Pull Request Guidelines
- Use short, focused commits; prefix with area when useful (for example, `btc: …`, `npm: …`, `tests: …`).
- Write commit subjects in imperative mood (for example, `add OP_RETURN example`).
- PRs should describe motivation, key changes, and any API or protocol impacts; link related issues and mention how it was tested (commands, examples, or sandbox runs).

