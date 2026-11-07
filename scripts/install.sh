#!/usr/bin/env bash
# Pulse installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/xt0n1-t3ch/Pulse-Analytics/main/scripts/install.sh | bash
#
# Arch-based systems (Arch, CachyOS, Manjaro) are pointed to the PKGBUILD in
# packaging/aur/pulse-analytics-bin. Other Linux systems get the release .deb,
# .rpm or .AppImage. Set PULSE_INSTALL_METHOD=appimage|deb|rpm to choose.
# Every download is checked against the release's SHA256SUMS.txt.
set -euo pipefail

REPO="xt0n1-t3ch/Pulse-Analytics"
API="https://api.github.com/repos/${REPO}/releases/latest"

say() { printf '\033[1;36m->\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null || die "curl is required"

is_arch_based() {
  [ -f /etc/arch-release ] && return 0
  [ -r /etc/os-release ] || return 1
  (
    # shellcheck disable=SC1091
    . /etc/os-release
    case " ${ID:-} ${ID_LIKE:-} " in
      *" arch "*) exit 0 ;;
    esac
    exit 1
  )
}

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "sha256sum or shasum is required to verify the download"
  fi
}

OS="$(uname -s)"
ARCH="$(uname -m)"
METHOD="${PULSE_INSTALL_METHOD:-}"

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64) PATTERN='_aarch64\.dmg$' ;;
      x86_64) PATTERN='_x64\.dmg$' ;;
      *) die "Unsupported macOS architecture: $ARCH" ;;
    esac
    METHOD="dmg"
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
      *) die "Unsupported Linux architecture: $ARCH" ;;
    esac
    if [ -z "$METHOD" ]; then
      if is_arch_based; then
        cat <<EOF
This is an Arch-based system. Pulse ships a package recipe that repackages the
release .deb (there is no AUR listing yet):

  git clone --depth 1 https://github.com/${REPO}.git
  cd Pulse-Analytics/packaging/aur/pulse-analytics-bin
  makepkg -si

pacman then owns the install and its updates. To use the AppImage instead:

  curl -fsSL https://raw.githubusercontent.com/${REPO}/main/scripts/install.sh | PULSE_INSTALL_METHOD=appimage bash
EOF
        exit 0
      elif [ -f /etc/debian_version ]; then
        METHOD="deb"
      elif [ -f /etc/redhat-release ] || [ -f /etc/fedora-release ] || [ -f /etc/SuSE-release ]; then
        METHOD="rpm"
      else
        METHOD="appimage"
      fi
    fi
    case "$METHOD" in
      deb) PATTERN="_${DEB_ARCH}\\.deb\$" ;;
      rpm) PATTERN="\\.${RPM_ARCH}\\.rpm\$" ;;
      appimage) PATTERN="_${APPIMAGE_ARCH}\\.AppImage\$" ;;
      *) die "PULSE_INSTALL_METHOD must be appimage, deb or rpm" ;;
    esac
    ;;
  *) die "Unsupported OS: $OS (download an installer from https://github.com/${REPO}/releases/latest)" ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

say "Fetching the latest release for $OS/$ARCH..."
RELEASE_JSON="$(curl -fsSL "$API")" || die "Could not read the latest release from GitHub"

asset_url() {
  printf '%s\n' "$RELEASE_JSON" \
    | grep -Eo '"browser_download_url":[[:space:]]*"[^"]+"' \
    | sed 's/.*"\(https[^"]*\)"/\1/' \
    | grep -E "$1" | head -n1 || true
}

URL="$(asset_url "$PATTERN")"
[ -n "$URL" ] || die "No release asset matches $PATTERN"
SUMS_URL="$(asset_url '/SHA256SUMS\.txt$')"
[ -n "$SUMS_URL" ] || die "The release has no SHA256SUMS.txt, so the download cannot be verified"

NAME="$(basename "$URL")"
FILE="$TMP/$NAME"
say "Downloading $NAME"
curl -fL --progress-bar -o "$FILE" "$URL"
curl -fsSL -o "$TMP/SHA256SUMS.txt" "$SUMS_URL"

EXPECTED="$(awk -v name="$NAME" '$2 == name || $2 == "*" name { print $1; exit }' "$TMP/SHA256SUMS.txt")"
[ -n "$EXPECTED" ] || die "SHA256SUMS.txt has no entry for $NAME"
ACTUAL="$(sha256_of "$FILE")"
[ "$EXPECTED" = "$ACTUAL" ] || die "Checksum mismatch for $NAME (expected $EXPECTED, got $ACTUAL)"
say "Checksum verified"

install_appimage() {
  local bin_dir="${HOME}/.local/bin"
  local data_dir="${XDG_DATA_HOME:-${HOME}/.local/share}"
  local dest="${bin_dir}/pulse.AppImage"
  local icon_dir="${data_dir}/icons/hicolor/128x128/apps"
  local desktop_dir="${data_dir}/applications"

  mkdir -p "$bin_dir" "$icon_dir" "$desktop_dir"
  mv "$FILE" "$dest"
  chmod +x "$dest"

  # The AppImage carries its own icon, and extracting it needs no FUSE.
  if (cd "$TMP" && "$dest" --appimage-extract 'usr/share/icons/hicolor/128x128/apps/*.png' >/dev/null 2>&1); then
    local icon
    icon="$(find "$TMP/squashfs-root/usr/share/icons/hicolor/128x128/apps" -name '*.png' 2>/dev/null | head -n1)"
    if [ -n "$icon" ]; then
      cp "$icon" "${icon_dir}/pulse.png"
    fi
  fi
  [ -f "${icon_dir}/pulse.png" ] || say "Could not extract the icon; the menu entry will use a generic one."

  cat > "${desktop_dir}/pulse.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Pulse
Comment=AI coding analytics and Discord Rich Presence
Exec="${dest}"
Icon=pulse
Terminal=false
Categories=Development;
StartupWMClass=pulse
EOF
  if command -v update-desktop-database >/dev/null; then
    update-desktop-database "$desktop_dir" 2>/dev/null || true
  fi
  if command -v gtk-update-icon-cache >/dev/null; then
    gtk-update-icon-cache -q -t "${data_dir}/icons/hicolor" 2>/dev/null || true
  fi
  say "Installed ${dest}"
  case ":${PATH}:" in
    *":${bin_dir}:"*) ;;
    *) say "Add ${bin_dir} to PATH to start it from a terminal." ;;
  esac
}

case "$METHOD" in
  dmg)
    # Finder reads the image after this script exits, so it cannot stay in the
    # temporary folder.
    mkdir -p "${HOME}/Downloads"
    mv "$FILE" "${HOME}/Downloads/${NAME}"
    say "Opening ${HOME}/Downloads/${NAME} (drag Pulse to Applications)..."
    open "${HOME}/Downloads/${NAME}"
    ;;
  deb)
    say "Installing with apt..."
    sudo apt-get install -y "$FILE"
    ;;
  rpm)
    say "Installing with the system package manager..."
    if command -v dnf >/dev/null; then
      sudo dnf install -y "$FILE"
    elif command -v zypper >/dev/null; then
      sudo zypper --non-interactive install --allow-unsigned-rpm "$FILE"
    else
      sudo yum install -y "$FILE"
    fi
    ;;
  appimage)
    install_appimage
    ;;
esac

printf '\n\033[1;32mdone:\033[0m Pulse is installed. Start it from your application menu.\n'
