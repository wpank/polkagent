#!/usr/bin/env bash
#
# scripts/setup-binaries.sh
#
# Downloads and verifies pinned versions of the Polkadot node binaries and
# zombienet, then installs them into a local cache directory.
#
# Supported platforms:
#   linux-x86_64        → GitHub releases publish a plain "polkadot" binary
#   macos-aarch64       → published as "polkadot-aarch64-apple-darwin"
#   macos-x86_64        → unsupported because the pinned Polkadot release does
#                         not publish Intel-Mac node binaries
#
# Environment variables (all optional — sensible defaults are set):
#
#   POLKADOT_VERSION        Release tag from paritytech/polkadot-sdk.
#                           Defaults to the value in POLKADOT_VERSION file if
#                           present, otherwise falls back to polkadot-stable2606.
#
#   ZOMBIENET_VERSION       Release tag from paritytech/zombienet.
#                           Default: v1.3.138
#
#   POLKAGENT_BIN_DIR       Destination directory for downloaded binaries.
#                           Default: ~/.cache/polkagent/binaries
#
#   SKIP_CHECKSUM           Set to "1" to skip sha256 verification (not
#                           recommended, but useful if upstream checksums are
#                           unavailable for a pre-release build).
#
#   GITHUB_TOKEN            Optional personal access token passed as a Bearer
#                           header to avoid GitHub API rate limiting on CI.
#
# Usage:
#   ./scripts/setup-binaries.sh
#   POLKADOT_VERSION=polkadot-stable2503 ./scripts/setup-binaries.sh
#
set -Euo pipefail

# ─── Colour output ────────────────────────────────────────────────────────────
BOLD="\033[1m"
GREEN="\033[32m"
YELLOW="\033[33m"
RED="\033[31m"
RESET="\033[0m"

info()    { echo -e "${BOLD}[setup-binaries]${RESET} $*"; }
success() { echo -e "${GREEN}[setup-binaries]${RESET} $*"; }
warn()    { echo -e "${YELLOW}[setup-binaries] WARNING:${RESET} $*" >&2; }
die()     { echo -e "${RED}[setup-binaries] ERROR:${RESET} $*" >&2; exit 1; }

# ─── Version resolution ───────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VERSION_FILE="${REPO_ROOT}/POLKADOT_VERSION"

# Precedence: env var > VERSION file > hard-coded default
if [[ -z "${POLKADOT_VERSION:-}" ]]; then
  if [[ -f "${VERSION_FILE}" ]]; then
    POLKADOT_VERSION="$(tr -d '[:space:]' < "${VERSION_FILE}")"
    info "Loaded POLKADOT_VERSION=${POLKADOT_VERSION} from ${VERSION_FILE}"
  else
    POLKADOT_VERSION="polkadot-stable2606"
    info "Using default POLKADOT_VERSION=${POLKADOT_VERSION}"
  fi
fi

ZOMBIENET_VERSION="${ZOMBIENET_VERSION:-v1.3.138}"
POLKAGENT_BIN_DIR="${POLKAGENT_BIN_DIR:-${HOME}/.cache/polkagent/binaries}"
SKIP_CHECKSUM="${SKIP_CHECKSUM:-0}"

# ─── Platform detection ───────────────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}-${ARCH}" in
  Linux-x86_64)
    PLATFORM="linux-x86_64"
    # The plain (no-suffix) binary is the Linux x86_64 release.
    POLKADOT_SUFFIX=""
    ZOMBIENET_ASSET="zombienet-linux-x64"
    ;;
  Darwin-arm64|Darwin-aarch64)
    PLATFORM="macos-aarch64"
    POLKADOT_SUFFIX="-aarch64-apple-darwin"
    ZOMBIENET_ASSET="zombienet-macos-arm64"
    ;;
  Darwin-x86_64)
    die "Intel macOS is unsupported: ${POLKADOT_VERSION} has no x86_64 node binaries"
    ;;
  *)
    die "Unsupported platform: ${OS}-${ARCH}"
    ;;
esac

info "Platform: ${PLATFORM}"
info "polkadot-sdk tag: ${POLKADOT_VERSION}"
info "zombienet tag: ${ZOMBIENET_VERSION}"
info "Binary directory: ${POLKAGENT_BIN_DIR}"

# ─── Helpers ──────────────────────────────────────────────────────────────────

mkdir -p "${POLKAGENT_BIN_DIR}"

# Build curl arguments: add auth header when a token is available.
CURL_AUTH_ARGS=()
if [[ -n "${GITHUB_TOKEN:-}" ]]; then
  CURL_AUTH_ARGS+=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
fi

# download_file <url> <destination>
download_file() {
  local url="$1"
  local dest="$2"
  info "Downloading: $(basename "${dest}")"
  info "  URL: ${url}"
  curl \
    --location \
    --fail \
    --silent \
    --show-error \
    --retry 3 \
    --retry-delay 5 \
    --retry-connrefused \
    "${CURL_AUTH_ARGS[@]}" \
    -o "${dest}" \
    "${url}"
}

# verify_sha256 <binary_path> <sha256_path>
# The upstream .sha256 file contains a single line of the form:
#   <hex-digest>  <filename>
# or just:
#   <hex-digest>
verify_sha256() {
  local binary="$1"
  local sha256_file="$2"

  if [[ "${SKIP_CHECKSUM}" == "1" ]]; then
    warn "SKIP_CHECKSUM=1 — skipping sha256 verification for $(basename "${binary}")"
    return 0
  fi

  local expected
  # Extract only the first field (the hex digest).
  expected="$(awk '{print $1}' "${sha256_file}")"

  local actual
  if command -v sha256sum &>/dev/null; then
    actual="$(sha256sum "${binary}" | awk '{print $1}')"
  elif command -v shasum &>/dev/null; then
    actual="$(shasum -a 256 "${binary}" | awk '{print $1}')"
  else
    warn "Neither sha256sum nor shasum found — skipping checksum verification."
    return 0
  fi

  if [[ "${actual}" == "${expected}" ]]; then
    success "sha256 OK: $(basename "${binary}")"
  else
    die "sha256 mismatch for $(basename "${binary}")!
  expected: ${expected}
  actual:   ${actual}"
  fi
}

# mark_executable_macos <path>
# chmod +x and strip the quarantine attribute that Gatekeeper adds to
# binaries downloaded by curl on macOS.
mark_executable_macos() {
  local path="$1"
  chmod +x "${path}"
  if [[ "${OS}" == "Darwin" ]]; then
    # xattr -d is a no-op when the attribute is not present; suppress error.
    xattr -d com.apple.quarantine "${path}" 2>/dev/null || true
  fi
}

# already_installed <dest_path> <version_marker_path>
# Returns 0 (true) if the binary exists and the version marker matches the
# requested version — meaning we can skip the download entirely.
already_installed() {
  local dest="$1"
  local marker="$2"
  if [[ -f "${dest}" && -f "${marker}" ]]; then
    local installed_version
    installed_version="$(cat "${marker}")"
    if [[ "${installed_version}" == "${POLKADOT_VERSION}" ]]; then
      return 0
    fi
  fi
  return 1
}

# ─── Download a single Polkadot binary ───────────────────────────────────────

GH_BASE="https://github.com/paritytech/polkadot-sdk/releases/download/${POLKADOT_VERSION}"

download_polkadot_binary() {
  local name="$1"          # logical name, e.g. "polkadot"
  local upstream_name="$2" # upstream asset name without suffix, e.g. "polkadot"

  local asset_name="${upstream_name}${POLKADOT_SUFFIX}"
  local dest="${POLKAGENT_BIN_DIR}/${name}"
  local marker="${POLKAGENT_BIN_DIR}/.version-${name}"

  if already_installed "${dest}" "${marker}"; then
    success "${name} already at ${POLKADOT_VERSION} — skipping download."
    return 0
  fi

  local tmp_binary
  tmp_binary="$(mktemp)"
  local tmp_sha256
  tmp_sha256="$(mktemp)"

  # Ensure temp files are cleaned up on exit, even on error.
  # shellcheck disable=SC2064
  trap "rm -f '${tmp_binary}' '${tmp_sha256}'" RETURN

  download_file "${GH_BASE}/${asset_name}" "${tmp_binary}"
  download_file "${GH_BASE}/${asset_name}.sha256" "${tmp_sha256}"

  verify_sha256 "${tmp_binary}" "${tmp_sha256}"

  mv "${tmp_binary}" "${dest}"
  mark_executable_macos "${dest}"
  echo "${POLKADOT_VERSION}" > "${marker}"

  success "Installed ${name} → ${dest}"
}

# ─── Download zombienet ───────────────────────────────────────────────────────

download_zombienet() {
  local dest="${POLKAGENT_BIN_DIR}/zombienet"
  local marker="${POLKAGENT_BIN_DIR}/.version-zombienet"

  if [[ -f "${dest}" && -f "${marker}" && "$(cat "${marker}")" == "${ZOMBIENET_VERSION}" ]]; then
    success "zombienet already at ${ZOMBIENET_VERSION} — skipping download."
    return 0
  fi

  local url="https://github.com/paritytech/zombienet/releases/download/${ZOMBIENET_VERSION}/${ZOMBIENET_ASSET}"
  local tmp_binary
  tmp_binary="$(mktemp)"
  # shellcheck disable=SC2064
  trap "rm -f '${tmp_binary}'" RETURN

  download_file "${url}" "${tmp_binary}"

  # zombienet does not publish .sha256 files; verify the binary at least
  # looks executable (ELF or Mach-O magic bytes).
  local magic
  magic="$(od -An -N4 -tx1 "${tmp_binary}" | tr -d ' ')"
  case "${OS}" in
    Linux)
      [[ "${magic}" == "7f454c46" ]] || die "zombienet binary does not look like an ELF executable (magic: ${magic})"
      ;;
    Darwin)
      # Mach-O: cafebabe (fat) or cffaedfe / feedfacf (thin arm64/x86)
      [[ "${magic}" =~ ^(cafebabe|cffaedfe|feedfacf) ]] || \
        die "zombienet binary does not look like a Mach-O executable (magic: ${magic})"
      ;;
  esac

  mv "${tmp_binary}" "${dest}"
  mark_executable_macos "${dest}"
  echo "${ZOMBIENET_VERSION}" > "${marker}"

  success "Installed zombienet → ${dest}"
}

# ─── Main download sequence ───────────────────────────────────────────────────

info "────────────────────────────────────────────────────────────"
info "Downloading Polkadot node binaries"
info "────────────────────────────────────────────────────────────"

# Core validator binary.
download_polkadot_binary "polkadot" "polkadot"

# Parachain collator binary.
download_polkadot_binary "polkadot-parachain" "polkadot-parachain"

# PVF (Parachain Validation Function) worker processes.
# These live in the same directory as the polkadot binary so the node can
# discover them without a machine-specific path in the Zombienet fixture.
download_polkadot_binary "polkadot-execute-worker" "polkadot-execute-worker"
download_polkadot_binary "polkadot-prepare-worker" "polkadot-prepare-worker"

info "────────────────────────────────────────────────────────────"
info "Downloading zombienet"
info "────────────────────────────────────────────────────────────"

download_zombienet

# ─── Final verification ───────────────────────────────────────────────────────

info "────────────────────────────────────────────────────────────"
info "Installed binaries"
info "────────────────────────────────────────────────────────────"

for bin in polkadot polkadot-parachain polkadot-execute-worker polkadot-prepare-worker zombienet; do
  dest="${POLKAGENT_BIN_DIR}/${bin}"
  if [[ ! -f "${dest}" ]]; then
    die "Expected binary not found: ${dest}"
  fi
  if [[ ! -x "${dest}" ]]; then
    die "Binary is not executable: ${dest}"
  fi
  size="$(du -h "${dest}" | awk '{print $1}')"
  success "  ${bin} (${size})"
done

info ""
info "All binaries ready in: ${POLKAGENT_BIN_DIR}"
info ""
info "Add to PATH:"
info "  export PATH=\"${POLKAGENT_BIN_DIR}:\$PATH\""
info ""
info "Quick-check:"
info "  polkadot --version"
info "  zombienet --version"
