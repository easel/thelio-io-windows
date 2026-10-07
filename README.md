# Thelio Io Windows Driver

Windows fan controller service for System76 Thelio systems with Thelio Io board.

## Features

- **Configurable temperature target** - Defaults to 90°C for the Ryzen 5950X; this is a controller setting and does not change the processor's PBO or BIOS limits
- **Temperature smoothing** - Five-sample rolling average filters transient spikes
- **Controlled ramping** - Quick rise to the configured quiet threshold (40% by default), then gradual increases under sustained load
- **Multi-trigger detection** - Sustained CPU load near the temperature target, CPU package power, or GPU temperature permits ramping above the quiet threshold
- **Critical override** - Instant full fan duty at the configured target plus 3°C (93°C with defaults)
- **CLI tool** - Diagnostic utility for testing and manual control

## Fan Control Behavior

The default service uses the `pbo` curve and the following controller settings:

| Setting | Default |
|---------|---------|
| Temperature target (`PboTemp`) | 90°C |
| Maximum normal duty (`MaxFanDuty`) | 100% |
| Quiet threshold (`SilenceThreshold`) | 40% |
| Sustained CPU load threshold (`SustainedLoadThreshold`) | 50% |
| CPU package power trigger (`CpuPowerThreshold`) | 125W |
| GPU temperature trigger (`GpuTempThreshold`) | 70°C |
| Critical offset (`CriticalTempOffset`) | +3°C |

Service settings can be overridden with DWORD values in
`HKLM\SOFTWARE\ThelioIo`, using whole degrees, percentages, or watts.
Restart the service after changing them. Interactive CLI mode uses the compiled
defaults rather than these registry overrides. Choose temperature settings for
your processor and firmware configuration; the defaults do not configure PBO.

The curve computes a target from the five-sample average of the higher CPU/GPU
temperature. Actual fan duty also depends on sustained load and slowdown delays:

| Averaged temperature | Curve target | Controller behavior |
|----------------------|--------------|---------------------|
| ≤40°C | 0% | Fans can stop after the slowdown delay |
| 40–60°C | Linear 0–40% | Quick rise within the quiet range |
| 60–90°C | Linear 40–100% | Hold at 40% until sustained airflow demand permits further increases |
| ≥90°C | 100% | Increase gradually if sustained airflow demand is present |
| Instantaneous temperature ≥93°C | 100% override | Immediate full duty, bypassing averaging, delays, and the normal duty cap |

**Sustained airflow demand** means any of the following remains true for at least
10 seconds:

- CPU load ≥50% AND averaged control temperature ≥90°C
- CPU package power ≥125W
- GPU temperature ≥70°C

These triggers permit increases toward the curve target; they do not raise that
target independently. Above the quiet threshold, duty increases by one percentage
point per update. The loop sleeps one second between updates, so sensor and device
I/O can make this slower than one percentage point per second. Five samples likewise
represent approximately five seconds, rather than a fixed time window.

Decreases wait 10 seconds before applying the lower target. Duty above 40% can
remain there after sustained demand ends if the curve target still calls for it.
The 40% threshold is a tuning choice; audible noise depends on the installed fans.

The sensor wrapper prefers AMD CCD temperatures, falls back to package/core
readings, and uses the higher selected CPU or GPU temperature for control.
A clock-based throttle detector exists in the source and has unit tests, but the
active daemon loop does not use it to control the fans.

## Installation

### Prerequisites

- Install [Git LFS](https://git-lfs.github.com/) prior to cloning
- Install [Rust](https://rustup.rs/)
- Install [Chocolatey](https://chocolatey.org/install)

### Build Dependencies

Launch an Administrator Command Prompt:
```
choco install dotnet-8.0-sdk wixtoolset
```

### Building

```bash
# Build release binaries
cargo build --release

# Build the .NET wrapper
cd wrapper
dotnet publish -c Release -r win-x64 --self-contained
cd ..

# Build MSI installer (requires WiX 4+)
wix build wix/main.wxs -d Version=0.2.0 -d Profile=release -arch x64 ^
    -ext WixToolset.Util.wixext -ext WixToolset.UI.wixext ^
    -o wix/thelio-io-0.2.0-x86_64.msi
```

### Install

Run the MSI installer at `wix/thelio-io-0.2.0-x86_64.msi`

The installer will:
- Install to `C:\Program Files\System76 Thelio Io\`
- Register and start the `System76 Thelio Io` service
- Install the CLI tool (`thelio-io-cli.exe`)

## CLI Tool

The CLI tool (`thelio-io-cli.exe`) provides diagnostics and manual control:

```
thelio-io-cli status           # Show fan duty and RPM
thelio-io-cli temps            # Show all temperature sensors
thelio-io-cli run <curve>      # Run fan control interactively
thelio-io-cli set <duty>       # Set fan duty manually (0-100)
thelio-io-cli curves           # List available fan curves
```

### Available Curves

- `pbo` - PBO-optimized (default): 40% quiet threshold at 60°C, 100% target at 90°C
- `standard` - Shaped temperature curve reaching a 100% target at 90°C
- `quiet` - Extended quiet range, fans off until 55°C
- `threadripper2` - For Threadripper 2 systems
- `hedt` - For HEDT platforms
- `xeon` - For Xeon systems

### Interactive Mode

```bash
thelio-io-cli run pbo
```

The CLI displays instantaneous and averaged temperatures, curve target and actual
fan duty, CPU load and package power, GPU temperature, sustained-demand duration,
and the controller state. `Target` is the curve request; `Actual` includes the
quiet threshold, gradual ramping, and slowdown delay. Stop with Ctrl+C.

## Logs

Service logs are available in Event Viewer:
- Path: `Windows Logs` → `Application`
- Source: `System76 Thelio Io`

## Architecture

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────┐
│  thelio-io.exe  │────▶│ thelio-io_       │────▶│ LibreHW     │
│  (Rust service) │     │ wrapper.exe      │     │ Monitor     │
└────────┬────────┘     │ (.NET, sensors)  │     └─────────────┘
         │              └──────────────────┘
         │ USB Serial (115200 baud)
         ▼
┌─────────────────┐
│   Thelio Io     │
│   (firmware)    │
│   VID:1209      │
│   PID:1776      │
└─────────────────┘
```

## License

GPL-3.0 - See [LICENSE.md](LICENSE.md)
