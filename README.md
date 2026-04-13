# WarmLight

WarmLight is a desktop controller for external monitors that support DDC/CI. It gives you direct control over brightness, contrast, presets, input selection, power, and a small set of warm color scenes from a native desktop UI.

The current implementation is a Tauri app with a Leptos frontend. On Linux it talks to monitors through `ddcutil`. On non-Linux platforms the fallback backend currently supports brightness only.

## What It Does

- Detects connected DDC/CI-capable monitors and lets you switch between them in the UI.
- Reads live monitor state and refreshes it on demand.
- Controls these VCP features when the monitor exposes them:
  - Brightness
  - Contrast
  - Volume
  - Color preset
  - Red gain
  - Green gain
  - Blue gain
  - Input source
  - Display mode
  - Mute
  - Display power
- Applies range changes either instantly or as a stepped ramp using the `Glide` control in the UI.
- Exposes built-in warm color scenes:
  - `Paper`
  - `Sunset`
  - `Ember`
  - `Incandescent`
  - `Candle`
  - `Nocturne`

WarmLight only shows and enables controls that a given monitor actually reports through DDC/CI. Unsupported controls remain visible with an error message so you can see what the app tried to probe.

## Platform Support

### Linux

Linux is the main target right now.

The backend shells out to `ddcutil` for:

- monitor detection
- capability parsing
- reading VCP features
- writing VCP features

`ddcutil` must be installed and available on `PATH`.

### Other Platforms

The non-Linux backend uses `ddc-hi`, but the current implementation only supports brightness there. Contrast, presets, scenes, source switching, and the broader Linux control surface are not implemented outside Linux yet.

## How Scenes Work

The custom scenes are implemented by:

1. switching the monitor to color preset `0x0b` (`User 1 · Custom`)
2. waiting briefly for the monitor to accept the mode change
3. applying red, green, and blue gain targets as percentages of each channel's reported maximum

If a monitor does not expose red, green, and blue gain controls, the custom scene buttons are not shown.

## Development

The repository is a Cargo workspace with three main parts:

- `src/`: Leptos frontend
- `src-tauri/`: native shell and monitor backend
- `crates/shared/`: shared monitor/control data structures

The Tauri config expects:

- `trunk serve --address 127.0.0.1 --port 1420` for dev
- `trunk build` for frontend production builds

Typical local workflow:

```bash
cargo tauri dev
```

Typical production build:

```bash
cargo tauri build
```

You will need the normal Rust toolchain plus the tools implied by the codebase and config:

- Rust
- Trunk
- Tauri CLI
- `ddcutil` on Linux
- the system libraries required by Tauri/WebKit on your distro

## Probe Scripts

The `scripts/` directory contains read-only helpers for diagnosing monitor support and discovering vendor-specific controls.

### `scripts/check-ddc.sh`

Basic environment and monitor sanity check. It prints:

- `ddcutil` version
- available `/dev/i2c-*` devices
- `ddcutil detect`
- per-display reads for brightness, contrast, input source, and capabilities

### `scripts/probe-samsung-vcp.sh`

Samsung-oriented probe script for standard and vendor-specific VCP codes. It never writes values; it only reads capabilities and VCP state.

Examples:

```bash
./scripts/probe-samsung-vcp.sh
./scripts/probe-samsung-vcp.sh --display=1
```

### `scripts/diff-vcp-snapshot.sh`

Captures read-only VCP snapshots and diffs them so you can toggle a monitor OSD setting manually and see what changed.

Examples:

```bash
./scripts/diff-vcp-snapshot.sh capture baseline --display=1
./scripts/diff-vcp-snapshot.sh capture eye-saver-on --display=1
./scripts/diff-vcp-snapshot.sh compare .ddc-probes/baseline-display1-*.txt .ddc-probes/eye-saver-on-display1-*.txt
```

Snapshot output is written to `.ddc-probes/`, which is ignored by git.

## Notes

- The app is built around external displays, not laptop internal panels.
- Detection and support depend on the monitor, cabling, GPU path, and whether DDC/CI is enabled in the monitor OSD.
- On Linux, failures to detect or change controls usually come from missing `ddcutil`, inaccessible I2C devices, or monitors exposing incomplete VCP support.
