use std::{
    env::current_exe,
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread::sleep,
    time::Duration,
};

use crate::{fan::{FanController, FanControllerOutput, SensorData, ThrottleStatus}, Io};

/// All fan device headers on Thelio Io
pub const FAN_DEVICES: &[&str] = &["CPUF", "INTF", "EXHF"];

/// Find all connected Thelio Io devices
pub fn find_thelio_io_devices() -> io::Result<Vec<Io>> {
    let mut ios = Vec::new();

    for port_info in serialport::available_ports()? {
        if let serialport::SerialPortType::UsbPort(usb_info) = port_info.port_type {
            if usb_info.vid == 0x1209 && usb_info.pid == 0x1776 {
                let port = serialport::new(&port_info.port_name, 115200)
                    .timeout(Duration::from_millis(100))
                    .open()?;

                // Flush any leftover data
                sleep(Duration::from_millis(100));
                let mut io = Io::new(port, 1000);

                // Reset device
                let _ = io.reset();
                sleep(Duration::from_millis(100));

                ios.push(io);
            }
        }
    }

    if ios.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "No Thelio Io devices found (VID: 0x1209, PID: 0x1776)"
        ));
    }

    Ok(ios)
}

/// Find a single Thelio Io device (for CLI use)
pub fn find_thelio_io() -> Option<(String, Box<dyn serialport::SerialPort>)> {
    for port_info in serialport::available_ports().ok()? {
        if let serialport::SerialPortType::UsbPort(usb_info) = port_info.port_type {
            if usb_info.vid == 0x1209 && usb_info.pid == 0x1776 {
                let mut port = serialport::new(&port_info.port_name, 115200)
                    .timeout(Duration::from_millis(100))
                    .open()
                    .ok()?;

                // Flush any leftover data
                sleep(Duration::from_millis(100));
                let mut discard = [0u8; 4096];
                let _ = port.read(&mut discard);

                return Some((port_info.port_name, port));
            }
        }
    }
    None
}

/// Launch the temperature wrapper process
pub fn launch_wrapper() -> io::Result<Child> {
    let bin_path = current_exe()?;
    let bin_dir = bin_path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "Failed to get exe directory")
    })?;
    let wrapper_path = bin_dir.join("thelio-io_wrapper.exe");

    if !wrapper_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Wrapper not found at {:?}", wrapper_path)
        ));
    }

    Command::new(&wrapper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
}

/// Callback for each control loop iteration
pub trait DaemonCallback {
    fn on_update(&mut self, output: &FanControllerOutput);
}

/// Simple callback that does nothing (for service use)
pub struct SilentCallback;
impl DaemonCallback for SilentCallback {
    fn on_update(&mut self, _output: &FanControllerOutput) {}
}

/// Callback that prints to console (for CLI use)
pub struct ConsoleCallback {
    first: bool,
}

impl ConsoleCallback {
    pub fn new() -> Self {
        Self { first: true }
    }
}

impl DaemonCallback for ConsoleCallback {
    fn on_update(&mut self, output: &FanControllerOutput) {
        if self.first {
            println!("Starting fan control loop (Ctrl+C to stop)");
            println!("Smoothing: 5s window | Ramp-up: 1.5s delay | Ramp-down: 10s delay\n");
            println!("{:>8} {:>8} {:>8} {:>8} {:>10} {:>6} {:>10} {:>6} {:>14}",
                "Instant", "Avg", "Target", "Actual", "CPU Clk", "CPU%", "GPU Clk", "GPU%", "State");
            println!("{}", "-".repeat(100));
            self.first = false;
        }

        let change_marker = if output.duty_changed { "*" } else { " " };

        // Format throttle indicator with boost info
        let throttle_indicator = match output.throttle_status {
            ThrottleStatus::None => String::new(),
            ThrottleStatus::Likely => format!(" [THROTTLE? +{}C]", output.throttle_boost / 100),
            ThrottleStatus::Confirmed => format!(" [THROTTLING +{}C]", output.throttle_boost / 100),
        };

        // Format CPU clock as "current/max MHz" or empty if no data
        let cpu_clock_str = if output.cpu_max_clock > 0 {
            format!("{:>4}/{:<4}", output.cpu_clock, output.cpu_max_clock)
        } else {
            "   -    ".to_string()
        };

        // Format GPU clock as "current/max MHz" or empty if no data
        let gpu_clock_str = if output.gpu_max_clock > 0 {
            format!("{:>4}/{:<4}", output.gpu_clock, output.gpu_max_clock)
        } else {
            "   -    ".to_string()
        };

        println!("{:>7.1}C {:>7.1}C {:>7.1}% {:>7.1}%{} {} {:>5.1}% {} {:>5.1}% {:>12}{}",
            output.instant_temp as f32 / 100.0,
            output.smoothed_temp as f32 / 100.0,
            output.target_duty as f32 / 100.0,
            output.actual_duty as f32 / 100.0,
            change_marker,
            cpu_clock_str,
            output.cpu_load,
            gpu_clock_str,
            output.gpu_load,
            output.reason,
            throttle_indicator,
        );
    }
}

/// Run the fan control daemon loop
pub fn run_daemon<C: DaemonCallback>(
    controller: &mut FanController,
    ios: &mut [Io],
    wrapper: &mut Child,
    stop_flag: &AtomicBool,
    callback: &mut C,
) -> io::Result<()> {
    let mut wrapper_in = wrapper.stdin.take().ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "Failed to get wrapper stdin")
    })?;
    let mut wrapper_out = BufReader::new(wrapper.stdout.take().ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, "Failed to get wrapper stdout")
    })?);

    while !stop_flag.load(Ordering::Relaxed) {
        // Request sensor data from wrapper
        wrapper_in.write_all(b"\n")?;
        let mut line = String::new();
        wrapper_out.read_line(&mut line)?;

        // Parse JSON sensor data from wrapper
        let sensor_data = SensorData::from_json(&line).map_err(|err| {
            io::Error::new(io::ErrorKind::InvalidData, err)
        })?;

        let output = controller.update_with_sensors(&sensor_data);

        // Notify callback
        callback.on_update(&output);

        // Send duty commands EVERY loop iteration (not just on change)
        // The Thelio Io firmware has a watchdog that resets to 100% if no commands received
        for io in ios.iter_mut() {
            for device in FAN_DEVICES {
                // Ignore errors for fans that don't exist on this model
                let _ = io.set_duty(device, output.actual_duty);
            }
        }

        sleep(Duration::from_secs(1));
    }

    Ok(())
}

/// Set all fans to a specific duty cycle
pub fn set_all_fans(io: &mut Io, duty_hundredths: u16) {
    for device in FAN_DEVICES {
        let _ = io.set_duty(device, duty_hundredths);
    }
}
