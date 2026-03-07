#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'EOF'
Usage: install-evelin.sh <version>

Install a specific evelin release archive into a local bin directory.

Environment:
  EVELIN_VERSION            Default version if no positional argument is given
  EVELIN_INSTALL_DIR        Target directory, default: $HOME/.local/bin
  EVELIN_RELEASE_BASE_URL   Full base URL that contains /v<version>/ artifacts
  EVELIN_S3_BUCKET          Bucket name used to derive the base URL when EVELIN_RELEASE_BASE_URL is unset
  EVELIN_S3_REGION          Region for derived S3 URL, default: eu-west-1
  EVELIN_S3_PREFIX          Prefix for derived S3 URL, default: evelin
EOF
}

fail() {
  echo "install-evelin: $*" >&2
  exit 1
}

need_fetch_tool() {
  if command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1; then
    return 0
  fi
  fail "curl or wget is required"
}

fetch() {
  local url="$1"
  local destination="$2"

  if command -v curl >/dev/null 2>&1; then
    curl --fail --silent --show-error --location --retry 3 --output "${destination}" "${url}"
    return 0
  fi
  if command -v wget >/dev/null 2>&1; then
    wget -qO "${destination}" "${url}"
    return 0
  fi

  fail "curl or wget is required"
}

verify_checksum() {
  local work_dir="$1"
  local manifest="$2"

  if command -v sha256sum >/dev/null 2>&1; then
    (
      cd "${work_dir}"
      sha256sum -c "${manifest}" >/dev/null
    )
    return 0
  fi
  if command -v shasum >/dev/null 2>&1; then
    (
      cd "${work_dir}"
      shasum -a 256 -c "${manifest}" >/dev/null
    )
    return 0
  fi

  fail "sha256sum or shasum is required to verify downloads"
}

detect_target() {
  local os
  local arch

  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
      fail "Windows shell installation is not supported; download the published zip artifact manually"
      ;;
    *)
      fail "unsupported operating system: ${os}"
      ;;
  esac

  case "${arch}" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)
      fail "unsupported CPU architecture: ${arch}"
      ;;
  esac

  case "${os}:${arch}" in
    linux:x86_64) printf 'x86_64-unknown-linux-gnu\n' ;;
    linux:aarch64) printf 'aarch64-unknown-linux-gnu\n' ;;
    macos:x86_64) printf 'x86_64-apple-darwin\n' ;;
    macos:aarch64) printf 'aarch64-apple-darwin\n' ;;
    *)
      fail "unsupported platform combination: ${os}/${arch}"
      ;;
  esac
}

resolve_base_url() {
  if [[ -n "${EVELIN_RELEASE_BASE_URL:-}" ]]; then
    printf '%s\n' "${EVELIN_RELEASE_BASE_URL%/}"
    return 0
  fi

  local bucket="${EVELIN_S3_BUCKET:-}"
  local region="${EVELIN_S3_REGION:-eu-west-1}"
  local prefix="${EVELIN_S3_PREFIX:-evelin}"

  if [[ -z "${bucket}" ]]; then
    fail "set EVELIN_RELEASE_BASE_URL or EVELIN_S3_BUCKET"
  fi

  printf 'https://s3.%s.amazonaws.com/%s/%s\n' "${region}" "${bucket}" "${prefix}"
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

VERSION="${1:-${EVELIN_VERSION:-}}"
if [[ -z "${VERSION}" ]]; then
  usage >&2
  fail "missing version argument"
fi
VERSION="${VERSION#v}"

need_fetch_tool

TARGET="$(detect_target)"
ARCHIVE_NAME="evelin-v${VERSION}-${TARGET}.tar.gz"
BASE_URL="$(resolve_base_url)"
RELEASE_URL="${BASE_URL}/v${VERSION}"
INSTALL_DIR="${EVELIN_INSTALL_DIR:-${HOME}/.local/bin}"
TMP_DIR="$(mktemp -d)"
trap 'find "${TMP_DIR}" -type f -delete >/dev/null 2>&1 || true; find "${TMP_DIR}" -type d -empty -delete >/dev/null 2>&1 || true' EXIT

CHECKSUMS_PATH="${TMP_DIR}/SHA256SUMS"
ARCHIVE_PATH="${TMP_DIR}/${ARCHIVE_NAME}"
FILTERED_MANIFEST="${TMP_DIR}/SHA256SUMS.filtered"

fetch "${RELEASE_URL}/SHA256SUMS" "${CHECKSUMS_PATH}"
fetch "${RELEASE_URL}/${ARCHIVE_NAME}" "${ARCHIVE_PATH}"

if ! grep -F "./${ARCHIVE_NAME}" "${CHECKSUMS_PATH}" > "${FILTERED_MANIFEST}"; then
  fail "checksum entry for ${ARCHIVE_NAME} not found in SHA256SUMS"
fi

verify_checksum "${TMP_DIR}" "$(basename "${FILTERED_MANIFEST}")"

tar -xzf "${ARCHIVE_PATH}" -C "${TMP_DIR}"

if [[ ! -f "${TMP_DIR}/evelin" ]]; then
  fail "archive ${ARCHIVE_NAME} did not contain evelin binary"
fi

mkdir -p "${INSTALL_DIR}"
install -m 0755 "${TMP_DIR}/evelin" "${INSTALL_DIR}/evelin"

echo "Installed evelin ${VERSION} to ${INSTALL_DIR}/evelin"
