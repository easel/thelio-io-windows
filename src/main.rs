use log::{
    debug,
    error,
};
use std::{
    env::current_exe,
    ffi::OsString,
    io::{
        self,
        BufRead,
        BufReader,
        Write,
    },
    process::{
        Child,
        Command,
        Stdio,
        exit,
    },
    thread::sleep,
    time::Duration,
};
use thelio_io::{
    fan::{FanCurve, FanController},
    Io,
};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl,
        ServiceControlAccept,
        ServiceExitCode,
        ServiceState,
        ServiceStatus,
        ServiceType,
    },
    service_dispatcher,
    service_control_handler::{
        self,
        ServiceControlHandlerResult,
    },
};

fn driver_loop(controller: &mut FanController, ios: &mut [Io], wrapper: &mut Child) -> io::Result<()> {
    let mut wrapper_in = wrapper.stdin.take().unwrap();
    let mut wrapper_out = BufReader::new(wrapper.stdout.take().unwrap());

    loop {
        wrapper_in.write_all(b"\n")?;
        let mut line = String::new();
        wrapper_out.read_line(&mut line)?;

        let temp = line.trim().parse::<f64>().map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                err
            )
        })?;

        let temp_hundredths = (temp * 100.0) as i16;
        let output = controller.update(temp_hundredths);

        // Only send commands to hardware when duty actually changes
        if output.duty_changed {
            debug!(
                "Fan duty change: {:.1}C (avg {:.1}C) -> {:.1}% ({})",
                output.instant_temp as f32 / 100.0,
                output.smoothed_temp as f32 / 100.0,
                output.actual_duty as f32 / 100.0,
                output.reason
            );

            for io in ios.iter_mut() {
                for device in &["CPUF", "INTF"] {
                    io.set_duty(device, output.actual_duty).map_err(|err| {
                        io::Error::new(
                            io::ErrorKind::Other,
                            err
                        )
                    })?;
                }
            }
        }

        sleep(Duration::new(1, 0));
    }
}

fn driver() -> io::Result<()> {
    let smbios = smbioslib::table_load_from_device()?;

    let sys_vendor = smbios.find_map(
        |sys: smbioslib::SMBiosSystemInformation| sys.manufacturer()
    ).unwrap_or(String::new());

    let product_version = smbios.find_map(
        |sys: smbioslib::SMBiosSystemInformation| sys.version()
    ).unwrap_or(String::new());

    let curve = match (sys_vendor.as_str(), product_version.as_str()) {
        ("System76", "thelio-mira-r1" | "thelio-mira-r2" | "thelio-mira-r3"
                   | "thelio-mira-b1" | "thelio-mira-b2" | "thelio-mira-b3" | "thelio-mira-b4") => {
            debug!("{} {} uses standard fan curve", sys_vendor, product_version);
            FanCurve::standard()
        },
        ("System76", "thelio-major-r1") => {
            debug!("{} {} uses threadripper2 fan curve", sys_vendor, product_version);
            FanCurve::threadripper2()
        },
        ("System76", "thelio-major-r2" | "thelio-major-r2.1" | "thelio-major-b1" | "thelio-major-b2"
                   | "thelio-major-b3" | "thelio-mega-r1" | "thelio-mega-r1.1" ) => {
            debug!("{} {} uses hedt fan curve", sys_vendor, product_version);
            FanCurve::hedt()
        },
        ("System76", "thelio-massive-b1") => {
            debug!("{} {} uses xeon fan curve", sys_vendor, product_version);
            FanCurve::xeon()
        },
        _ => return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "unsupported sys_vendor '{}' and product_version '{}'",
                sys_vendor,
                product_version
            )
        )),
    };

    let mut ios = Vec::new();
    for port_info in serialport::available_ports()? {
        match port_info.port_type {
            serialport::SerialPortType::UsbPort(usb_info) => {
                if usb_info.vid == 0x1209 && usb_info.pid == 0x1776 {
                    debug!("Thelio Io at {}", port_info.port_name);

                    let port = serialport::new(port_info.port_name, 115200)
                        .timeout(Duration::from_millis(1))
                        .open()?;

                    let mut io = Io::new(port, 1000);
                    io.reset().map_err(|err| io::Error::new(
                        io::ErrorKind::Other,
                        err
                    ))?;
                    ios.push(io);
                }
            },
            _ => (),
        }
    }

    if ios.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "failed to find any Thelio Io devices"
        ));
    }

    // Create fan controller with smoothing and hysteresis
    // This prevents fan "panic" from short temperature spikes
    let mut controller = FanController::new(curve)
        .with_smoothing_window(5)      // 5 second temperature averaging
        .with_ramp_up_delay(3.0)       // 3 second delay before speeding up
        .with_ramp_down_delay(10.0)    // 10 second delay before slowing down
        .with_min_duty_change(2_00);   // Ignore changes < 2%

    debug!("Fan controller initialized with smoothing and hysteresis");

    let bin_path = current_exe()?;
    let bin_dir = bin_path.parent().unwrap();
    let wrapper_path = bin_dir.join("thelio-io_wrapper.exe");
    let mut wrapper = Command::new(&wrapper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let res = driver_loop(&mut controller, &mut ios, &mut wrapper);

    let _ = wrapper.kill();

    res
}

fn service_main(_args: Vec<OsString>) {
    // Windows event log
    winlog::init("System76 Thelio Io").expect("failed to initialize logging");

    // Handle service events
    let status_handle = service_control_handler::register("thelio-io", |event| -> ServiceControlHandlerResult {
        //TODO: handle stop event
        match event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    }).expect("failed to register for service events");

    // Update service status
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }).expect("failed to set service status");

    // Run driver
    if let Err(err) = driver() {
        error!("{}\n{:#?}", err, err);
        //TODO: set service status
        exit(1);
    }
}

define_windows_service!(ffi_service_main, service_main);

fn main() -> Result<(), windows_service::Error> {
    // Dispatch service
    service_dispatcher::start("thelio-io", ffi_service_main)?;
    Ok(())
}
