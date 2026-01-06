use std::env::{args, current_exe};
use std::time::Duration;
use std::thread::sleep;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Command, Child, Stdio};
use thelio_io::{Io, fan::{FanCurve, FanController}};

fn find_thelio_io() -> Option<(String, Box<dyn serialport::SerialPort>)> {
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

fn cmd_status() {
    match find_thelio_io() {
        Some((name, port)) => {
            println!("Thelio Io at {}", name);

            let mut io = Io::new(port, 1000);

            // Reset device first
            if let Err(e) = io.reset() {
                println!("  Reset failed: {}", e);
            }
            sleep(Duration::from_millis(100));

            // Get revision
            match io.revision() {
                Ok(rev) => println!("  Revision: {}", rev),
                Err(e) => println!("  Revision: Error - {}", e),
            }

            // Get CPU fan info
            match io.duty("CPUF") {
                Ok(duty) => println!("  CPU Fan Duty: {:.1}%", duty as f32 / 100.0),
                Err(e) => println!("  CPU Fan Duty: Error - {}", e),
            }
            match io.tach("CPUF") {
                Ok(rpm) => println!("  CPU Fan RPM: {}", rpm),
                Err(e) => println!("  CPU Fan RPM: Error - {}", e),
            }

            // Get internal fan info
            match io.duty("INTF") {
                Ok(duty) => println!("  Internal Fan Duty: {:.1}%", duty as f32 / 100.0),
                Err(e) => println!("  Internal Fan Duty: Error - {}", e),
            }
            match io.tach("INTF") {
                Ok(rpm) => println!("  Internal Fan RPM: {}", rpm),
                Err(e) => println!("  Internal Fan RPM: Error - {}", e),
            }
        }
        None => {
            println!("No Thelio Io devices found");
            println!("(VID: 0x1209, PID: 0x1776)");
        }
    }
}

fn run_loop(controller: &mut FanController, io: &mut Io, wrapper: &mut Child) -> io::Result<()> {
    let mut wrapper_in = wrapper.stdin.take().unwrap();
    let mut wrapper_out = BufReader::new(wrapper.stdout.take().unwrap());

    println!("Starting fan control loop (Ctrl+C to stop)");
    println!("Smoothing: 5s window | Ramp-up: 3s delay | Ramp-down: 10s delay\n");
    println!("{:>8} {:>8} {:>8} {:>8} {:>18}", "Instant", "Avg", "Target", "Actual", "State");
    println!("{}", "-".repeat(62));

    loop {
        // Request temperature from wrapper
        wrapper_in.write_all(b"\n")?;
        let mut line = String::new();
        wrapper_out.read_line(&mut line)?;

        let temp = line.trim().parse::<f64>().map_err(|err| {
            io::Error::new(io::ErrorKind::InvalidData, err)
        })?;

        let temp_hundredths = (temp * 100.0) as i16;
        let output = controller.update(temp_hundredths);

        // Only send commands to hardware when duty actually changes
        if output.duty_changed {
            for device in &["CPUF", "INTF"] {
                if let Err(e) = io.set_duty(device, output.actual_duty) {
                    eprintln!("Error setting {} duty: {}", device, e);
                }
            }
        }

        // Show status with change indicator
        let change_marker = if output.duty_changed { "*" } else { " " };
        println!("{:>7.1}C {:>7.1}C {:>7.1}% {:>7.1}%{} {:>16}",
            output.instant_temp as f32 / 100.0,
            output.smoothed_temp as f32 / 100.0,
            output.target_duty as f32 / 100.0,
            output.actual_duty as f32 / 100.0,
            change_marker,
            output.reason
        );

        sleep(Duration::new(1, 0));
    }
}

fn cmd_run(curve_name: &str) {
    let curve = match curve_name {
        "standard" => {
            println!("Using STANDARD fan curve");
            FanCurve::standard()
        }
        "quiet" => {
            println!("Using QUIET fan curve");
            FanCurve::quiet()
        }
        "threadripper2" => {
            println!("Using THREADRIPPER2 fan curve");
            FanCurve::threadripper2()
        }
        "hedt" => {
            println!("Using HEDT fan curve");
            FanCurve::hedt()
        }
        "xeon" => {
            println!("Using XEON fan curve");
            FanCurve::xeon()
        }
        _ => {
            eprintln!("Unknown curve '{}'. Available: standard, quiet, threadripper2, hedt, xeon", curve_name);
            return;
        }
    };

    // Create controller with smoothing and hysteresis
    let mut controller = FanController::new(curve)
        .with_smoothing_window(5)      // 5 second temperature averaging
        .with_ramp_up_delay(3.0)       // 3 second delay before speeding up
        .with_ramp_down_delay(10.0)    // 10 second delay before slowing down
        .with_min_duty_change(2_00);   // Ignore changes < 2%

    // Find Thelio Io
    let (name, port) = match find_thelio_io() {
        Some(x) => x,
        None => {
            eprintln!("No Thelio Io devices found");
            return;
        }
    };
    println!("Found Thelio Io at {}", name);

    let mut io = Io::new(port, 1000);

    // Reset device
    if let Err(e) = io.reset() {
        eprintln!("Warning: Reset failed: {}", e);
    }

    // Find and launch wrapper
    let bin_path = current_exe().expect("Failed to get exe path");
    let bin_dir = bin_path.parent().unwrap();
    let wrapper_path = bin_dir.join("thelio-io_wrapper.exe");

    if !wrapper_path.exists() {
        eprintln!("Wrapper not found at {:?}", wrapper_path);
        eprintln!("Make sure thelio-io_wrapper.exe is in the same directory");
        return;
    }

    println!("Launching temperature wrapper: {:?}", wrapper_path);

    let mut wrapper = match Command::new(&wrapper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to launch wrapper: {}", e);
            return;
        }
    };

    // Run the control loop
    if let Err(e) = run_loop(&mut controller, &mut io, &mut wrapper) {
        eprintln!("Error in control loop: {}", e);
    }

    let _ = wrapper.kill();
}

fn cmd_curves() {
    println!("Available fan curves:\n");

    println!("STANDARD (default for most Thelio models):");
    println!("  45C->30%  55C->35%  65C->40%  75C->50%  78C->60%  81C->70%  84C->80%  86C->90%  88C->100%");
    println!();

    println!("QUIET (lower noise, higher temps):");
    println!("  55C->25%  65C->30%  75C->40%  80C->50%  83C->60%  86C->70%  89C->80%  92C->90%  95C->100%");
    println!();

    println!("THREADRIPPER2:");
    println!("  0C->30%  40C->40%  47.5C->50%  55C->65%  62.5C->85%  66.25C->100%");
    println!();

    println!("HEDT:");
    println!("  0C->30%  50C->35%  60C->45%  70C->55%  74C->60%  76C->70%  78C->80%  81C->100%");
    println!();

    println!("XEON:");
    println!("  0C->40%  50C->40%  55C->45%  60C->50%  65C->55%  70C->60%  72C->65%  74C->80%  76C->85%  77C->90%  78C->100%");
}

fn cmd_temps() {
    // Find and launch wrapper in debug mode
    let bin_path = current_exe().expect("Failed to get exe path");
    let bin_dir = bin_path.parent().unwrap();
    let wrapper_path = bin_dir.join("thelio-io_wrapper.exe");

    if !wrapper_path.exists() {
        eprintln!("Wrapper not found at {:?}", wrapper_path);
        return;
    }

    let mut wrapper = match Command::new(&wrapper_path)
        .arg("--debug")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to launch wrapper: {}", e);
            return;
        }
    };

    let mut wrapper_in = wrapper.stdin.take().unwrap();
    let mut wrapper_out = BufReader::new(wrapper.stdout.take().unwrap());

    // Trigger a read
    let _ = wrapper_in.write_all(b"\n");

    // Read all output
    println!("Temperature Sensors:\n");
    loop {
        let mut line = String::new();
        match wrapper_out.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.starts_with("MAX:") {
                    println!("\n{}", trimmed);
                    break;
                } else {
                    println!("{}", trimmed);
                }
            }
            Err(_) => break,
        }
    }

    let _ = wrapper.kill();
}

fn main() {
    let args: Vec<String> = args().collect();

    if args.len() < 2 {
        println!("Thelio Io CLI - Fan Control Utility\n");
        println!("Usage: {} <command> [options]\n", args[0]);
        println!("Commands:");
        println!("  status              Show current fan status from Thelio Io board");
        println!("  temps               Show all CPU/GPU temperature sensors");
        println!("  run <curve>         Run fan control with specified curve");
        println!("  curves              List available fan curves");
        println!();
        println!("Examples:");
        println!("  {} status", args[0]);
        println!("  {} temps", args[0]);
        println!("  {} run quiet", args[0]);
        println!("  {} run standard", args[0]);
        return;
    }

    match args[1].as_str() {
        "status" => cmd_status(),
        "temps" => cmd_temps(),
        "run" => {
            let curve = args.get(2).map(|s| s.as_str()).unwrap_or("quiet");
            cmd_run(curve);
        }
        "curves" => cmd_curves(),
        other => {
            eprintln!("Unknown command: {}", other);
            eprintln!("Use 'status', 'temps', 'run', or 'curves'");
        }
    }
}
