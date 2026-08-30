# PS5 Camera Windows Driver

A Rust Windows driver stack for the **Sony PlayStation 5 HD Camera CFI-ZEY1**. It turns the camera into a UVC webcam for Windows Camera, OBS, and applications that use the native Windows camera driver.

## Install

1. Download `PS5-Camera-Setup.exe` from the [latest release](https://github.com/valtoni/PS5-Camera/releases/latest).
2. Run it and follow the one-window wizard.
3. Approve the UAC prompt when Windows asks.
4. Connect or reconnect the camera.

The installer detects the current state and only offers the appropriate actions: install, repair/reinstall, or remove. It binds WinUSB only to the bootloader (`USB\\VID_05A9&PID_0580`), then installs the upload service and diagnostics. The final UVC device (`USB\\VID_05A9&PID_058C`) continues to use the Windows camera driver.

The wizard follows the Windows display language:

- `en-*` and unsupported languages: English
- `pt-*`, including `pt-PT`: Brazilian Portuguese
- `fr-CA` and `fr-FR`: French
- `es-*`: Spanish
- `de-*`: German
- `ja-*`: Japanese
- `zh-*`: Simplified Chinese

## How it works

```text
PS5 HD Camera in boot mode (05A9:0580)
              │
              ├─ WinUSB + PS5 Camera service
              │          │
              │          └─ loads V1 firmware into RAM
              │
              └─ USB Camera-OV580 (05A9:058C)
                           │
                           └─ Native Windows UVC driver
```

Firmware is not written to the camera. After power is removed or the cable is unplugged, it returns to boot mode; the service uploads the firmware again on the next connection.

## V1.0.1 status

- Automatic firmware upload on connection and reconnection.
- UVC video validated at `1920×1080 @30` and stereo `3840×1080 @30`.
- Single-file installer with native UI, progress reporting, repair, and uninstall.
- WinUSB is limited to boot mode and never replaces the Windows UVC driver.
- Reference firmware pinned by SHA-256 and distributed under the publisher's declared MIT license.

The V1 firmware is `21.01-03.20.00.04-00.00.00.bin`, SHA-256 `10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54`, from commit `8773610978d5a4d91a6a6d8063d48a4f3afcfe5b` of [prosperodev/hdcamera](https://github.com/prosperodev/hdcamera). V1 intentionally uses this MIT reference firmware; a fully independent firmware is future work.

## Distribution note

The current package uses a development signature for the WinUSB catalog. Installation therefore asks for explicit administrator approval to trust the project certificate. It is not Microsoft distribution signing and is not delivered through Windows Update.

## Development and validation

```powershell
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
.\windows\package\test-package.ps1
.\windows\installer\test-installer.ps1
```

The verification workflow runs these checks on Windows. Release assets are assembled and published from `v*` tags by GitHub-hosted `windows-2022` runners. The development signing PFX is supplied exclusively through repository secrets, imported only for the release job, and removed before the ephemeral VM is discarded.

Maintainers configure release signing once, from an elevated terminal with GitHub CLI authentication, without ever committing a PFX:

```powershell
gh auth login
.\windows\package\configure-github-release-signing.ps1 -Repository valtoni/PS5-Camera -DispatchReleaseVersion 1.0.1
```

The helper generates a PFX password locally, writes `PS5CAM_SIGNING_PFX_BASE64` and `PS5CAM_SIGNING_PFX_PASSWORD` as Actions Secrets, and can dispatch a tagged release. A manual release run checks out the existing `v<version>` tag before building, so it produces the tagged source rather than the workflow branch.

## Original project

https://github.com/raleighlittles/PS5-Camera-Firmware-Loader

## Support the project

<a href="bitcoin:bc1qw22nzhyrrk3eq45n4c06tje2q37a8fjtslrwrm"><img src="assets/bitcoin-donation-qr.svg" width="180" alt="Bitcoin donation QR code" /></a>

Bitcoin: [`bc1qw22nzhyrrk3eq45n4c06tje2q37a8fjtslrwrm`](bitcoin:bc1qw22nzhyrrk3eq45n4c06tje2q37a8fjtslrwrm)
