use std::env::args;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::thread::sleep;

use thelio_io::{
    Io,
    fan::{FanCurve, FanController},
    daemon::{self, ConsoleCallback, FAN_DEVICES},
};

fn cmd_status() {
    match daemon::find_thelio_io() {
        Some((name, port)) => {
            println!("Thelio Io at {}", name);

            let mut io = Io::new(port, 1000);

            if let Err(e) = io.reset() {
                println!("  Reset failed: {}", e);
            }
            sleep(Duration::from_millis(100));

            match io.revision() {
                Ok(rev) => println!("  Revision: {}", rev),
                Err(e) => println!("  Revision: Error - {}", e),
            }

            let fan_names = [
                ("CPUF", "CPU Fan"),
                ("INTF", "Intake Fan"),
                ("EXHF", "Exhaust Fan"),
            ];

            for (device, name) in &fan_names {
                match (io.duty(device), io.tach(device)) {
                    (Ok(duty), Ok(rpm)) => {
                        println!("  {} [{}]: {:.1}% duty, {} RPM", name, device, duty as f32 / 100.0, rpm);
                    }
                    (Ok(duty), Err(_)) => {
                        println!("  {} [{}]: {:.1}% duty, RPM unavailable", name, device, duty as f32 / 100.0);
                    }
                    (Err(_), _) => {}
                }
            }
        }
        None => {
            println!("No Thelio Io devices found");
            println!("(VID: 0x1209, PID: 0x1776)");
        }
    }
}

fn cmd_run(curve_name: &str) {
    // Default PBO settings (can be overridden by legacy curves)
    let pbo_temp = 85_00i16;
    let max_fan_duty = 100_00u16;
    let silence_threshold = 40_00u16;
    let sustained_load_threshold = 50.0f32;

    let curve = match curve_name {
        "pbo" => {
            println!("Using PBO-based fan curve (PBO=85°C, Max=100%, Silence=40%)");
            FanCurve::pbo_curve(pbo_temp, max_fan_duty, silence_threshold)
        }
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
            eprintln!("Unknown curve '{}'. Available: pbo, standard, quiet, threadripper2, hedt, xeon", curve_name);
            return;
        }
    };

    // Create controller with PBO-based settings
    let mut controller = FanController::new(curve)
        .with_smoothing_window(5)
        .with_ramp_down_delay(10.0)
        .with_min_duty_change(2_00)
        .with_pbo_temp(pbo_temp)
        .with_max_fan_duty(max_fan_duty)
        .with_silence_threshold(silence_threshold)
        .with_sustained_load_threshold(sustained_load_threshold);

    // Find Thelio Io devices
    let mut ios = match daemon::find_thelio_io_devices() {
        Ok(ios) => ios,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };
    println!("Found {} Thelio Io device(s)", ios.len());

    // Launch wrapper
    let mut wrapper = match daemon::launch_wrapper() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to launch wrapper: {}", e);
            return;
        }
    };

    // Run the control loop with console output
    let stop_flag = AtomicBool::new(false);
    let mut callback = ConsoleCallback::new();

    if let Err(e) = daemon::run_daemon(&mut controller, &mut ios, &mut wrapper, &stop_flag, &mut callback) {
        eprintln!("Error in control loop: {}", e);
    }

    let _ = wrapper.kill();
}

fn cmd_set(duty_str: &str) {
    let duty_percent: f32 = match duty_str.parse() {
        Ok(d) => d,
        Err(_) => {
            eprintln!("Invalid duty cycle: '{}'. Use a number 0-100.", duty_str);
            return;
        }
    };

    if !(0.0..=100.0).contains(&duty_percent) {
        eprintln!("Duty cycle must be between 0 and 100");
        return;
    }

    let duty_hundredths = (duty_percent * 100.0) as u16;

    let (_name, port) = match daemon::find_thelio_io() {
        Some(x) => x,
        None => {
            eprintln!("No Thelio Io devices found");
            return;
        }
    };

    let mut io = Io::new(port, 1000);
    let _ = io.reset();
    sleep(Duration::from_millis(100));

    println!("Setting all fans to {:.1}%...", duty_percent);

    for device in FAN_DEVICES {
        if io.set_duty(device, duty_hundredths).is_ok() {
            println!("  {} set to {:.1}%", device, duty_percent);
        }
    }

    println!("\nDone. Run 'status' to verify.");
    println!("WARNING: Fans will revert to 100% in ~10s unless controlled!");
}

fn cmd_temps() {
    let mut wrapper = match daemon::launch_wrapper() {
        Ok(mut w) => {
            // Relaunch with --debug flag
            let _ = w.kill();
            match Command::new(std::env::current_exe().unwrap().parent().unwrap().join("thelio-io_wrapper.exe"))
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
            }
        }
        Err(e) => {
            eprintln!("Failed to launch wrapper: {}", e);
            return;
        }
    };

    let mut wrapper_in = wrapper.stdin.take().unwrap();
    let mut wrapper_out = BufReader::new(wrapper.stdout.take().unwrap());

    let _ = wrapper_in.write_all(b"\n");

    println!("Temperature Sensors:\n");
    loop {
        let mut line = String::new();
        match wrapper_out.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                println!("{}", trimmed);
                if trimmed.starts_with("REPORTED:") {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let _ = wrapper.kill();
}

fn cmd_curves() {
    println!("Available fan curves:\n");
    println!("STANDARD (default) - optimized for PBO @ 85C:");
    println!("  40C->0%  60C->40%  75C->60%  80C->80%  85C->100%");
    println!();
    println!("QUIET:");
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

fn main() {
    let args: Vec<String> = args().collect();

    if args.len() < 2 {
        println!("Thelio Io CLI - Fan Control Utility\n");
        println!("Usage: {} <command> [options]\n", args[0]);
        println!("Commands:");
        println!("  status              Show current fan status");
        println!("  temps               Show CPU/GPU temperature sensors");
        println!("  run <curve>         Run fan control (standard, quiet, etc.)");
        println!("  set <percent>       Manually set all fans (0-100)");
        println!("  curves              List available fan curves");
        println!();
        println!("Examples:");
        println!("  {} status", args[0]);
        println!("  {} run standard", args[0]);
        println!("  {} set 40", args[0]);
        return;
    }

    match args[1].as_str() {
        "status" => cmd_status(),
        "temps" => cmd_temps(),
        "run" => {
            let curve = args.get(2).map(|s| s.as_str()).unwrap_or("standard");
            cmd_run(curve);
        }
        "set" => {
            let duty = args.get(2).map(|s| s.as_str()).unwrap_or("50");
            cmd_set(duty);
        }
        "curves" => cmd_curves(),
        other => {
            eprintln!("Unknown command: {}", other);
        }
    }
}
