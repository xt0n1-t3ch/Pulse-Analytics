#!/usr/bin/env bash
# Pulse — installer for macOS & Linux
# Usage: curl -fsSL https://raw.githubusercontent.com/xt0n1-t3ch/Pulse/main/scripts/install.sh | bash
set -euo pipefail

REPO="xt0n1-t3ch/Pulse-Analytics"
API="https://api.github.com/repos/${REPO}/releases/latest"

say() { printf '\033[1;36m→\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m✗\033[0m %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null || die "curl is required"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64) PATTERN='_aarch64.dmg$' ;;
      x86_64) PATTERN='_x64.dmg$' ;;
      *) die "Unsupported macOS arch: $ARCH" ;;
    esac
    ;;
  Linux)
    case "$ARCH" in
      x86_64|amd64)
        DEB_ARCH='amd64'
        RPM_ARCH='x86_64'
        APPIMAGE_ARCH='amd64'
        ;;
      aarch64|arm64)
        DEB_ARCH='arm64'
        RPM_ARCH='aarch64'
        APPIMAGE_ARCH='aarch64'
        ;;
      *) die "Unsupported Linux arch: $ARCH" ;;
    esac
    if [ -f /etc/debian_version ]; then
      PATTERN="_${DEB_ARCH}\\.deb$"
    elif [ -f /etc/redhat-release ] || [ -f /etc/fedora-release ]; then
      PATTERN="\\.${RPM_ARCH}\\.rpm$"
    else
      PATTERN="_${APPIMAGE_ARCH}\\.AppImage$"
    fi
    ;;
  *) die "Unsupported OS: $OS (try the manual download)" ;;
esac

say "Fetching latest release for $OS/$ARCH..."
URL="$(curl -fsSL "$API" | grep -Eo '"browser_download_url":[[:space:]]*"[^"]+"' | sed 's/.*"\(https[^"]*\)"/\1/' | grep -E "$PATTERN" | head -n1 || true)"
[ -z "${URL:-}" ] && die "No matching asset found for pattern: $PATTERN"

TMP="$(mktemp -d)"
FILE="$TMP/$(basename "$URL")"
say "Downloading $URL"
curl -fL -o "$FILE" "$URL"

case "$FILE" in
  *.dmg)
    say "Opening installer (drag Pulse to /Applications)..."
    open "$FILE"
    ;;
  *.deb)
    say "Installing via apt..."
    sudo apt-get install -y "$FILE"
    ;;
  *.rpm)
    say "Installing via dnf/yum..."
    sudo dnf install -y "$FILE" 2>/dev/null || sudo yum install -y "$FILE"
    ;;
  *.AppImage)
    DEST="${HOME}/.local/bin/pulse.AppImage"
    mkdir -p "$(dirname "$DEST")"
    mv "$FILE" "$DEST"
    chmod +x "$DEST"
    say "Installed to $DEST — add ~/.local/bin to PATH if needed."
    ;;
esac

printf '\n\033[1;32m✓\033[0m Pulse installed. Launch it from your app menu.\n'
