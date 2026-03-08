#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build evelin and sync the binary plus base eval config into another repo.

Usage:
  sync-to-agents.sh [--target <path>] [--profile debug|release] [--force] [--skip-build] [--dry-run]

Options:
  --target <path>    Destination repository root.
                    Default: /Users/jiri/agents
  --profile <name>   Cargo profile to sync from: debug or release.
                    Default: release
  --force            Replace existing destination files when contents differ.
  --skip-build       Reuse an existing compiled binary instead of running cargo build.
  --dry-run          Print planned actions without changing files.
  -h, --help         Show this help.
EOF
}

TARGET_ROOT="/Users/jiri/agents"
PROFILE="release"
FORCE="false"
SKIP_BUILD="false"
DRY_RUN="false"

while (($# > 0)); do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || {
        echo "Missing value for --target" >&2
        exit 1
      }
      TARGET_ROOT="$2"
      shift
      ;;
    --profile)
      [[ $# -ge 2 ]] || {
        echo "Missing value for --profile" >&2
        exit 1
      }
      PROFILE="$2"
      shift
      ;;
    --force)
      FORCE="true"
      ;;
    --skip-build)
      SKIP_BUILD="true"
      ;;
    --dry-run)
      DRY_RUN="true"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift
done

case "$PROFILE" in
  debug|release)
    ;;
  *)
    echo "Unsupported profile '$PROFILE'. Use debug or release." >&2
    exit 1
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
SOURCE_BIN="$REPO_ROOT/target/$PROFILE/evelin"
SOURCE_CONFIG="$REPO_ROOT/core/src/eval-config.toml"
DEST_BIN_DIR="$TARGET_ROOT/.evelin/bin"
DEST_BIN="$DEST_BIN_DIR/evelin"
DEST_CONFIG="$TARGET_ROOT/eval-config.toml"

print_cmd() {
  printf '%q' "$1"
  shift || true
  for arg in "$@"; do
    printf ' %q' "$arg"
  done
  printf '\n'
}

run_cmd() {
  if [[ "$DRY_RUN" == "true" ]]; then
    printf '[dry-run] '
    print_cmd "$@"
  else
    "$@"
  fi
}

ensure_dir() {
  if [[ -d "$1" ]]; then
    return
  fi
  run_cmd mkdir -p "$1"
}

sync_file() {
  local source_path="$1"
  local dest_path="$2"
  local label="$3"
  local mode="$4"

  if [[ -e "$dest_path" || -L "$dest_path" ]]; then
    if cmp -s "$source_path" "$dest_path"; then
      echo "SKIP   $label (already up to date)"
      return 0
    fi
    if [[ "$FORCE" != "true" ]]; then
      echo "SKIP   $label (destination exists: $dest_path, use --force to replace)"
      return 0
    fi
  fi

  ensure_dir "$(dirname "$dest_path")"
  run_cmd cp "$source_path" "$dest_path"
  run_cmd chmod "$mode" "$dest_path"
  echo "SYNC   $label -> $dest_path"
}

if [[ ! -d "$TARGET_ROOT" ]]; then
  echo "Destination repo does not exist: $TARGET_ROOT" >&2
  exit 1
fi

if [[ ! -f "$SOURCE_CONFIG" ]]; then
  echo "Missing source config template: $SOURCE_CONFIG" >&2
  exit 1
fi

if [[ "$SKIP_BUILD" != "true" ]]; then
  build_cmd=(cargo build --bin evelin)
  if [[ "$PROFILE" == "release" ]]; then
    build_cmd+=(--release)
  fi
  run_cmd "${build_cmd[@]}"
fi

if [[ ! -f "$SOURCE_BIN" ]]; then
  echo "Compiled binary not found: $SOURCE_BIN" >&2
  echo "Run without --skip-build or build the selected profile first." >&2
  exit 1
fi

sync_file "$SOURCE_BIN" "$DEST_BIN" "binary" "755"
sync_file "$SOURCE_CONFIG" "$DEST_CONFIG" "config" "644"

echo
echo "Done."
echo "Repo root:    $REPO_ROOT"
echo "Target root:  $TARGET_ROOT"
echo "Binary path:  $DEST_BIN"
echo "Config path:  $DEST_CONFIG"
