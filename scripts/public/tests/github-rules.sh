#!/usr/bin/env bash
# Rules that keep the public CI safe for pull requests from forks:
# - no workflow uses pull_request_target or workflow_run;
# - every workflow declares top-level `permissions`, and `write` only appears
#   in the `draft` job of release.yml;
# - the only secret is the GITHUB_TOKEN of release.yml (`github.token`); no
#   `secrets.` anywhere;
# - every third-party action is pinned to a full commit SHA with a version
#   comment; container images are pinned by digest;
# - ci.yml cancels older runs of the same pull request.
# Usage: github-rules.sh [GITHUB_DIR]  (default: scripts/public/github in the
# development tree, .github in the exported tree). Plain bash, grep and awk.
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/../../.." && pwd)

check() { # check GITHUB_DIR → exit 1 listing every violation
  local dir=$1 failures=0 file name code
  fail() { echo "github-rules: $*" >&2; failures=$((failures + 1)); }
  local workflows=("$dir"/workflows/*.yml "$dir"/workflows/*.yaml)
  local found=0
  for file in "${workflows[@]}"; do
    [[ -f $file ]] || continue
    found=1
    name=$(basename -- "$file")
    # Comment lines may explain the rules; only the YAML itself counts.
    code=$(grep -v '^[[:space:]]*#' "$file")
    grep -q 'pull_request_target' <<< "$code" && fail "$name: uses pull_request_target"
    grep -q 'workflow_run' <<< "$code" && fail "$name: uses workflow_run"
    grep -q '^permissions:' <<< "$code" || fail "$name: no top-level permissions"
    grep -Eq '^permissions:[[:space:]]*(write-all|read-all)' <<< "$code" && fail "$name: blanket permissions"
    grep -q 'secrets\.' <<< "$code" && fail "$name: uses secrets.*"
    if [[ $name != release.yml ]] && grep -q 'github\.token' <<< "$code"; then fail "$name: uses github.token"; fi
    # `write` scopes (and `write-all`) only inside jobs.draft of release.yml.
    awk -v name="$name" '
      /^jobs:/ { in_jobs = 1; next }
      /^[^[:space:]#]/ { in_jobs = 0; job = "" }
      in_jobs && /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { job = $1; sub(":", "", job) }
      /^[[:space:]]*[a-z-]+:[[:space:]]*write(-all)?([[:space:]]|$)/ {
        if (!(name == "release.yml" && job == "draft")) { print "github-rules: " name ": write permission outside release.yml draft job (line " NR ")"; bad = 1 }
      }
      END { exit bad }
    ' "$file" >&2 || failures=$((failures + 1))
    # Actions pinned by commit SHA with a version comment.
    while IFS= read -r line; do
      [[ $line =~ uses:[[:space:]]*([^[:space:]]+) ]] || continue
      local ref=${BASH_REMATCH[1]}
      [[ $ref == ./* ]] && continue
      [[ $ref =~ @[0-9a-f]{40}$ ]] || fail "$name: action not pinned by SHA: $ref"
      [[ $line =~ \#[[:space:]]*[^[:space:]] ]] || fail "$name: pinned action without version comment: $ref"
    done < <(grep -E '^[[:space:]]*-?[[:space:]]*uses:' "$file")
    while IFS= read -r line; do
      [[ $line =~ @sha256:[0-9a-f]{64} ]] || fail "$name: container image not pinned by digest: $line"
    done < <(grep -E '^[[:space:]]*(container|image):[[:space:]]*[^[:space:]]' "$file")
  done
  [[ $found == 1 ]] || fail "$dir: no workflows"
  if [[ -f $dir/workflows/ci.yml ]]; then
    grep -q '^concurrency:' "$dir/workflows/ci.yml" || fail "ci.yml: no concurrency"
    grep -q 'cancel-in-progress:' "$dir/workflows/ci.yml" || fail "ci.yml: concurrency does not cancel older runs"
    grep -Eq '^  pull_request:' "$dir/workflows/ci.yml" || fail "ci.yml: does not run on pull_request"
  else
    fail "$dir: no workflows/ci.yml"
  fi
  if [[ -f $dir/workflows/release.yml ]]; then
    grep -q -- '--draft' "$dir/workflows/release.yml" || fail "release.yml: release is not created as a draft"
  fi
  [[ -f $dir/CODEOWNERS ]] || fail "$dir: no CODEOWNERS"
  [[ -f $dir/dependabot.yml ]] || fail "$dir: no dependabot.yml"
  return $((failures > 0))
}

if [[ $# -gt 0 ]]; then
  target=$1
elif [[ -d $root/scripts/public/github ]]; then
  target=$root/scripts/public/github
else
  target=$root/.github
fi
check "$target"

# The checker itself must reject each violation (synthetic copies).
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
mutate() { # mutate LABEL FILE SED_EXPR → the mutated copy must fail
  local label=$1 file=$2 expr=$3 copy=$scratch/$1
  rm -rf -- "$copy"
  cp -r -- "$target" "$copy"
  sed -i -e "$expr" "$copy/workflows/$file"
  if cmp -s "$target/workflows/$file" "$copy/workflows/$file"; then
    echo "github-rules: mutation '$label' changed nothing" >&2; exit 1
  fi
  if check "$copy" 2>/dev/null; then
    echo "github-rules: mutation '$label' was accepted" >&2; exit 1
  fi
}
mutate pull-request-target ci.yml 's/^  pull_request:/  pull_request_target:/'
mutate no-permissions ci.yml '/^permissions:/,/^$/d'
# shellcheck disable=SC2016 # literal ${{ }} written into the copy
mutate secret ci.yml 's/^env:/env:\n  TOKEN: ${{ secrets.EXAMPLE }}/'
# shellcheck disable=SC2016
mutate token-in-ci ci.yml 's/^env:/env:\n  GH_TOKEN: ${{ github.token }}/'
mutate job-write ci.yml 's/^  contents: read/  contents: write/'
mutate job-write-all ci.yml 's/^    runs-on: ubuntu-latest$/    runs-on: ubuntu-latest\n    permissions: write-all/'
mutate workflow-run ci.yml 's/^  pull_request:$/  pull_request:\n  workflow_run:\n    workflows: [CI]\n    types: [completed]/'
mutate unpinned ci.yml '0,/@[0-9a-f]\{40\} # v[0-9.]*/s//@v7/'
mutate no-concurrency ci.yml '/^concurrency:/,/^$/d'
mutate build-write release.yml 's/^    runs-on: ubuntu-latest$/    runs-on: ubuntu-latest\n    permissions:\n      contents: write/'
mutate unpinned-container release.yml 's/@sha256:[0-9a-f]*//'
mutate no-draft release.yml 's/ --draft//'
echo "github-rules: ok (${target#"$root"/})"
