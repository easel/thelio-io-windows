# Thelio Io Windows Driver

Windows fan controller service for System76 Thelio systems with Thelio Io board.

## Features

- **PBO-aware fan control** - Designed around AMD PBO temperature limits (85°C) to avoid jet-engine behavior
- **Temperature smoothing** - 5-second rolling average filters transient spikes
- **Intelligent ramping** - Fast ramp to silence threshold (40%), gradual ramp under sustained load
- **Multi-trigger detection** - Ramps fans for high CPU load, CPU power draw, or GPU temperature
- **Thermal safety** - Critical override at 88°C, thermal throttle detection
- **CLI tool** - Diagnostic utility for testing and manual control

## Fan Control Behavior

The service maintains near-silent operation during normal use:

| Temperature | Fan Duty | Behavior |
|-------------|----------|----------|
| < 40°C | 0% | Fans off |
| 40-60°C | 0-40% | Quick ramp to silence threshold |
| 60-85°C | 40% | Hold at silence (inaudible) unless sustained load |
| 85°C+ | 40-100% | Gradual +1%/sec ramp with sustained load |
| 88°C+ | 100% | Critical override (immediate) |

**Sustained load triggers** (any of these for 10+ seconds):
- CPU load ≥50% AND temp ≥85°C
- CPU package power ≥95W
- GPU temp ≥70°C

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
wix build wix/main.wxs -d Version=0.1.0 -d Profile=release -arch x64 ^
    -ext WixToolset.Util.wixext -ext WixToolset.UI.wixext ^
    -o wix/thelio-io-0.1.0-x86_64.msi
```

### Install

Run the MSI installer at `wix/thelio-io-0.1.0-x86_64.msi`

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

- `pbo` - PBO-optimized (default): 40% silence threshold, 100% at 85°C
- `standard` - Standard curve optimized for PBO @ 85°C
- `quiet` - Extended quiet range, fans off until 55°C
- `threadripper2` - For Threadripper 2 systems
- `hedt` - For HEDT platforms
- `xeon` - For Xeon systems

### Interactive Mode Example

```
> thelio-io-cli run pbo
Using PBO-based fan curve (PBO=85°C, Max=100%, Silence=40%)
Found 1 Thelio Io device(s)
Starting fan control loop (Ctrl+C to stop)

 Instant      Avg   Target   Actual    CPU%   PkgW   GpuC  Sustain                State
----------------------------------------------------------------------------------------------------
   72.5C    71.2C    40.0%    40.0%    12.3%    45W    52C       -       holding at silence
   73.1C    71.8C    40.0%    40.0%    15.1%    48W    52C       -       holding at silence
   85.2C    78.4C    60.0%    40.0%    98.2%   142W    55C      3s   holding (no sustained load)
   86.1C    82.3C    80.0%    40.0%    99.1%   145W    56C      8s   holding (no sustained load)
   85.8C    84.1C    95.0%    41.0%*   98.5%   143W    57C     12s*       ramp-up (sustained)
```

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
