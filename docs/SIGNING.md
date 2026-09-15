# Code signing

**Short answer: SSH_CLI releases are not digitally signed.** This page explains
exactly what that means, how to install anyway, and what it would take to
change.

## What ships today

| Platform | Signature | What the OS does |
|---|---|---|
| **macOS** | **Ad-hoc only** — `build_mac.sh` runs `codesign --force --deep -s -`, which satisfies the Apple Silicon requirement that binaries be *signed with something*, but it is **not** a Developer ID identity and the app is **not notarized** | Gatekeeper refuses the first launch: *"SSH_CLI cannot be opened because the developer cannot be verified."* |
| **Linux** | None. The binary is plain ELF; the `.deb` is not GPG-signed | Nothing blocks execution; `apt` may warn that the package is unsigned |
| **Windows** | — | Not a supported platform |

An ad-hoc signature proves *nothing about who built the app* — it only binds
the code to itself so the loader will accept it. Do not read it as a security
guarantee.

## Opening the unsigned macOS app

Either:

1. **Right-click** `SSH_CLI.app` → **Open** → **Open** in the dialog. macOS
   remembers the exception for that copy. *(A plain double-click will not
   offer the option.)*
2. Or strip the quarantine attribute:
   ```bash
   xattr -dr com.apple.quarantine /Applications/SSH_CLI.app
   ```

Both are you telling macOS you trust this binary. If you'd rather not, build
from source — it takes one command and a few minutes.

## Linux

Nothing to bypass. If you want to check that a download matches the source,
build it yourself and compare: the build is close to reproducible, since the
only inputs are this repo and the pinned versions in `Cargo.lock` and the
build script's `fetch` lines.

## What proper signing would require

### macOS — Developer ID + notarization

1. Join the Apple Developer Program (US$99/year) and create a **Developer ID
   Application** certificate.
2. Sign with a hardened runtime and a secure timestamp:
   ```bash
   codesign --force --deep --options runtime --timestamp \
     -s "Developer ID Application: Pankaj Sharma (TEAMID)" dist/SSH_CLI.app
   ```
3. Notarize and staple:
   ```bash
   ditto -c -k --keepParent dist/SSH_CLI.app /tmp/SSH_CLI.zip
   xcrun notarytool submit /tmp/SSH_CLI.zip \
     --apple-id you@example.com --team-id TEAMID \
     --password "$APP_SPECIFIC_PASSWORD" --wait
   xcrun stapler staple dist/SSH_CLI.app
   ```
4. In `build_mac.sh`, replace the `codesign --force --deep -s -` line with the
   above, guarded by `if [[ -n "$SIGN_IDENTITY" ]]`.

For CI, add these repository secrets and import the certificate into a
temporary keychain in the workflow:

| Secret | What it is |
|---|---|
| `MACOS_CERTIFICATE` | base64 of the `.p12` export |
| `MACOS_CERTIFICATE_PWD` | its password |
| `MACOS_SIGN_IDENTITY` | `Developer ID Application: … (TEAMID)` |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | notarization credentials |

### Windows

Would need an OV or EV code-signing certificate (roughly US$200–600/year, EV
on a hardware token) and `signtool sign /fd sha256 /tr <timestamp server>`.
Moot until the app runs on Windows at all — see
[ARCHITECTURE.md](ARCHITECTURE.md#portability).

### Linux

Optional but nice for a hosted apt repository: sign the `.deb` with
`dpkg-sig`, or publish an `InRelease` signed with a project GPG key, and
publish the fingerprint here.

## Until then

Everything is MIT and the build is one command. If provenance matters for your
environment, **build from source** and distribute that internally — that is
the honest answer for an unfunded open-source project.
