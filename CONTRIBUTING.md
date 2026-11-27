# Contributing

Thanks for helping improve the Keenable CLI. Bug reports and feature requests
go in [GitHub issues](https://github.com/keenableai/keenable-cli/issues/new/choose).
For a security problem, see [SECURITY.md](SECURITY.md) instead.

## Build and test

You need a stable Rust toolchain and, for the end-to-end tests,
[uv](https://docs.astral.sh/uv/).

```bash
cargo build --release
cargo test                                         # unit and local integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The end-to-end suite runs the real binary against the live API:

```bash
export KEENABLE_API_KEY=keen_...                   # a key from https://keenable.ai/console
export KEENABLE_BIN=./target/release/keenable
uv run --project tests/e2e pytest tests/e2e -v -m "not latency and not semantic and not install"
```

## Pull requests

- Keep one change per PR, and say what changed and why.
- A user-facing change (a command, flag, output field, error message or exit
  code) updates `--help`, the README and `CHANGELOG.md` ("Unreleased") in the
  same PR. It also needs a docs PR in
  [keenable-external-docs](https://github.com/keenableai/keenable-external-docs).
- Never remove or rename a command, flag or output field without saying so in
  the PR description; maintainers decide on removals.

[CLAUDE.md](CLAUDE.md) has the full conventions: output format, error
handling, the terminal UI helpers, the daemon and the network rules.
Releases are described in [RELEASE.md](RELEASE.md).
