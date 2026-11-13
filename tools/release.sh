#!/usr/bin/env bash
# One-command release: tools/release.sh [patch|minor|major|X.Y.Z] [--yes] [--dry-run]
#
# Runs your own `gh` login, so the release PR is authored by you and the Qodo
# bridge approves it like any other PR (see .github/workflows/cut-release.yml).
#   1. dispatches "Cut release" on main (bumps Cargo.toml/Cargo.lock on a
#      release/vX.Y.Z branch and runs CI on it)
#   2. opens the PR as you, waits for checks and approval, squash-merges it
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

log "dispatching Cut release"
start=$(date -u +%FT%TZ)
gh workflow run cut-release.yml --repo "$repo" --ref main -f bump="$next"
run=""
for _ in $(seq 1 30); do
  run=$(gh run list --repo "$repo" --workflow cut-release.yml --event workflow_dispatch --limit 5 \
    --json databaseId,createdAt --jq "[.[] | select(.createdAt >= \"$start\")][0].databaseId // empty")
  [ -n "$run" ] && break
  sleep 2
done
[ -n "$run" ] || die "could not find the dispatched run; check the Actions tab"
gh run watch "$run" --repo "$repo" --exit-status >/dev/null || die "Cut release failed: https://github.com/$repo/actions/runs/$run"

pr=$(gh pr list --repo "$repo" --head "$branch" --state open --json number --jq '.[0].number // empty')
if [ -z "$pr" ]; then
  log "opening the PR"
  changes=$(gh api "repos/$repo/compare/v$cur...main" --jq '.commits[].commit.message | split("\n")[0] | "- " + .' 2>/dev/null || true)
  pr=$(gh pr create --repo "$repo" --base main --head "$branch" --title "chore: release $next" \
    --body "Bumps the version to $next. Merging tags \`v$next\`, which starts the dist release workflow (binaries, installers, GitHub Release, Homebrew formula).

Changes since v$cur:
$changes" | sed 's#.*/##')
fi
log "PR #$pr: waiting for checks and approval"
for _ in $(seq 1 90); do
  failed=$(gh pr checks "$pr" --repo "$repo" --json bucket --jq '[.[] | select(.bucket == "fail")] | length' 2>/dev/null || echo 0)
  [ "$failed" = 0 ] || die "a check failed: https://github.com/$repo/pull/$pr/checks"
  state=$(gh pr view "$pr" --repo "$repo" --json mergeStateStatus --jq .mergeStateStatus)
  case "$state" in
    CLEAN) break ;;
    DIRTY | BEHIND) die "PR #$pr is $state; resolve it, then re-run" ;;
  esac
  sleep 20
done
[ "$state" = CLEAN ] || die "PR #$pr is not mergeable yet ($state): https://github.com/$repo/pull/$pr"

log "merging PR #$pr"
gh pr merge "$pr" --repo "$repo" --squash --delete-branch

log "waiting for tag v$next and the dist release"
for _ in $(seq 1 360); do
  if gh release view "v$next" --repo "$repo" >/dev/null 2>&1; then
    log "released: https://github.com/$repo/releases/tag/v$next"
    exit 0
  fi
  bad=$(gh run list --repo "$repo" --workflow release.yml --branch "v$next" --json conclusion,status \
    --jq '[.[] | select(.conclusion == "failure")] | length' 2>/dev/null || echo 0)
  [ "$bad" = 0 ] || die "release.yml failed for v$next: https://github.com/$repo/actions/workflows/release.yml"
  sleep 10
done
die "v$next is not released after 60 min: https://github.com/$repo/actions"
