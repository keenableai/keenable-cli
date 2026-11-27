# Contributing

Thanks for helping improve the Keenable CLI. Issues and pull requests from
anyone are welcome: bug reports, docs fixes, new flags, new MCP clients.

For a security problem, don't open an issue: see [SECURITY.md](SECURITY.md).

## Ways to help

- **Report a bug or ask for a feature:**
  [open an issue](https://github.com/keenableai/keenable-cli/issues/new/choose).
  The bug form asks for `keenable --version`, the command and its output.
- **Pick up an issue:** issues labelled
  [good first issue](https://github.com/keenableai/keenable-cli/issues?q=is%3Aopen+label%3A%22good+first+issue%22)
  are small and well defined;
  [help wanted](https://github.com/keenableai/keenable-cli/issues?q=is%3Aopen+label%3A%22help+wanted%22)
  are ones we'd like help with. Comment on the issue to say you're on it.
- **Something bigger** (a new command, a change to output or exit codes):
  open an issue first so we can agree on the shape before you write code.

## Set up

You need a stable Rust toolchain (`rustup`). For the end-to-end tests, you
also need [uv](https://docs.astral.sh/uv/).

```bash
git clone https://github.com/<you>/keenable-cli && cd keenable-cli
cargo build
./target/debug/keenable search "hello" -p   # works without an API key
```

To keep your own `~/.keenable` untouched while you test, point the CLI at a
scratch directory: `export KEENABLE_HOME=$(mktemp -d)`.

## Before you open a pull request

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test                                   # no API key or network needed
```

CI runs these on every pull request. The end-to-end suite calls the live API
and needs a key, so a maintainer runs it for pull requests from forks. To run
it yourself with your own key (free at https://keenable.ai/console):

```bash
export KEENABLE_API_KEY=keen_...
export KEENABLE_BIN=./target/debug/keenable
uv run --project tests/e2e pytest tests/e2e -v -m "not latency and not semantic and not install"
```

## Pull requests

- One change per pull request. Say what changed and why; the template lists
  the rest.
- A user-facing change (a command, flag, output field, error message or exit
  code) also updates its `--help` text, the README and `CHANGELOG.md` (under
  "Unreleased"). Maintainers handle the matching change on
  [docs.keenable.ai](https://docs.keenable.ai) if you don't.
- Don't remove or rename a command, flag or output field without saying so in
  the description: scripts and agents depend on them, so maintainers decide.
- Add a test for a bug fix or new behavior: a unit test next to the code, or a
  case in `tests/e2e/` for behavior against the API.

[CLAUDE.md](CLAUDE.md) has the full conventions: output format, error
handling, the terminal UI helpers, the daemon and the network rules. It's
written for coding agents, so it also works as context if you use one.
Releases are described in [RELEASE.md](RELEASE.md).

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
