#!/usr/bin/env bash
# Runs the project checks locally. The GitHub CI workflow is manual-only, so
# this is what has to pass before a PR is merged.
#
#   scripts/ci.sh             run every job whose project exists
#   scripts/ci.sh --changed   run only the jobs for paths changed vs origin/main
#   scripts/ci.sh rust web    run the named jobs (proto, rust, web, android)
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

jobs=()
if [ "${1:-}" = "--changed" ]; then
  base=$(git merge-base origin/main HEAD)
  changed=$( { git diff --name-only "$base"; git ls-files --others --exclude-standard; } | sort -u)
  touches() { grep -qE "$1" <<<"$changed"; }
  touches '^(proto/|buf\.yaml$)' && jobs+=(proto)
  touches '^(crates/|apps/desktop/src-tauri/|Cargo\.(toml|lock)$|rust-toolchain\.toml$|proto/)' && jobs+=(rust)
  touches '^(apps/desktop/|package\.json$|pnpm-(lock|workspace)\.yaml$)' && jobs+=(web)
  touches '^(apps/android/|proto/)' && jobs+=(android)
elif [ $# -gt 0 ]; then
  jobs=("$@")
else
  jobs=(proto rust web android)
fi

proto() {
  if command -v buf >/dev/null; then
    buf lint && proto_breaking
  else
    echo "buf not installed; checking that the schema compiles with protoc instead"
    echo "wire compatibility with main is NOT checked; install buf before merging proto/ changes"
    protoc -I proto -o /dev/null $(find proto -name '*.proto')
  fi
}

# Checks the schema against the one this branch started from. The base is
# exported with git archive rather than read through buf's .git input, which
# fails in git worktrees.
proto_breaking() {
  local base tmp status
  base=$(git merge-base origin/main HEAD) || { echo "no merge base with origin/main"; return 1; }
  git cat-file -e "$base:proto" 2>/dev/null || { echo "no schema at the merge base; nothing to compare"; return 0; }
  tmp=$(mktemp -d)
  git archive "$base" proto buf.yaml | tar -x -C "$tmp"
  buf breaking --against "$tmp"
  status=$?
  rm -rf "$tmp"
  return $status
}

rust() {
  compgen -G 'crates/*/Cargo.toml' >/dev/null || compgen -G 'apps/*/src-tauri/Cargo.toml' >/dev/null || { echo "no Rust crates yet"; return 0; }
  local locked=(); [ -f Cargo.lock ] && locked=(--locked)
  cargo fmt --all --check &&
    cargo clippy --workspace --all-targets "${locked[@]}" -- -D warnings &&
    cargo test --workspace "${locked[@]}" &&
    { [ ! -f apps/desktop/src/lib/bindings.ts ] || git diff --exit-code -- apps/desktop/src/lib/bindings.ts; }
}

web() {
  [ -f apps/desktop/package.json ] && [ -f pnpm-lock.yaml ] || { echo "no desktop frontend yet"; return 0; }
  pnpm install --frozen-lockfile &&
    pnpm --filter desktop lint &&
    pnpm --filter desktop typecheck &&
    pnpm --filter desktop test
}

android() {
  [ -x apps/android/gradlew ] || { echo "no Android app yet"; return 0; }
  (cd apps/android && ./gradlew assembleDebug testDebugUnitTest lint)
}

[ ${#jobs[@]} -eq 0 ] && { echo "ci: nothing to check"; exit 0; }
failed=()
for job in "${jobs[@]}"; do
  case "$job" in proto|rust|web|android) ;; *) echo "ci: unknown job '$job'"; exit 2 ;; esac
  echo "==> $job"
  start=$SECONDS
  if "$job"; then echo "<== $job passed ($((SECONDS - start))s)"; else echo "<== $job FAILED"; failed+=("$job"); fi
done

if [ ${#failed[@]} -gt 0 ]; then echo "ci: failed: ${failed[*]}"; exit 1; fi
echo "ci: all passed (${jobs[*]})"
