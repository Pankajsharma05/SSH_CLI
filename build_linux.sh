#!/usr/bin/env bash
# ============================================================
#  SSH_CLI — Linux (Ubuntu/Debian) build script
#
#  Usage:
#    ./build_linux.sh --deps      install system build deps (uses sudo apt)
#    ./build_linux.sh             build -> dist/ssh_cli
#    ./build_linux.sh --install   also install for this user
#                                 (~/.local/bin + app menu entry)
#    ./build_linux.sh --deb       also produce dist/ssh-cli_<ver>_<arch>.deb
#
#  Requires: Rust (rustup) and the Tauri Linux prerequisites
#  (webkit2gtk etc. — see --deps). OpenSSH client is standard.
#  Tested target: Ubuntu 22.04 / 24.04.
# ============================================================
set -euo pipefail
cd "$(dirname "$0")"

VERSION=1.0.0
DEPS=0; INSTALL=0; DEB=0
for arg in "$@"; do
  case "$arg" in
    --deps)    DEPS=1 ;;
    --install) INSTALL=1 ;;
    --deb)     DEB=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

bold() { printf '\033[1m%s\033[0m\n' "$*"; }

# ---- 0. system dependencies ---------------------------------------
if [[ "$DEPS" == "1" ]]; then
  bold "==> installing Tauri build prerequisites (sudo apt)"
  sudo apt-get update
  sudo apt-get install -y \
    build-essential curl wget file pkg-config \
    libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev \
    libssl-dev libxdo-dev
  bold "==> prerequisites installed — run ./build_linux.sh to build"
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is not installed. Install it with:"
  echo
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  echo
  echo "then:  ./build_linux.sh --deps   (once)   and   ./build_linux.sh"
  exit 1
fi
if ! pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
  echo "webkit2gtk dev libraries not found."
  echo "Run:  ./build_linux.sh --deps"
  exit 1
fi
bold "==> rust: $(rustc --version)"

# ---- 1. vendor frontend libraries (same set as macOS) -------------
VENDOR=ui/vendor
mkdir -p "$VENDOR"
fetch() {
  if [[ -s "$2" ]]; then return 0; fi
  bold "==> fetching $(basename "$2")"
  curl -fsSL "$1" -o "$2" || curl -fsSL "${1/cdn.jsdelivr.net\/npm/unpkg.com}" -o "$2"
}
fetch "https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"          "$VENDOR/xterm.js"
fetch "https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css"         "$VENDOR/xterm.css"
fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-fit@0.10.0/lib/addon-fit.js" "$VENDOR/addon-fit.js"
fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-search@0.15.0/lib/addon-search.js"       "$VENDOR/addon-search.js"
fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-web-links@0.11.0/lib/addon-web-links.js" "$VENDOR/addon-web-links.js"
fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-image@0.8.0/lib/addon-image.js"           "$VENDOR/addon-image.js"
CM="https://cdn.jsdelivr.net/npm/codemirror@5.65.18"
fetch "$CM/lib/codemirror.js"             "$VENDOR/codemirror.js"
fetch "$CM/lib/codemirror.css"            "$VENDOR/codemirror.css"
fetch "$CM/mode/python/python.js"         "$VENDOR/cm-python.js"
fetch "$CM/mode/shell/shell.js"           "$VENDOR/cm-shell.js"
fetch "$CM/mode/clike/clike.js"           "$VENDOR/cm-clike.js"
fetch "$CM/mode/stex/stex.js"             "$VENDOR/cm-stex.js"
fetch "$CM/mode/yaml/yaml.js"             "$VENDOR/cm-yaml.js"
fetch "$CM/mode/toml/toml.js"             "$VENDOR/cm-toml.js"
fetch "$CM/mode/rust/rust.js"             "$VENDOR/cm-rust.js"
fetch "$CM/mode/julia/julia.js"           "$VENDOR/cm-julia.js"
fetch "$CM/mode/javascript/javascript.js" "$VENDOR/cm-javascript.js"
fetch "$CM/mode/markdown/markdown.js"     "$VENDOR/cm-markdown.js"
# UI fonts (already shipped in this folder; fetched only if missing)
mkdir -p "$VENDOR/fonts"
fetch "https://cdn.jsdelivr.net/npm/@fontsource-variable/inter@5.2.5/files/inter-latin-wght-normal.woff2" \
      "$VENDOR/fonts/inter-var.woff2"
fetch "https://cdn.jsdelivr.net/npm/@fontsource-variable/jetbrains-mono@5.2.5/files/jetbrains-mono-latin-wght-normal.woff2" \
      "$VENDOR/fonts/jetbrains-mono-var.woff2"

# ---- 2. compile ----------------------------------------------------
bold "==> building release binary"
( cd src-tauri && cargo build --release )
mkdir -p dist
cp src-tauri/target/release/ssh_cli dist/ssh_cli
bold "==> built: dist/ssh_cli"

DESKTOP_ENTRY="[Desktop Entry]
Name=SSH_CLI
Comment=Dual-pane SSH file manager with remote and local terminals
Exec=ssh_cli
Icon=ssh_cli
Type=Application
Categories=Network;Development;
Terminal=false"

# ---- 3. optional per-user install ---------------------------------
if [[ "$INSTALL" == "1" ]]; then
  bold "==> installing for this user"
  mkdir -p "$HOME/.local/bin" \
           "$HOME/.local/share/applications" \
           "$HOME/.local/share/icons/hicolor/512x512/apps"
  cp dist/ssh_cli "$HOME/.local/bin/ssh_cli"
  cp src-tauri/icons/512x512.png "$HOME/.local/share/icons/hicolor/512x512/apps/ssh_cli.png"
  echo "$DESKTOP_ENTRY" > "$HOME/.local/share/applications/ssh_cli.desktop"
  command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database "$HOME/.local/share/applications" || true
  case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) echo "NOTE: add to your shell rc:   export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
  esac
  bold "==> installed — find SSH_CLI in your app launcher, or run: ssh_cli"
fi

# ---- 4. optional .deb ----------------------------------------------
if [[ "$DEB" == "1" ]]; then
  ARCH="$(dpkg --print-architecture)"
  PKGDIR="$(mktemp -d)/ssh-cli_${VERSION}_${ARCH}"
  bold "==> building dist/ssh-cli_${VERSION}_${ARCH}.deb"
  mkdir -p "$PKGDIR/DEBIAN" \
           "$PKGDIR/usr/bin" \
           "$PKGDIR/usr/share/applications" \
           "$PKGDIR/usr/share/icons/hicolor/512x512/apps"
  cp dist/ssh_cli "$PKGDIR/usr/bin/ssh_cli"
  cp src-tauri/icons/512x512.png "$PKGDIR/usr/share/icons/hicolor/512x512/apps/ssh_cli.png"
  echo "$DESKTOP_ENTRY" > "$PKGDIR/usr/share/applications/ssh_cli.desktop"
  cat > "$PKGDIR/DEBIAN/control" <<CTRL
Package: ssh-cli
Version: ${VERSION}
Architecture: ${ARCH}
Maintainer: Pankaj Sharma <pankajsharma6810@gmail.com>
Depends: openssh-client, libwebkit2gtk-4.1-0, libgtk-3-0
Section: net
Priority: optional
Description: SSH_CLI — dual-pane SSH file manager with terminals
 Dual-pane file manager (local/server and server/server), terminal tabs
 for remote hosts and for this machine, a built-in editor that saves over
 SSH, an image and matplotlib viewer, and a SLURM/PBS queue panel — all
 sharing one multiplexed connection per host via the system OpenSSH client.
CTRL
  dpkg-deb --build --root-owner-group "$PKGDIR" "dist/ssh-cli_${VERSION}_${ARCH}.deb" >/dev/null
  bold "==> dist/ssh-cli_${VERSION}_${ARCH}.deb  (install: sudo apt install ./dist/ssh-cli_${VERSION}_${ARCH}.deb)"
fi

bold "Done. Run it with:  dist/ssh_cli"
