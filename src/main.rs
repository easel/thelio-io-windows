use log::{debug, error, info, warn};
use std::{
    ffi::OsString,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use thelio_io::{
    daemon,
    fan::{FanCurve, FanController, ThrottleStatus},
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
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

/// Logging callback for service mode
struct LogCallback {
    /// Track previous throttle status to avoid spamming logs
    last_throttle_status: ThrottleStatus,
}

impl LogCallback {
    fn new() -> Self {
        Self {
            last_throttle_status: ThrottleStatus::None,
        }
    }
}

impl daemon::DaemonCallback for LogCallback {
    fn on_update(&mut self, output: &thelio_io::fan::FanControllerOutput) {
        // Log fan duty changes
        if output.duty_changed {
            debug!(
                "Fan duty change: {:.1}C (avg {:.1}C) -> {:.1}% ({}) [CPU: {}/{}MHz {:.1}%, GPU: {}/{}MHz {:.1}%]",
                output.instant_temp as f32 / 100.0,
                output.smoothed_temp as f32 / 100.0,
                output.actual_duty as f32 / 100.0,
                output.reason,
                output.cpu_clock,
                output.cpu_max_clock,
                output.cpu_load,
                output.gpu_clock,
                output.gpu_max_clock,
                output.gpu_load,
            );
        }

        // Log throttle status changes
        if output.throttle_status != self.last_throttle_status {
            match output.throttle_status {
                ThrottleStatus::None => {
                    if self.last_throttle_status.is_throttling() {
                        info!("Thermal throttling cleared - clocks restored");
                    }
                }
                ThrottleStatus::Likely => {
                    let cpu_pct = if output.cpu_max_clock > 0 {
                        (output.cpu_clock as f32 / output.cpu_max_clock as f32) * 100.0
                    } else { 100.0 };
                    let gpu_pct = if output.gpu_max_clock > 0 {
                        (output.gpu_clock as f32 / output.gpu_max_clock as f32) * 100.0
                    } else { 100.0 };
                    warn!(
                        "Possible thermal throttling: {:.1}C, CPU {}/{}MHz ({:.0}%) load {:.1}%, GPU {}/{}MHz ({:.0}%) load {:.1}% - boosting curve +{}C",
                        output.instant_temp as f32 / 100.0,
                        output.cpu_clock, output.cpu_max_clock, cpu_pct, output.cpu_load,
                        output.gpu_clock, output.gpu_max_clock, gpu_pct, output.gpu_load,
                        output.throttle_boost / 100,
                    );
                }
                ThrottleStatus::Confirmed => {
                    let cpu_pct = if output.cpu_max_clock > 0 {
                        (output.cpu_clock as f32 / output.cpu_max_clock as f32) * 100.0
                    } else { 100.0 };
                    let gpu_pct = if output.gpu_max_clock > 0 {
                        (output.gpu_clock as f32 / output.gpu_max_clock as f32) * 100.0
                    } else { 100.0 };
                    warn!(
                        "THROTTLING CONFIRMED: {:.1}C, CPU {}/{}MHz ({:.0}%) load {:.1}%, GPU {}/{}MHz ({:.0}%) load {:.1}% - boosting curve +{}C",
                        output.instant_temp as f32 / 100.0,
                        output.cpu_clock, output.cpu_max_clock, cpu_pct, output.cpu_load,
                        output.gpu_clock, output.gpu_max_clock, gpu_pct, output.gpu_load,
                        output.throttle_boost / 100,
                    );
                }
            }
            self.last_throttle_status = output.throttle_status;
        }
    }
}

fn driver(stop_flag: Arc<AtomicBool>) -> io::Result<()> {
    let smbios = smbioslib::table_load_from_device()?;

    let sys_vendor = smbios
        .find_map(|sys: smbioslib::SMBiosSystemInformation| sys.manufacturer())
        .unwrap_or_default();

    let product_version = smbios
        .find_map(|sys: smbioslib::SMBiosSystemInformation| sys.version())
        .unwrap_or_default();

    let curve = match (sys_vendor.as_str(), product_version.as_str()) {
        ("System76", "thelio-mira-r1" | "thelio-mira-r2" | "thelio-mira-r3"
                   | "thelio-mira-b1" | "thelio-mira-b2" | "thelio-mira-b3" | "thelio-mira-b4") => {
            debug!("{} {} uses standard fan curve", sys_vendor, product_version);
            FanCurve::standard()
        }
        ("System76", "thelio-major-r1") => {
            debug!("{} {} uses threadripper2 fan curve", sys_vendor, product_version);
            FanCurve::threadripper2()
        }
        ("System76", "thelio-major-r2" | "thelio-major-r2.1" | "thelio-major-b1" | "thelio-major-b2"
                   | "thelio-major-b3" | "thelio-mega-r1" | "thelio-mega-r1.1") => {
            debug!("{} {} uses hedt fan curve", sys_vendor, product_version);
            FanCurve::hedt()
        }
        ("System76", "thelio-massive-b1") => {
            debug!("{} {} uses xeon fan curve", sys_vendor, product_version);
            FanCurve::xeon()
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "unsupported sys_vendor '{}' and product_version '{}'",
                    sys_vendor, product_version
                ),
            ))
        }
    };

    // Find Thelio Io devices
    let mut ios = daemon::find_thelio_io_devices()?;
    debug!("Found {} Thelio Io device(s)", ios.len());

    // Create fan controller with smoothing and hysteresis
    let mut controller = FanController::new(curve)
        .with_smoothing_window(5)
        .with_ramp_up_delay(1.5)  // Faster response (was 3.0)
        .with_ramp_down_delay(10.0)
        .with_min_duty_change(2_00);

    debug!("Fan controller initialized with smoothing and hysteresis");

    // Launch wrapper
    let mut wrapper = daemon::launch_wrapper()?;

    // Run the daemon loop
    let mut callback = LogCallback::new();
    let res = daemon::run_daemon(&mut controller, &mut ios, &mut wrapper, &stop_flag, &mut callback);

    let _ = wrapper.kill();

    if res.is_ok() {
        info!("Stop signal received, shutting down");
    }

    res
}

fn service_main(_args: Vec<OsString>) {
    winlog::init("System76 Thelio Io").expect("failed to initialize logging");

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_clone = Arc::clone(&stop_flag);

    let status_handle = service_control_handler::register("thelio-io", move |event| -> ServiceControlHandlerResult {
        match event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop => {
                info!("Service stop requested");
                stop_flag_clone.store(true, Ordering::Relaxed);
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })
    .expect("failed to register for service events");

    status_handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .expect("failed to set service status");

    let exit_code = match driver(stop_flag) {
        Ok(()) => {
            info!("Service stopped cleanly");
            ServiceExitCode::Win32(0)
        }
        Err(err) => {
            error!("{}\n{:#?}", err, err);
            ServiceExitCode::Win32(1)
        }
    };

    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });
}

define_windows_service!(ffi_service_main, service_main);

fn main() -> Result<(), windows_service::Error> {
    service_dispatcher::start("thelio-io", ffi_service_main)?;
    Ok(())
}
