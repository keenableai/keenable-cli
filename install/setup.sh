#!/bin/sh
# Keenable setup: one command that installs the Keenable CLI, signs you in, adds
# Keenable to your coding agents over MCP and runs a test search.
#
#   curl -fsSL https://github.com/keenableai/keenable-cli/releases/latest/download/setup.sh | sh
#
# With an API key from the console, nothing opens in the browser:
#
#   curl -fsSL https://github.com/keenableai/keenable-cli/releases/latest/download/setup.sh | KEENABLE_API_KEY=keen_... sh
#
# Arguments go to `keenable configure-mcp`, so one agent can be named:
#
#   curl -fsSL https://github.com/keenableai/keenable-cli/releases/latest/download/setup.sh | sh -s -- --claude-code
#
# Docs: https://docs.keenable.ai/cli

set -eu

INSTALLER_URL="https://github.com/keenableai/keenable-cli/releases/latest/download/keenable-cli-installer.sh"
DOCS_URL="https://docs.keenable.ai/cli"
TEST_QUERY="latest Next.js release notes"

say() {
    printf '%s\n' "$*" >&2
}

step() {
    printf '\n==> %s\n' "$*" >&2
}

fail() {
    say ""
    say "Keenable setup stopped: $*"
    say "Set it up by hand: $DOCS_URL"
    exit 1
}

# Where the CLI is, whether it was already installed or the installer just put it there.
# The installer adds its directory to PATH for new shells only, so this one looks there itself.
find_cli() {
    if command -v keenable >/dev/null 2>&1; then
        command -v keenable
        return 0
    fi
    for dir in "${KEENABLE_CLI_INSTALL_DIR:-}" "${KEENABLE_CLI_INSTALL_DIR:-}/bin" \
        "${CARGO_HOME:-$HOME/.cargo}/bin" "$HOME/.local/bin"; do
        if [ -n "$dir" ] && [ -x "$dir/keenable" ]; then
            printf '%s\n' "$dir/keenable"
            return 0
        fi
    done
    return 1
}

# Downloaded to a file and checked before it runs: piped straight into sh, a failed download
# is an empty script that exits 0, and the failure would be reported as a missing CLI.
install_cli() {
    command -v curl >/dev/null 2>&1 || fail "curl is not installed."
    installer=$(mktemp "${TMPDIR:-/tmp}/keenable-cli-installer.XXXXXX") ||
        fail "could not create a temporary file."
    if ! curl --proto '=https' --tlsv1.2 -LsSf "$INSTALLER_URL" -o "$installer" || [ ! -s "$installer" ]; then
        rm -f "$installer"
        fail "the CLI installer could not be downloaded."
    fi
    if ! sh "$installer" >&2; then
        rm -f "$installer"
        fail "the CLI installer failed."
    fi
    rm -f "$installer"
}

main() {
    case "$(uname -s 2>/dev/null || echo unknown)" in
    Darwin | Linux) ;;
    *) fail "this script runs on macOS and Linux. On Windows, use the PowerShell installer." ;;
    esac

    step "Installing the Keenable CLI"
    if cli=$(find_cli); then
        say "Already installed: $cli"
    else
        install_cli
        cli=$(find_cli) || fail "the CLI was installed but could not be found."
    fi

    step "Signing in"
    if [ -n "${KEENABLE_API_KEY:-}" ]; then
        "$cli" login --api-key "$KEENABLE_API_KEY" || fail "the API key was not accepted."
    elif grep -q '"api_key"' "$HOME/.keenable/config.json" 2>/dev/null; then
        say "Already signed in."
    else
        say "A browser window opens. Create an account or sign in, then approve the code."
        "$cli" login || fail "sign-in did not finish."
    fi

    step "Adding Keenable to your coding agents"
    if [ "$#" -eq 0 ]; then
        set -- --all
    fi
    # The CLI lists what it will change and asks first. Piped from curl, stdin is the script
    # itself, so the question is asked on the terminal; with no terminal (CI, an agent) it
    # runs with --yes.
    if (: </dev/tty) 2>/dev/null; then
        "$cli" configure-mcp "$@" </dev/tty || fail "your coding agents were not changed."
    else
        "$cli" configure-mcp "$@" --yes || fail "your coding agents were not changed."
    fi

    step "Running a test search"
    "$cli" search "$TEST_QUERY" -p --max-results 3 ||
        fail "the test search failed. Keenable is installed. Run: keenable search \"$TEST_QUERY\" -p"

    say ""
    say "Keenable is set up. Restart your coding agent, then ask it to search the web."
    if ! command -v keenable >/dev/null 2>&1; then
        say "Open a new terminal to use the keenable command."
    fi
}

main "$@"
