#!/usr/bin/env bash
# One-command release: tools/release.sh [patch|minor|major|X.Y.Z] [--yes] [--dry-run]
#
# Runs your own `gh` login, so the release PR is authored by you and the Qodo
# bridge approves it like any other PR (see .github/workflows/cut-release.yml).
#   1. dispatches "Cut release" on main (bumps Cargo.toml/Cargo.lock on a
#      release/vX.Y.Z branch and dispatches CI on it)
#   2. opens the PR as you, waits for that CI run and the approval, and
#      squash-merges the exact commit it checked
#   3. waits for the tag job and the dist release workflow, prints the release
# Nothing is built or pushed from this machine.
set -euo pipefail

repo=keenableai/keenable-cli
bump="patch"
yes=0
dry=0
for a in "$@"; do
  case "$a" in
    --yes | -y) yes=1 ;;
    --dry-run) dry=1 ;;
    -h | --help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) bump=$a ;;
  esac
done

log() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
command -v gh >/dev/null || die "gh is required (brew install gh)"
command -v jq >/dev/null || die "jq is required (brew install jq)"
gh auth status >/dev/null 2>&1 || die "run: gh auth login"

version_of() { sed -n 's/^version = "\(.*\)"$/\1/p' | head -1; }
cur=$(gh api -H 'Accept: application/vnd.github.raw' "repos/$repo/contents/Cargo.toml?ref=main" | version_of)
base=${cur%%-*}
IFS=. read -r maj min pat <<<"$base"
case "$bump" in
  patch) next="$maj.$min.$((pat + 1))" ;;
  minor) next="$maj.$((min + 1)).0" ;;
  major) next="$((maj + 1)).0.0" ;;
  *)
    [[ "$bump" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "bump must be patch, minor, major or X.Y.Z"
    next=$bump
    ;;
esac
branch="release/v$next"
log "keenable-cli $cur -> $next (branch $branch)"
[ "$dry" = 1 ] && exit 0

if [ "$yes" != 1 ]; then
  [ -t 0 ] || die "not a terminal; pass --yes to release without asking"
  read -r -p "Release v$next now? [y/N] " ans
  [[ "$ans" =~ ^[Yy]$ ]] || die "cancelled"
fi

# The newest run of <workflow> whose title is <title>, created since $start.
find_run() { # <workflow> <title> [extra gh run list args...]
  local wf=$1 title=$2
  shift 2
  gh run list --repo "$repo" --workflow "$wf" --limit 20 "$@" --json databaseId,displayTitle,createdAt \
    --jq "[.[] | select(.displayTitle == \"$title\" and .createdAt >= \"$start\")][0].databaseId // empty"
}

log "dispatching Cut release"
start=$(date -u +%FT%TZ)
gh workflow run cut-release.yml --repo "$repo" --ref main -f bump="$next"
run=""
for _ in $(seq 1 30); do
  # The run title carries the version (run-name), so a concurrent release of
  # another version can't be mistaken for ours.
  run=$(find_run cut-release.yml "Cut release $next" --event workflow_dispatch)
  [ -n "$run" ] && break
  sleep 2
done
[ -n "$run" ] || die "could not find the dispatched run; check the Actions tab"
gh run watch "$run" --repo "$repo" --exit-status >/dev/null || die "Cut release failed: https://github.com/$repo/actions/runs/$run"

# The workflow dispatches CI on the branch; the ruleset requires no status
# checks, so a CLEAN PR does not mean CI ran. Wait for that run ourselves.
log "waiting for CI on $branch"
ci=""
for _ in $(seq 1 30); do
  ci=$(gh run list --repo "$repo" --workflow ci.yml --branch "$branch" --event workflow_dispatch --limit 5 \
    --json databaseId,createdAt --jq "[.[] | select(.createdAt >= \"$start\")][0].databaseId // empty")
  [ -n "$ci" ] && break
  sleep 2
done
[ -n "$ci" ] || die "CI was not dispatched on $branch; check the Actions tab"
gh run watch "$ci" --repo "$repo" --exit-status >/dev/null || die "CI failed on $branch: https://github.com/$repo/actions/runs/$ci"
ci_sha=$(gh run view "$ci" --repo "$repo" --json headSha --jq .headSha)

me=$(gh api user --jq .login)
allowed='^(Cargo\.toml|Cargo\.lock)$'
pr=$(gh pr list --repo "$repo" --head "$branch" --state open --json number --jq '.[0].number // empty')
if [ -n "$pr" ]; then
  # Someone else's PR on our branch name is not ours to merge.
  author=$(gh pr view "$pr" --repo "$repo" --json author --jq .author.login)
  [ "$author" = "$me" ] || die "PR #$pr on $branch is authored by $author, not you; check it, close it, and re-run"
else
  log "opening the PR"
  changes=$(gh api "repos/$repo/compare/v$cur...main" --jq '.commits[].commit.message | split("\n")[0] | "- " + .' 2>/dev/null || true)
  pr=$(gh pr create --repo "$repo" --base main --head "$branch" --title "chore: release $next" \
    --body "Bumps the version to $next. Merging tags \`v$next\`, which starts the dist release workflow (binaries, installers, GitHub Release, Homebrew formula).

Changes since v$cur:
$changes" | sed 's#.*/##')
fi

log "PR #$pr: waiting for checks and approval"
state=""
pending=1
for _ in $(seq 1 90); do
  # Opening the PR starts its own CI run, which the ruleset doesn't require:
  # a failure there must stop us, and a run still going must be waited for.
  failed=$(gh pr checks "$pr" --repo "$repo" --json bucket --jq '[.[] | select(.bucket == "fail")] | length' 2>/dev/null || echo 0)
  [ "$failed" = 0 ] || die "a check failed: https://github.com/$repo/pull/$pr/checks"
  pending=$(gh pr checks "$pr" --repo "$repo" --json bucket --jq '[.[] | select(.bucket == "pending")] | length' 2>/dev/null || echo 0)
  state=$(gh pr view "$pr" --repo "$repo" --json mergeStateStatus --jq .mergeStateStatus)
  case "$state" in
    CLEAN) [ "$pending" = 0 ] && break ;;
    DIRTY | BEHIND) die "PR #$pr is $state; resolve it, then re-run" ;;
  esac
  sleep 20
done
{ [ "$state" = CLEAN ] && [ "$pending" = 0 ]; } || die "PR #$pr is not mergeable yet ($state, $pending checks pending): https://github.com/$repo/pull/$pr"

# Merge exactly what was checked: only the release files, at the head we saw,
# and that head must be the commit the CI run we watched tested.
head=$(gh pr view "$pr" --repo "$repo" --json headRefOid --jq .headRefOid)
[ "$ci_sha" = "$head" ] || die "CI ran on ${ci_sha:0:7} but PR #$pr head is ${head:0:7}; the branch changed after CI started, re-run"
extra=$(gh api "repos/$repo/pulls/$pr/files" --paginate --jq '.[].filename' | grep -v -E "$allowed" || true)
[ -z "$extra" ] || die "PR #$pr changes more than the release files ($extra); review it, then merge by hand"

log "merging PR #$pr"
gh pr merge "$pr" --repo "$repo" --squash --delete-branch --match-head-commit "$head"

log "waiting for the dist release of v$next"
seen=0
merged=$SECONDS # the 15 min "no run appeared" timeout counts from the merge, not script start
for _ in $(seq 1 360); do
  runs=$(gh run list --repo "$repo" --workflow release.yml --branch "v$next" --json status,conclusion 2>/dev/null || echo '[]')
  if [ "$(jq '[.[] | select(.conclusion == "success")] | length' <<<"$runs")" != 0 ]; then
    log "released: https://github.com/$repo/releases/tag/v$next"
    exit 0
  fi
  total=$(jq 'length' <<<"$runs")
  if [ "$total" != 0 ]; then
    seen=1
    # Every run finished and none succeeded (failed, cancelled, timed out).
    if [ "$(jq '[.[] | select(.status != "completed")] | length' <<<"$runs")" = 0 ]; then
      die "release.yml did not succeed for v$next ($(jq -r '[.[].conclusion] | join(", ")' <<<"$runs")): https://github.com/$repo/actions/workflows/release.yml"
    fi
  elif [ "$seen" = 0 ] && [ $((SECONDS - merged)) -gt 900 ]; then
    die "no release.yml run for v$next after 15 min; the tag job may have failed: https://github.com/$repo/actions/workflows/cut-release.yml"
  fi
  sleep 10
done
die "v$next is not released after 60 min: https://github.com/$repo/actions"
