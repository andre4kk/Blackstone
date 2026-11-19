# Contributing to Blackstone

Thanks for helping. Blackstone is a Minecraft Bedrock server in Rust (NetherNet transport). Site: https://andre4kk.github.io/Blackstone/

## Build and run

- Server (stable Rust): `cargo run` from the repository root. It reads `server.toml` (created from `server.example.toml` if missing).
- World generator (`generator/`, separate Cargo workspace, nightly toolchain pinned by `generator/rust-toolchain.toml`, downloads Java 26.3 data on first build):
  `cd generator && cargo build --release`.
- See `README.md` for the architecture and configuration.

## Rules

- `cargo fmt --all` must leave no diff.
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- `cargo test --workspace` must pass (it uses a fake generator; the real one has its own tests: `cd generator && cargo test`).
- Add a test for new behaviour (see `crates/server/src/tests.rs` and `crates/server/tests/join.rs`).
- Code comments and log messages are in Italian (preferred); English is accepted in new code.
- Packet bytes: the reference is gophertunnel of the same game version, not the Mojang schemas. `crates/protocol/src/generated.rs` is generated: do not edit it by hand.
- Game behaviour (damage, drops, tables): the reference is Dragonfly (MIT). Copying behaviour and data from it is welcome.
- Never copy code from `generator/` (AGPL-3.0-or-later) into the MIT crates. The server only talks to the generator through the `vgen` protocol.

## Pull request checklist

- [ ] fmt, clippy and tests pass locally
- [ ] new behaviour has a test
- [ ] packet layouts checked against gophertunnel, behaviour against Dragonfly
- [ ] no code copied from the AGPL generator into MIT crates
- [ ] README or docs updated if user-visible behaviour changed
- [ ] one focused change per PR

Starter tasks: `docs/good-first-issues.md`.
