#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURE_ROOT="${SCRIPT_DIR}/fixtures/scope-to-acceptance-project"
OUT_DIR="$(mktemp -d)"
trap 'rm -rf "${OUT_DIR}"' EXIT

SCHEMA_CONFIG="${FIXTURE_ROOT}/tests/src/skills/scope-to-acceptance/suite.eval.yaml"
SCHEMA_OUT="${OUT_DIR}/schema.json"
GATE_OUT="${OUT_DIR}/gate.json"

echo "Running schema-lint against integration fixture"
cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -- \
  schema-lint \
  --config "${SCHEMA_CONFIG}" \
  --out "${SCHEMA_OUT}"

echo "Running gate-lint against integration fixture"
cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -- \
  gate-lint \
  --requirements "${SCHEMA_CONFIG}" \
  --root "${FIXTURE_ROOT}" \
  --out "${GATE_OUT}"

grep -q '"verdict": "pass"' "${SCHEMA_OUT}"
grep -q '"verdict": "pass"' "${GATE_OUT}"

echo "Integration smoke tests passed"
