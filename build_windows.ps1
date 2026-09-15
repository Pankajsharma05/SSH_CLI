<#
============================================================
  SSH_CLI — Windows build script

  Usage (from a normal PowerShell prompt, in the repo root):
    .\build_windows.ps1              build -> dist\ssh_cli.exe
    .\build_windows.ps1 -Installer   also produce an .exe installer (NSIS)
    .\build_windows.ps1 -Install     copy to %LOCALAPPDATA%\Programs + Start menu

  Requires:
    * Rust (https://rustup.rs) with the MSVC toolchain
    * Visual Studio Build Tools, "Desktop development with C++"
    * WebView2 runtime (preinstalled on Windows 11 and current Windows 10)
    * OpenSSH client — Settings > System > Optional features > OpenSSH Client

  Tested target: Windows 10 22H2 and Windows 11, x86_64.
============================================================
#>
[CmdletBinding()]
param(
  [switch]$Installer,
  [switch]$Install
)

$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

$Version = "1.0.0"
function Bold($m) { Write-Host $m -ForegroundColor Cyan }

# ---- 0. prerequisites ----------------------------------------------
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Write-Host "Rust is not installed. Install it from https://rustup.rs (choose the MSVC host), then re-run this script."
  exit 1
}
Bold "==> rust: $(rustc --version)"

$sshExe = Join-Path $env:SystemRoot "System32\OpenSSH\ssh.exe"
if (-not (Test-Path $sshExe)) {
  Write-Warning @"
The Windows OpenSSH client was not found at $sshExe.
SSH_CLI will still build, but it needs ssh.exe at runtime. Install it with:

  Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0

(an elevated PowerShell, or Settings > System > Optional features).
"@
}

# ---- 1. vendor frontend libraries (same set as macOS/Linux) --------
$Vendor = "ui\vendor"
New-Item -ItemType Directory -Force -Path $Vendor, "$Vendor\fonts" | Out-Null

function Fetch($url, $dest) {
  if ((Test-Path $dest) -and ((Get-Item $dest).Length -gt 0)) { return }
  Bold "==> fetching $(Split-Path $dest -Leaf)"
  try {
    Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
  } catch {
    # jsDelivr occasionally rate-limits; unpkg serves the same packages.
    $alt = $url -replace 'cdn\.jsdelivr\.net/npm', 'unpkg.com'
    Invoke-WebRequest -Uri $alt -OutFile $dest -UseBasicParsing
  }
}

Fetch "https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"                    "$Vendor\xterm.js"
Fetch "https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css"                   "$Vendor\xterm.css"
Fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-fit@0.10.0/lib/addon-fit.js"           "$Vendor\addon-fit.js"
Fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-search@0.15.0/lib/addon-search.js"     "$Vendor\addon-search.js"
Fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-web-links@0.11.0/lib/addon-web-links.js" "$Vendor\addon-web-links.js"
Fetch "https://cdn.jsdelivr.net/npm/@xterm/addon-image@0.8.0/lib/addon-image.js"        "$Vendor\addon-image.js"

$CM = "https://cdn.jsdelivr.net/npm/codemirror@5.65.18"
Fetch "$CM/lib/codemirror.js"             "$Vendor\codemirror.js"
Fetch "$CM/lib/codemirror.css"            "$Vendor\codemirror.css"
Fetch "$CM/mode/python/python.js"         "$Vendor\cm-python.js"
Fetch "$CM/mode/shell/shell.js"           "$Vendor\cm-shell.js"
Fetch "$CM/mode/clike/clike.js"           "$Vendor\cm-clike.js"
Fetch "$CM/mode/stex/stex.js"             "$Vendor\cm-stex.js"
Fetch "$CM/mode/yaml/yaml.js"             "$Vendor\cm-yaml.js"
Fetch "$CM/mode/toml/toml.js"             "$Vendor\cm-toml.js"
Fetch "$CM/mode/rust/rust.js"             "$Vendor\cm-rust.js"
Fetch "$CM/mode/julia/julia.js"           "$Vendor\cm-julia.js"
Fetch "$CM/mode/javascript/javascript.js" "$Vendor\cm-javascript.js"
Fetch "$CM/mode/markdown/markdown.js"     "$Vendor\cm-markdown.js"

Fetch "https://cdn.jsdelivr.net/npm/@fontsource-variable/inter@5.2.5/files/inter-latin-wght-normal.woff2" `
      "$Vendor\fonts\inter-var.woff2"
Fetch "https://cdn.jsdelivr.net/npm/@fontsource-variable/jetbrains-mono@5.2.5/files/jetbrains-mono-latin-wght-normal.woff2" `
      "$Vendor\fonts\jetbrains-mono-var.woff2"

# ---- 2. compile ----------------------------------------------------
Bold "==> building release binary"
Push-Location src-tauri
try { cargo build --release } finally { Pop-Location }
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force -Path dist | Out-Null
Copy-Item "src-tauri\target\release\ssh_cli.exe" "dist\ssh_cli.exe" -Force
Bold "==> built: dist\ssh_cli.exe"

# ---- 3. optional installer -----------------------------------------
if ($Installer) {
  if (-not (Get-Command cargo-tauri -ErrorAction SilentlyContinue)) {
    Bold "==> installing the tauri CLI (one time)"
    cargo install tauri-cli --version "^2" --locked
  }
  Bold "==> building the NSIS installer"
  Push-Location src-tauri
  try {
    # bundle.active is false in tauri.conf.json so the plain builds stay
    # fast; turn it on just for this invocation.
    cargo tauri build --config '{\"bundle\":{\"active\":true}}' --bundles nsis
  } finally { Pop-Location }
  $nsis = Get-ChildItem "src-tauri\target\release\bundle\nsis\*.exe" -ErrorAction SilentlyContinue
  if ($nsis) {
    Copy-Item $nsis.FullName "dist\SSH_CLI_${Version}_x64-setup.exe" -Force
    Bold "==> installer: dist\SSH_CLI_${Version}_x64-setup.exe"
  } else {
    Write-Warning "NSIS bundle not found — check the cargo tauri output above."
  }
}

# ---- 4. optional per-user install ----------------------------------
if ($Install) {
  $target = Join-Path $env:LOCALAPPDATA "Programs\SSH_CLI"
  New-Item -ItemType Directory -Force -Path $target | Out-Null
  Copy-Item "dist\ssh_cli.exe" (Join-Path $target "ssh_cli.exe") -Force

  $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
  $shortcut  = Join-Path $startMenu "SSH_CLI.lnk"
  $ws = New-Object -ComObject WScript.Shell
  $sc = $ws.CreateShortcut($shortcut)
  $sc.TargetPath = Join-Path $target "ssh_cli.exe"
  $sc.WorkingDirectory = $target
  $sc.Description = "Dual-pane SSH file manager with terminals"
  $sc.Save()
  Bold "==> installed to $target — find SSH_CLI in the Start menu"
}

Bold "Done. Run it with:  .\dist\ssh_cli.exe"
