#!/usr/bin/env bash

set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $0 <version> <out-dir> [binary-path]" >&2
  exit 1
fi

VERSION="$1"
OUT_DIR="$2"
BINARY_PATH="${3:-target/release/evelin}"
HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"

if [[ -z "${HOST_TARGET}" ]]; then
  echo "Unable to determine Rust host target" >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"

archive_base="evelin-v${VERSION}-${HOST_TARGET}"

if [[ "${HOST_TARGET}" == *windows* ]]; then
  if [[ ! -f "${BINARY_PATH}.exe" ]]; then
    echo "Missing Windows binary at ${BINARY_PATH}.exe" >&2
    exit 1
  fi

  archive_path="${OUT_DIR}/${archive_base}.zip"
  if command -v 7z >/dev/null 2>&1; then
    7z a -tzip "${archive_path}" "${BINARY_PATH}.exe" >/dev/null
  else
    echo "Missing 7z; cannot create Windows archive" >&2
    exit 1
  fi
else
  if [[ ! -f "${BINARY_PATH}" ]]; then
    echo "Missing Unix binary at ${BINARY_PATH}" >&2
    exit 1
  fi

  archive_path="${OUT_DIR}/${archive_base}.tar.gz"
  tar -C "$(dirname "${BINARY_PATH}")" -czf "${archive_path}" "$(basename "${BINARY_PATH}")"
fi

printf '%s\n' "${archive_path}"
