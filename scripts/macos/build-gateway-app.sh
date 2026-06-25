#!/usr/bin/env bash
#
# build-gateway-app.sh — package kriya-gateway as a signed macOS .app bundle + .dmg (R24).
#
# WHY a bundle: macOS TCC grants Accessibility to a stable app identity (CFBundleIdentifier), not to
# a file path. A loose binary spawned by an Electron host (Claude Desktop) can't be granted
# Accessibility — we proved this live — but a signed .app with a fixed CFBundleIdentifier can, and
# the grant sticks across runs. This script reproduces that bundle deterministically.
#
# Usage:
#   bash scripts/macos/build-gateway-app.sh [--version X.Y.Z]
#
# Output (under dist/macos/, which is gitignored):
#   dist/macos/Kriya Gateway.app   — ad-hoc-signed bundle (com.kriya.gateway)
#   dist/macos/KriyaGateway.dmg    — drag-to-Applications disk image
#
# Signing: ad-hoc ("--sign -") is fine for a LOCAL demo and for granting Accessibility on the build
# machine. For REAL distribution you need a Developer ID identity + notarization — see README.md.

set -euo pipefail

# ── Resolve repo root relative to this script (so it runs from anywhere) ─────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Parse args ──────────────────────────────────────────────────────────────────────────────────
VERSION="0.1.0"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || { echo "error: --version needs a value" >&2; exit 2; }
      VERSION="$2"; shift 2 ;;
    -h|--help)
      echo "usage: $0 [--version X.Y.Z]"; exit 0 ;;
    *)
      echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Cargo needs its env in a fresh shell; the toolchain is pinned (rust-toolchain).
# shellcheck disable=SC1091
source "$HOME/.cargo/env"

APP_NAME="Kriya Gateway"
BUNDLE_ID="com.kriya.gateway"
DIST_DIR="${REPO_ROOT}/dist/macos"
APP_DIR="${DIST_DIR}/${APP_NAME}.app"
DMG_PATH="${DIST_DIR}/KriyaGateway.dmg"
MANIFEST="${REPO_ROOT}/crates/kriya/Cargo.toml"
PLIST_TEMPLATE="${SCRIPT_DIR}/Info.plist"

echo "==> Building kriya-gateway (release, features: mcp-client,reach-in)"
cargo build --release \
  --manifest-path "${MANIFEST}" \
  --no-default-features \
  --features mcp-client,reach-in \
  --bin kriya-gateway

# The release dir is the workspace target/ if there is one, else the crate's target/. Find the
# freshly built binary rather than assuming a path.
BIN_PATH="$(find "${REPO_ROOT}" -type f -name kriya-gateway -path '*/release/*' -not -path '*/deps/*' -print0 \
  | xargs -0 ls -t 2>/dev/null | head -1 || true)"
if [[ -z "${BIN_PATH}" || ! -x "${BIN_PATH}" ]]; then
  echo "error: could not locate the built kriya-gateway binary under target/*/release/" >&2
  exit 1
fi
echo "==> Built binary: ${BIN_PATH}"

echo "==> Assembling bundle: ${APP_DIR}"
rm -rf "${APP_DIR}"
mkdir -p "${APP_DIR}/Contents/MacOS"

# Copy the binary in as Contents/MacOS/kriya-gateway (matches CFBundleExecutable).
cp "${BIN_PATH}" "${APP_DIR}/Contents/MacOS/kriya-gateway"
chmod +x "${APP_DIR}/Contents/MacOS/kriya-gateway"

# Copy the Info.plist template in, then stamp the version so the bundle reports --version.
cp "${PLIST_TEMPLATE}" "${APP_DIR}/Contents/Info.plist"
PLISTBUDDY="/usr/libexec/PlistBuddy"
if [[ -x "${PLISTBUDDY}" ]]; then
  "${PLISTBUDDY}" -c "Set :CFBundleShortVersionString ${VERSION}" "${APP_DIR}/Contents/Info.plist"
  "${PLISTBUDDY}" -c "Set :CFBundleVersion ${VERSION}" "${APP_DIR}/Contents/Info.plist"
else
  echo "note: PlistBuddy not found — bundle keeps the template's default version" >&2
fi

# ── Ad-hoc codesign (local demo). Real distribution uses Developer ID + notarization (README.md) ──
echo "==> Codesigning (ad-hoc)"
codesign --force --deep --sign - "${APP_DIR}"
echo "==> Signature identity:"
codesign -dvv "${APP_DIR}" 2>&1 | grep -E "Identifier|Signature" || true

# ── Build a simple drag-to-Applications .dmg ─────────────────────────────────────────────────────
echo "==> Building DMG: ${DMG_PATH}"
rm -f "${DMG_PATH}"
hdiutil create \
  -volname "${APP_NAME}" \
  -srcfolder "${APP_DIR}" \
  -ov -format UDZO \
  "${DMG_PATH}" >/dev/null
echo "==> DMG written"

# ── What you got + next steps ────────────────────────────────────────────────────────────────────
GATEWAY_EXE="${APP_DIR}/Contents/MacOS/kriya-gateway"
cat <<EOF

────────────────────────────────────────────────────────────────────────────────
  Kriya Gateway ${VERSION} — packaged.

  App bundle : ${APP_DIR}
  Disk image : ${DMG_PATH}
  Binary     : ${GATEWAY_EXE}
  Bundle id  : ${BUNDLE_ID}  (signed ad-hoc)

  Next steps
  ----------
  1. Install: open the .dmg and drag "Kriya Gateway.app" to /Applications
     (or use the bundle in place).

  2. Grant Accessibility ONCE (required for reach-in):
       System Settings → Privacy & Security → Accessibility → add "Kriya Gateway.app"
     Run the built-in preflight to check + open that pane for you:
       "${GATEWAY_EXE}" doctor --app "Numbers"

  3. Wire into Claude Desktop's claude_desktop_config.json ("mcpServers"), pointing
     command at the BUNDLE path (not a loose binary, or TCC won't grant):
       "command": "${GATEWAY_EXE}"
       "args":    ["reach-in", "--app", "<App Name>", "--approval", "gui"]
     (\`doctor --app "<App Name>"\` prints this snippet ready to paste.)

  Ad-hoc signing is fine locally. For public distribution you need Developer ID +
  notarization — see scripts/macos/README.md.
────────────────────────────────────────────────────────────────────────────────
EOF
