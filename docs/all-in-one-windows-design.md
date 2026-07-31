# All-in-one Windows design

## Product goal

The user installs one signed Windows package, connects a supported PlayStation camera, and the camera becomes available to applications as a standard webcam. No separate firmware download, Zadig step, command prompt, or manual reloading is required.

## Device lifecycle

The camera has two relevant USB states:

- `05a9:0580` — OmniVision USB Boot mode. The factory firmware does not expose a usable UVC webcam interface.
- `05a9:058c` — USB Camera-OV580 mode after compatible UVC firmware has been uploaded for the current power cycle.

The firmware upload is volatile. Disconnecting power returns the camera to USB Boot mode, so the installed software must detect the device and upload the UVC firmware automatically each time.

## Installed components

### 1. Windows installer

A single elevated installer must:

1. install the application files under `Program Files`;
2. install a signed WinUSB driver package for `USB\VID_05A9&PID_0580` only;
3. install and start the Windows service;
4. optionally install a small diagnostics UI;
5. verify that the service can detect the camera.

The UVC device `05a9:058c` must not be bound to WinUSB. It must remain associated with the Microsoft USB Video Class driver.

Recommended packaging: WiX Toolset/Burn or Inno Setup. The first production milestone should use WiX because it provides stronger MSI/service/driver lifecycle support.

### 2. WinUSB device package

The package contains an INF matching only:

```text
USB\VID_05A9&PID_0580
```

The INF selects Microsoft's in-box WinUSB function driver. The package still needs a catalog and a trusted signature for a warning-free production installation.

Development builds may use test signing, but a public installer requires a code-signing certificate and Windows-compatible driver-package signing.

### 3. Rust Windows service

The service runs under `LocalSystem` and performs the following state machine:

```text
WaitingForDevice
    -> BootDeviceDetected
    -> FirmwareValidated
    -> UploadingFirmware
    -> WaitingForReenumeration
    -> UvcReady
```

It watches for device arrival/removal and also performs a low-frequency reconciliation scan so startup races and missed notifications do not leave the camera unusable.

The service must:

- ignore a camera already running as `05a9:058c`;
- claim interface 0 of `05a9:0580` through libusb/WinUSB;
- upload firmware in 512-byte control transfers;
- derive `wValue` and `wIndex` from the complete byte offset, supporting firmware larger than 64 KiB;
- send the final `0x5b / 0x2200 / 0x8018` command;
- treat `NoDevice` on the final command as expected re-enumeration;
- wait for `05a9:058c` and report a useful error if it does not appear;
- retry only transient USB failures with bounded backoff;
- avoid per-packet logging because timing disturbances can break uploads on some controllers.

### 4. Embedded firmware resource

The production executable should contain the approved UVC firmware as a Windows resource or compile-time byte asset. The firmware must be validated before upload using:

- exact byte length;
- SHA-256 allowlist;
- optional internal version identifier.

A development command may accept an external firmware path, but normal users must never be asked to locate `firmware.bin`.

Redistribution rights for the selected firmware must be confirmed before publishing binaries. If redistribution is not permitted, a truly offline all-in-one installer cannot legally include it; the alternative would be a first-run download from an authorized source, which is less desirable and should not be the default design.

### 5. Diagnostics application

A small optional UI should show only actionable states:

- Camera disconnected
- Preparing camera
- Camera ready
- Driver installation required
- Firmware rejected
- USB 3 connection recommended
- Upload failed, with a copyable diagnostic code

It should expose a button to restart the service and export logs, but firmware upload remains automatic.

## Repository structure

```text
PS5-Camera/
├── rust/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── device.rs
│       ├── firmware.rs
│       ├── uploader.rs
│       ├── service.rs
│       └── status.rs
├── assets/
│   └── firmware/
│       ├── README.md
│       └── approved-firmware.bin
├── packaging/
│   └── windows/
│       ├── driver/
│       │   └── ps5-camera-boot.inf
│       ├── wix/
│       │   └── Product.wxs
│       └── signing/
│           └── README.md
└── docs/
    └── all-in-one-windows-design.md
```

## CLI retained for support

The installed executable may expose administrative commands for diagnostics, while the regular user never needs them:

```text
ps5-camera-service service install
ps5-camera-service service start
ps5-camera-service status
ps5-camera-service firmware verify
ps5-camera-service firmware load --file <path>   # development only
```

## Security boundaries

- Match exact VID/PID and interface before sending vendor control transfers.
- Validate firmware before opening the device for upload.
- Do not download or execute firmware supplied through an unauthenticated channel.
- Run the service with the minimum Windows privileges compatible with device access and service management.
- Protect service configuration and firmware assets from modification by standard users.
- Sign the executable, installer, INF catalog, and release metadata.

## Delivery milestones

1. Refactor the Rust loader into a tested library with correct cross-platform USB behavior.
2. Add embedded-firmware validation and automatic UVC re-enumeration checks.
3. Add the Windows service and automatic device detection.
4. Add the WinUSB INF and development/test-sign installation path.
5. Add the WiX all-in-one installer.
6. Sign and test clean installation, upgrade, repair, and uninstall on Windows 10 and Windows 11.

## Definition of done

On a clean supported Windows machine:

1. the user runs one installer;
2. Windows displays the expected publisher identity;
3. the user connects the camera to USB 3;
4. the service automatically uploads the bundled UVC firmware;
5. the camera appears in the Windows Camera app, OBS, Teams, and other UVC clients;
6. unplugging and reconnecting the camera requires no user action;
7. uninstall removes the service, application, and boot-mode WinUSB package without altering the Microsoft UVC driver.
