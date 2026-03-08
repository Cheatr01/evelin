#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURE_ROOT="${REPO_ROOT}/integration-tests"

CONFIG_REL="${1:-tests/skills/hello-world/invocation.eval.yaml}"
CONFIG_PATH="${FIXTURE_ROOT}/${CONFIG_REL}"

if [[ ! -f "${CONFIG_PATH}" ]]; then
  echo "Missing eval config: ${CONFIG_PATH}" >&2
  exit 1
fi

SKILL_NAME="$(basename "$(dirname "${CONFIG_REL}")")"
CONFIG_FILE="$(basename "${CONFIG_REL}")"
CONFIG_BASE="${CONFIG_FILE%.yaml}"
CONFIG_BASE="${CONFIG_BASE%.yml}"
CONFIG_BASE="${CONFIG_BASE%.json}"

DEFAULT_OUT="${FIXTURE_ROOT}/results/${SKILL_NAME}/${CONFIG_BASE}.json"
OUT_PATH="${2:-${DEFAULT_OUT}}"

CMD=(
  cargo run
  --manifest-path "${REPO_ROOT}/Cargo.toml"
  --
  eval
  --root "${FIXTURE_ROOT}"
  --config "${CONFIG_REL}"
  --out "${OUT_PATH}"
)

if [[ "${EVELIN_DRY_RUN:-0}" == "1" ]]; then
  printf '%q ' "${CMD[@]}"
  printf '\n'
  exit 0
fi

"${CMD[@]}"
