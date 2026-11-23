use log::{debug, error, info};
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
    fan::{FanConfig, FanCurve, FanController},
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
use winreg::enums::*;
use winreg::RegKey;

/// Registry key for fan controller settings
const REGISTRY_KEY: &str = r"SOFTWARE\ThelioIo";

/// Read a DWORD value from registry, returning None if not found
fn read_registry_dword(name: &str) -> Option<u32> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey(REGISTRY_KEY).ok()?;
    key.get_value::<u32, _>(name).ok()
}

/// Logging callback for service mode
struct LogCallback {
    /// Track previous sustained load state to avoid spamming logs
    was_sustained: bool,
}

impl LogCallback {
    fn new() -> Self {
        Self {
            was_sustained: false,
        }
    }
}

impl daemon::DaemonCallback for LogCallback {
    fn on_update(&mut self, output: &thelio_io::fan::FanControllerOutput) {
        // Log fan duty changes
        if output.duty_changed {
            debug!(
                "Fan duty change: {:.1}C (avg {:.1}C) -> {:.1}% ({}) [CPU: {:.1}%, GPU: {:.1}%]",
                output.instant_temp as f32 / 100.0,
                output.smoothed_temp as f32 / 100.0,
                output.actual_duty as f32 / 100.0,
                output.reason,
                output.cpu_load,
                output.gpu_load,
            );
        }

        // Log sustained load state changes
        if output.sustained_load != self.was_sustained {
            if output.sustained_load {
                info!(
                    "Sustained load detected at {:.1}C, CPU {:.1}% - beginning gradual ramp",
                    output.smoothed_temp as f32 / 100.0,
                    output.cpu_load,
                );
            } else if self.was_sustained {
                info!("Sustained load ended - holding duty at {:.1}%",
                    output.actual_duty as f32 / 100.0);
            }
            self.was_sustained = output.sustained_load;
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

    // Validate this is a supported Thelio system
    let is_supported = matches!(
        (sys_vendor.as_str(), product_version.as_str()),
        ("System76", "thelio-mira-r1" | "thelio-mira-r2" | "thelio-mira-r3"
                   | "thelio-mira-b1" | "thelio-mira-b2" | "thelio-mira-b3" | "thelio-mira-b4"
                   | "thelio-major-r1" | "thelio-major-r2" | "thelio-major-r2.1"
                   | "thelio-major-b1" | "thelio-major-b2" | "thelio-major-b3"
                   | "thelio-mega-r1" | "thelio-mega-r1.1" | "thelio-massive-b1")
    );

    if !is_supported {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "unsupported sys_vendor '{}' and product_version '{}'",
                sys_vendor, product_version
            ),
        ));
    }

    debug!("{} {} detected", sys_vendor, product_version);

    // Build config from defaults, overriding with registry values where present.
    // Registry values use whole units (e.g., PboTemp=90 for 90°C).
    let mut config = FanConfig::default();
    if let Some(v) = read_registry_dword("PboTemp") { config.pbo_temp = (v as i16) * 100; }
    if let Some(v) = read_registry_dword("MaxFanDuty") { config.max_fan_duty = (v as u16) * 100; }
    if let Some(v) = read_registry_dword("SilenceThreshold") { config.silence_threshold = (v as u16) * 100; }
    if let Some(v) = read_registry_dword("SustainedLoadThreshold") { config.sustained_load_threshold = v as f32; }
    if let Some(v) = read_registry_dword("CpuPowerThreshold") { config.cpu_power_threshold = v as f32; }
    if let Some(v) = read_registry_dword("GpuTempThreshold") { config.gpu_temp_threshold = (v as i16) * 100; }
    if let Some(v) = read_registry_dword("CriticalTempOffset") { config.critical_temp_offset = (v as i16) * 100; }

    info!(
        "Fan settings: PBO={}°C, MaxFan={}%, Silence={}%, LoadThreshold={}%, PowerThreshold={}W, GpuTempThreshold={}°C, CriticalOffset=+{}°C",
        config.pbo_temp / 100, config.max_fan_duty / 100, config.silence_threshold / 100,
        config.sustained_load_threshold, config.cpu_power_threshold,
        config.gpu_temp_threshold / 100, config.critical_temp_offset / 100
    );

    // Create PBO-based fan curve from config
    let curve = FanCurve::pbo_curve(config.pbo_temp, config.max_fan_duty, config.silence_threshold);

    // Find Thelio Io devices
    let mut ios = daemon::find_thelio_io_devices()?;
    debug!("Found {} Thelio Io device(s)", ios.len());

    // Create fan controller from config
    let mut controller = FanController::from_config(curve, &config);

    debug!("Fan controller initialized with PBO-based curve");

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
