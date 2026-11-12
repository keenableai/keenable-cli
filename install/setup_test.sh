#!/bin/sh
# Runs install/setup.sh against a stand-in `keenable` that logs its calls, so
# the order of steps and their arguments are pinned with no install or network.
#
#   sh install/setup_test.sh            # uses sh
#   SHELL_UNDER_TEST=dash sh install/setup_test.sh

# Checks are single-quoted on purpose: check() evals them after each run.
# shellcheck disable=SC2016,SC2034

set -u

SCRIPT="$(cd "$(dirname "$0")" && pwd)/setup.sh"
SH="${SHELL_UNDER_TEST:-sh}"
failures=0
tmp=$(mktemp -d "${TMPDIR:-/tmp}/keenable-setup-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

# A fresh HOME and bin dir with a stand-in CLI; $1 names a subcommand that fails.
setup_case() {
    rm -rf "$tmp/case"
    home="$tmp/case/home"
    bin="$tmp/case/bin"
    log="$tmp/case/calls.log"
    mkdir -p "$home" "$bin"
    cat >"$bin/keenable" <<EOF
#!/bin/sh
echo "\$*" >> "$log"
[ "\$1" = "${1:-}" ] && exit 1
exit 0
EOF
    chmod +x "$bin/keenable"
}

# --yes is added only when there is no terminal, so checks accept both.
# Runs setup.sh with stdin from /dev/null; output in $out, exit code in $status.
run() {
    out=$(env -i HOME="$home" PATH="$bin:/usr/bin:/bin" "$@" "$SH" "$SCRIPT" ${ARGS:-} 2>&1 </dev/null)
    status=$?
}

calls() { cat "$log" 2>/dev/null; }
line() { calls | sed -n "${1}p"; }

check() {
    if eval "$2"; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FAIL %s\n' "$1"
        printf '%s\n' "--- output:" "$out" "--- calls:" "$(calls)" "---"
        failures=$((failures + 1))
    fi
}

setup_case
ARGS='' run KEENABLE_API_KEY=keen_abc_secret
check "key: login, configure every agent, test search, in order" '
    [ "$status" -eq 0 ] &&
    [ "$(line 1)" = "login --api-key keen_abc_secret" ] &&
    line 2 | grep -Eq "^configure-mcp --all( --yes)?$" &&
    line 3 | grep -Eq "^search .+ -p --max-results 3$" &&
    [ "$(calls | wc -l | tr -d " ")" -eq 3 ] &&
    printf "%s" "$out" | grep -q "Keenable is set up."'
check "never prints the key" '! printf "%s" "$out" | grep -q keen_abc_secret'

setup_case
ARGS='' run
check "no key: signs in through the browser" '[ "$status" -eq 0 ] && [ "$(line 1)" = "login" ]'

setup_case
mkdir -p "$home/.keenable"
printf '{"api_key":"keen_saved"}' >"$home/.keenable/config.json"
ARGS='' run
check "stored key: does not sign in again" '
    [ "$status" -eq 0 ] && line 1 | grep -q "^configure-mcp --all" &&
    printf "%s" "$out" | grep -q "Already signed in."'

setup_case
ARGS='--claude-code' run KEENABLE_API_KEY=keen_abc_secret
check "arguments go to configure-mcp" 'line 2 | grep -Eq "^configure-mcp --claude-code( --yes)?$"'

setup_case login
ARGS='' run KEENABLE_API_KEY=keen_bad
check "refused key: stops before changing any agent" '
    [ "$status" -eq 1 ] && [ "$(calls)" = "login --api-key keen_bad" ] &&
    printf "%s" "$out" | grep -q "the API key was not accepted" &&
    printf "%s" "$out" | grep -q "https://docs.keenable.ai/cli"'

setup_case
rm "$bin/keenable"
printf '#!/bin/sh\nexit 22\n' >"$bin/curl"
chmod +x "$bin/curl"
ARGS='' run
check "failed installer download: stops" '
    [ "$status" -eq 1 ] && [ -z "$(calls)" ] &&
    printf "%s" "$out" | grep -q "the CLI installer could not be downloaded"'

setup_case search
ARGS='' run KEENABLE_API_KEY=keen_abc_secret
check "only the test search fails: says Keenable is installed" '
    [ "$status" -eq 1 ] &&
    printf "%s" "$out" | grep -q "the test search failed. Keenable is installed."'

# Piped like `curl ... | sh -s --`: the script arrives on stdin.
setup_case
out=$(env -i HOME="$home" PATH="$bin:/usr/bin:/bin" KEENABLE_API_KEY=keen_abc_secret \
    "$SH" -s -- --claude-code <"$SCRIPT" 2>&1)
status=$?
check "piped into sh -s: same steps" '
    [ "$status" -eq 0 ] && [ "$(line 1)" = "login --api-key keen_abc_secret" ] &&
    line 2 | grep -Eq "^configure-mcp --claude-code( --yes)?$"'

[ "$failures" -eq 0 ] || {
    echo "$failures setup.sh test(s) failed"
    exit 1
}
echo "all setup.sh tests passed"
