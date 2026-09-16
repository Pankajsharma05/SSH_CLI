#!/usr/bin/env bash
# ============================================================
#  SSH_CLI — macOS build script
#
#  Usage:
#    ./build_mac.sh              build -> dist/SSH_CLI.app  (current arch)
#    ./build_mac.sh --universal  Apple Silicon + Intel in one binary
#    ./build_mac.sh --dmg        also produce dist/SSH_CLI.dmg
#    ./build_mac.sh --install    copy the app into /Applications
#
#  Requires: Rust (rustup) and the Xcode command-line tools.
#  First build compiles Tauri (~a few minutes); later builds are fast.
# ============================================================
set -euo pipefail
cd "$(dirname "$0")"

VERSION=1.1.0
UNIVERSAL=0; DMG=0; INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --universal) UNIVERSAL=1 ;;
    --dmg)       DMG=1 ;;
    --install)   INSTALL=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

bold() { printf '\033[1m%s\033[0m\n' "$*"; }

# ---- 0. prerequisites ---------------------------------------------
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is not installed. Install it with:"
  echo
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  echo
  exit 1
fi
if ! xcode-select -p >/dev/null 2>&1; then
  echo "Xcode command-line tools not found. Install with:  xcode-select --install"
  exit 1
fi
bold "==> rust: $(rustc --version)"

# ---- 1. vendor frontend libraries (skipped if already present) ----
VENDOR=ui/vendor
mkdir -p "$VENDOR" "$VENDOR/fonts"
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
# v2 fonts (already shipped in this folder; fetched only if missing)
fetch "https://cdn.jsdelivr.net/npm/@fontsource-variable/inter@5.2.5/files/inter-latin-wght-normal.woff2" \
      "$VENDOR/fonts/inter-var.woff2"
fetch "https://cdn.jsdelivr.net/npm/@fontsource-variable/jetbrains-mono@5.2.5/files/jetbrains-mono-latin-wght-normal.woff2" \
      "$VENDOR/fonts/jetbrains-mono-var.woff2"

# ---- 2. compile ----------------------------------------------------
if [[ "$UNIVERSAL" == "1" ]]; then
  bold "==> building universal binary (arm64 + x86_64)"
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  ( cd src-tauri && cargo build --release --target aarch64-apple-darwin )
  ( cd src-tauri && cargo build --release --target x86_64-apple-darwin )
  mkdir -p dist
  lipo -create \
    src-tauri/target/aarch64-apple-darwin/release/ssh_cli \
    src-tauri/target/x86_64-apple-darwin/release/ssh_cli \
    -output dist/ssh_cli_bin
else
  bold "==> building release binary ($(uname -m))"
  ( cd src-tauri && cargo build --release )
  mkdir -p dist
  cp src-tauri/target/release/ssh_cli dist/ssh_cli_bin
fi

# ---- 3. app icon (.icns from icon.png) -----------------------------
ICONSET="$(mktemp -d)/ssh_cli.iconset"
mkdir -p "$ICONSET"
SRC_ICON=src-tauri/icons/icon.png
for s in 16 32 64 128 256 512; do
  sips -z $s $s       "$SRC_ICON" --out "$ICONSET/icon_${s}x${s}.png"      >/dev/null
  sips -z $((s*2)) $((s*2)) "$SRC_ICON" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o dist/ssh_cli.icns

# ---- 4. assemble SSH_CLI.app ---------------------------------------
APP=dist/SSH_CLI.app
bold "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
mv dist/ssh_cli_bin "$APP/Contents/MacOS/ssh_cli"
mv dist/ssh_cli.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>             <string>SSH_CLI</string>
  <key>CFBundleDisplayName</key>      <string>SSH_CLI</string>
  <key>CFBundleIdentifier</key>       <string>io.github.pankajsharma05.ssh-cli</string>
  <key>CFBundleVersion</key>          <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundlePackageType</key>      <string>APPL</string>
  <key>CFBundleExecutable</key>       <string>ssh_cli</string>
  <key>CFBundleIconFile</key>         <string>icon</string>
  <key>LSMinimumSystemVersion</key>   <string>10.15</string>
  <key>NSHighResolutionCapable</key>  <true/>
  <key>NSHumanReadableCopyright</key> <string>MIT</string>
</dict>
</plist>
PLIST
# ad-hoc signature so Gatekeeper on Apple Silicon will run it
codesign --force --deep -s - "$APP"
bold "==> built: $APP"

# ---- 5. optional .dmg ----------------------------------------------
if [[ "$DMG" == "1" ]]; then
  bold "==> building dist/SSH_CLI.dmg"
  rm -f dist/SSH_CLI.dmg
  STAGE="$(mktemp -d)/SSH_CLI"
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "SSH_CLI" -srcfolder "$STAGE" -ov -format UDZO dist/SSH_CLI.dmg >/dev/null
  bold "==> dist/SSH_CLI.dmg"
fi

# ---- 6. optional install -------------------------------------------
if [[ "$INSTALL" == "1" ]]; then
  bold "==> installing into /Applications"
  rm -rf /Applications/SSH_CLI.app
  cp -R "$APP" /Applications/
fi

bold "Done. Run it with:  open $APP"
