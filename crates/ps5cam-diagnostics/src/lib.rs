use ps5cam_usb::{probe, DeviceMode, ProbeReport, ProbeStatus};
use serde::Serialize;
#[cfg(windows)]
use std::fs;
use std::{
    collections::BTreeSet,
    env,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_WINDOWS_SERVICE_NAME: &str = "PS5CameraService";
pub const DEFAULT_PROBE_TIMEOUT_MS: u64 = 1_000;
pub const MIN_PROBE_TIMEOUT_MS: u64 = 1;
pub const MAX_PROBE_TIMEOUT_MS: u64 = 30_000;
const USBPCAP_SERVICE_NAME: &str = "USBPcap";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TimeoutValidationError {
    pub code: &'static str,
    pub provided_ms: u64,
    pub minimum_ms: u64,
    pub maximum_ms: u64,
}

impl std::fmt::Display for TimeoutValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "timeout_ms must be between {} and {} milliseconds, got {}",
            self.minimum_ms, self.maximum_ms, self.provided_ms
        )
    }
}

impl std::error::Error for TimeoutValidationError {}

pub fn validate_probe_timeout_ms(timeout_ms: u64) -> Result<Duration, TimeoutValidationError> {
    if !(MIN_PROBE_TIMEOUT_MS..=MAX_PROBE_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(TimeoutValidationError {
            code: "invalid_timeout_ms",
            provided_ms: timeout_ms,
            minimum_ms: MIN_PROBE_TIMEOUT_MS,
            maximum_ms: MAX_PROBE_TIMEOUT_MS,
        });
    }
    Ok(Duration::from_millis(timeout_ms))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Ready,
    Blocked,
    Unavailable,
    NotApplicable,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Gate {
    pub state: GateState,
    pub blockers: Vec<String>,
}

impl Gate {
    pub fn ready() -> Self {
        Self {
            state: GateState::Ready,
            blockers: Vec::new(),
        }
    }

    pub fn blocked(message: impl Into<String>) -> Self {
        Self {
            state: GateState::Blocked,
            blockers: vec![message.into()],
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            state: GateState::Unavailable,
            blockers: vec![message.into()],
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state == GateState::Ready
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlatformObservation {
    pub os: String,
    pub architecture: String,
    pub windows_checks_supported: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticIssue {
    pub component: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CameraDeviceObservation {
    pub mode: String,
    pub vendor_id: String,
    pub product_id: String,
    pub speed: String,
    pub usb_version: String,
    pub controller_id: String,
    pub bus_number: u8,
    pub device_address: u8,
    pub port_path: Vec<u8>,
    pub windows_instance_id: Option<String>,
    pub devnode_status: Option<u32>,
    pub problem_code: Option<u32>,
    pub libusb_open_succeeded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CameraReadiness {
    pub query_succeeded: bool,
    pub probe_status: String,
    pub devices: Vec<CameraDeviceObservation>,
    pub superspeed: Gate,
    pub boot_binding: Gate,
    pub issues: Vec<DiagnosticIssue>,
}

pub fn evaluate_probe(report: &ProbeReport) -> CameraReadiness {
    let devices = report
        .devices
        .iter()
        .map(|device| {
            let pnp = device.windows_pnp.as_ref();
            let open_failed = device
                .issues
                .iter()
                .any(|issue| issue.operation == "open_device");
            CameraDeviceObservation {
                mode: match device.mode {
                    DeviceMode::Boot => "boot".to_owned(),
                    DeviceMode::Camera => "camera".to_owned(),
                },
                vendor_id: format!("{:04x}", device.descriptor.vendor_id),
                product_id: format!("{:04x}", device.descriptor.product_id),
                speed: device.locator.speed.to_ascii_lowercase(),
                usb_version: device.descriptor.usb_version.clone(),
                controller_id: device.locator.controller_id.clone(),
                bus_number: device.locator.bus_number,
                device_address: device.locator.device_address,
                port_path: device.locator.port_path.clone(),
                windows_instance_id: pnp.map(|value| value.instance_id.clone()),
                devnode_status: pnp.and_then(|value| value.status),
                problem_code: pnp.and_then(|value| value.problem_code),
                libusb_open_succeeded: device.locator.controller_id != "windows-pnp"
                    && !open_failed,
            }
        })
        .collect::<Vec<_>>();

    let superspeed = match devices.as_slice() {
        [] => Gate::blocked("camera PS5 ausente; velocidade USB nao observada"),
        [device] if matches!(device.speed.as_str(), "super" | "superplus") => Gate::ready(),
        [device] if matches!(device.speed.as_str(), "low" | "full" | "high") => {
            Gate::blocked(format!(
                "camera conectada em velocidade '{}' abaixo de SuperSpeed",
                device.speed
            ))
        }
        [device] => Gate::blocked(format!(
            "velocidade '{}' nao comprova SuperSpeed; acesso libusb e necessario",
            device.speed
        )),
        _ => Gate::blocked(format!(
            "{} cameras suportadas detectadas; conecte exatamente uma",
            devices.len()
        )),
    };

    let boot_binding = match devices.as_slice() {
        [] => Gate::blocked("camera PS5 ausente; binding WinUSB nao pode ser avaliado"),
        [device] if device.mode == "camera" => Gate {
            state: GateState::NotApplicable,
            blockers: Vec::new(),
        },
        [device] if device.problem_code == Some(28) => {
            Gate::blocked("Problem Code 28: modo boot sem function driver/WinUSB associado")
        }
        [device] if device.problem_code.is_some_and(|code| code != 0) => Gate::blocked(format!(
            "devnode do modo boot reporta Problem Code {}",
            device.problem_code.unwrap_or_default()
        )),
        [device] if device.libusb_open_succeeded => Gate::ready(),
        [_] => {
            Gate::blocked("modo boot presente, mas o probe nao conseguiu abri-lo por libusb/WinUSB")
        }
        _ => Gate::blocked("binding ambiguo com multiplas cameras suportadas"),
    };

    let mut issues = report
        .issues
        .iter()
        .map(|issue| DiagnosticIssue {
            component: issue.operation.clone(),
            message: issue.message.clone(),
        })
        .collect::<Vec<_>>();
    for device in &report.devices {
        issues.extend(device.issues.iter().map(|issue| DiagnosticIssue {
            component: issue.operation.clone(),
            message: issue.message.clone(),
        }));
    }

    CameraReadiness {
        query_succeeded: true,
        probe_status: match report.status {
            ProbeStatus::Absent => "absent",
            ProbeStatus::Boot => "boot",
            ProbeStatus::Camera => "camera",
            ProbeStatus::Mixed => "mixed",
        }
        .to_owned(),
        devices,
        superspeed,
        boot_binding,
        issues,
    }
}

fn failed_camera_query(message: impl Into<String>) -> CameraReadiness {
    let message = message.into();
    CameraReadiness {
        query_succeeded: false,
        probe_status: "unavailable".to_owned(),
        devices: Vec::new(),
        superspeed: Gate::unavailable(message.clone()),
        boot_binding: Gate::unavailable(message.clone()),
        issues: vec![DiagnosticIssue {
            component: "probe".to_owned(),
            message,
        }],
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolObservation {
    pub name: String,
    pub found: bool,
    pub path: Option<String>,
}

impl ToolObservation {
    pub fn missing(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            found: false,
            path: None,
        }
    }

    pub fn found(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            found: true,
            path: Some(path.into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Running,
    Stopped,
    StartPending,
    StopPending,
    ContinuePending,
    PausePending,
    Paused,
    NotInstalled,
    Unknown,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServiceObservation {
    pub name: String,
    pub query_succeeded: bool,
    pub installed: bool,
    pub state: ServiceState,
    pub process_id: Option<u32>,
    pub error_code: Option<u32>,
}

impl ServiceObservation {
    pub fn unavailable(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            query_succeeded: false,
            installed: false,
            state: ServiceState::Unavailable,
            process_id: None,
            error_code: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UsbPcapReadiness {
    pub gate: Gate,
    pub driver: ServiceObservation,
    pub tools: Vec<ToolObservation>,
}

pub fn evaluate_usbpcap(
    driver: ServiceObservation,
    tools: Vec<ToolObservation>,
) -> UsbPcapReadiness {
    let missing = missing_required_tools(&tools, &["USBPcapCMD.exe", "tshark.exe", "dumpcap.exe"]);
    let gate = if !driver.query_succeeded {
        Gate::unavailable("status do driver USBPcap nao pode ser consultado")
    } else if !driver.installed {
        Gate::blocked("driver USBPcap nao esta instalado")
    } else if !missing.is_empty() {
        Gate::blocked(format!(
            "ferramentas USBPcap ausentes: {}",
            missing.join(", ")
        ))
    } else {
        Gate::ready()
    };
    UsbPcapReadiness {
        gate,
        driver,
        tools,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WdkReadiness {
    pub gate: Gate,
    pub roots_checked: Vec<String>,
    pub tools: Vec<ToolObservation>,
}

pub fn evaluate_wdk(roots_checked: Vec<String>, tools: Vec<ToolObservation>) -> WdkReadiness {
    let missing = missing_required_tools(&tools, &["signtool.exe", "infverif.exe", "inf2cat.exe"]);
    let gate = if missing.is_empty() {
        Gate::ready()
    } else {
        Gate::blocked(format!("ferramentas WDK ausentes: {}", missing.join(", ")))
    };
    WdkReadiness {
        gate,
        roots_checked,
        tools,
    }
}

fn missing_required_tools(tools: &[ToolObservation], required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter(|required_name| {
            !tools
                .iter()
                .any(|tool| tool.found && tool.name.eq_ignore_ascii_case(required_name))
        })
        .map(|name| (*name).to_owned())
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReadinessSummary {
    pub development_ready: bool,
    pub package_build_ready: bool,
    pub installed_runtime_ready: bool,
    pub blockers: Vec<String>,
}

pub fn summarize(
    platform: &PlatformObservation,
    camera: &CameraReadiness,
    usbpcap: &UsbPcapReadiness,
    wdk: &WdkReadiness,
    service: &ServiceObservation,
) -> ReadinessSummary {
    let binding_ready =
        camera.boot_binding.is_ready() || camera.boot_binding.state == GateState::NotApplicable;
    let development_ready = platform.windows_checks_supported
        && camera.superspeed.is_ready()
        && binding_ready
        && usbpcap.gate.is_ready();
    let package_build_ready = platform.windows_checks_supported && wdk.gate.is_ready();
    let installed_runtime_ready = platform.windows_checks_supported
        && camera.superspeed.is_ready()
        && binding_ready
        && service.state == ServiceState::Running;

    let mut blockers = BTreeSet::new();
    if !platform.windows_checks_supported {
        blockers.insert("diagnosticos de instalacao exigem Windows".to_owned());
    }
    for gate in [
        &camera.superspeed,
        &camera.boot_binding,
        &usbpcap.gate,
        &wdk.gate,
    ] {
        blockers.extend(gate.blockers.iter().cloned());
    }
    if service.state != ServiceState::Running {
        blockers.insert(format!(
            "servico {} nao esta em execucao ({:?})",
            service.name, service.state
        ));
    }

    ReadinessSummary {
        development_ready,
        package_build_ready,
        installed_runtime_ready,
        blockers: blockers.into_iter().collect(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticReport {
    pub schema_version: u32,
    pub captured_at_unix_ms: u64,
    pub read_only: bool,
    pub platform: PlatformObservation,
    pub camera: CameraReadiness,
    pub usbpcap: UsbPcapReadiness,
    pub wdk: WdkReadiness,
    pub service: ServiceObservation,
    pub summary: ReadinessSummary,
}

pub fn collect(timeout: Duration, service_name: &str) -> DiagnosticReport {
    let platform = PlatformObservation {
        os: env::consts::OS.to_owned(),
        architecture: env::consts::ARCH.to_owned(),
        windows_checks_supported: cfg!(windows),
    };
    let camera = match probe(timeout) {
        Ok(snapshot) => evaluate_probe(&snapshot.report),
        Err(error) => failed_camera_query(error.to_string()),
    };
    let (usbpcap, wdk, service) = collect_platform_components(service_name);
    let summary = summarize(&platform, &camera, &usbpcap, &wdk, &service);
    DiagnosticReport {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION,
        captured_at_unix_ms: now_unix_ms(),
        read_only: true,
        platform,
        camera,
        usbpcap,
        wdk,
        service,
        summary,
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn observe_tool(name: &str, candidates: impl IntoIterator<Item = PathBuf>) -> ToolObservation {
    candidates
        .into_iter()
        .chain(find_in_path(name))
        .find(|candidate| candidate.is_file())
        .map(|path| ToolObservation::found(name, path.to_string_lossy()))
        .unwrap_or_else(|| ToolObservation::missing(name))
}

#[cfg(not(windows))]
fn collect_platform_components(
    service_name: &str,
) -> (UsbPcapReadiness, WdkReadiness, ServiceObservation) {
    let unsupported = Gate::unavailable("USBPcap esta disponivel somente no Windows");
    let driver = ServiceObservation::unavailable(USBPCAP_SERVICE_NAME);
    let usbpcap = UsbPcapReadiness {
        gate: unsupported,
        driver,
        tools: vec![
            ToolObservation::missing("USBPcapCMD.exe"),
            ToolObservation::missing("tshark.exe"),
            ToolObservation::missing("dumpcap.exe"),
        ],
    };
    let wdk = WdkReadiness {
        gate: Gate::unavailable("WDK esta disponivel somente no Windows"),
        roots_checked: Vec::new(),
        tools: vec![
            ToolObservation::missing("signtool.exe"),
            ToolObservation::missing("infverif.exe"),
            ToolObservation::missing("inf2cat.exe"),
        ],
    };
    (usbpcap, wdk, ServiceObservation::unavailable(service_name))
}

#[cfg(windows)]
fn collect_platform_components(
    service_name: &str,
) -> (UsbPcapReadiness, WdkReadiness, ServiceObservation) {
    let program_files = env::var_os("ProgramFiles").map(PathBuf::from);
    let program_files_x86 = env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    let usbpcap_cmd = observe_tool(
        "USBPcapCMD.exe",
        program_files.iter().flat_map(|root| {
            [
                root.join("Wireshark/extcap/USBPcapCMD.exe"),
                root.join("USBPcap/USBPcapCMD.exe"),
            ]
        }),
    );
    let tshark = observe_tool(
        "tshark.exe",
        program_files
            .iter()
            .map(|root| root.join("Wireshark/tshark.exe")),
    );
    let dumpcap = observe_tool(
        "dumpcap.exe",
        program_files
            .iter()
            .map(|root| root.join("Wireshark/dumpcap.exe")),
    );
    let usbpcap_driver = query_service(USBPCAP_SERVICE_NAME);
    let usbpcap = evaluate_usbpcap(usbpcap_driver, vec![usbpcap_cmd, tshark, dumpcap]);

    let mut roots = Vec::new();
    if let Some(root) = env::var_os("WindowsSdkDir").map(PathBuf::from) {
        roots.push(root);
    }
    if let Some(root) = program_files_x86 {
        roots.push(root.join("Windows Kits/10"));
    }
    roots.sort();
    roots.dedup();
    let wdk_tools = ["signtool.exe", "infverif.exe", "inf2cat.exe"]
        .into_iter()
        .map(|name| observe_tool(name, wdk_candidates(&roots, name)))
        .collect::<Vec<_>>();
    let roots_checked = roots
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect();
    let wdk = evaluate_wdk(roots_checked, wdk_tools);
    let service = query_service(service_name);
    (usbpcap, wdk, service)
}

#[cfg(windows)]
fn wdk_candidates(roots: &[PathBuf], tool: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for root in roots {
        let bin = root.join("bin");
        candidates.push(bin.join(tool));
        let Ok(entries) = fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let version = entry.path();
            if !version.is_dir() {
                continue;
            }
            candidates.push(version.join("x64").join(tool));
            candidates.push(version.join("x86").join(tool));
        }
    }
    candidates
}

#[cfg(windows)]
fn query_service(name: &str) -> ServiceObservation {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_SERVICE_DOES_NOT_EXIST},
        System::Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
            SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTINUE_PENDING, SERVICE_PAUSED,
            SERVICE_PAUSE_PENDING, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START_PENDING,
            SERVICE_STATUS_PROCESS, SERVICE_STOPPED, SERVICE_STOP_PENDING,
        },
    };

    let wide = name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: all handles are query-only, strings are NUL terminated, output
    // buffers have their exact declared size, and every opened handle is closed.
    unsafe {
        let manager = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return ServiceObservation {
                name: name.to_owned(),
                query_succeeded: false,
                installed: false,
                state: ServiceState::Unavailable,
                process_id: None,
                error_code: Some(GetLastError()),
            };
        }
        let service = OpenServiceW(manager, wide.as_ptr(), SERVICE_QUERY_STATUS);
        if service.is_null() {
            let error = GetLastError();
            CloseServiceHandle(manager);
            return ServiceObservation {
                name: name.to_owned(),
                query_succeeded: error == ERROR_SERVICE_DOES_NOT_EXIST,
                installed: false,
                state: if error == ERROR_SERVICE_DOES_NOT_EXIST {
                    ServiceState::NotInstalled
                } else {
                    ServiceState::Unavailable
                },
                process_id: None,
                error_code: Some(error),
            };
        }
        let mut status = SERVICE_STATUS_PROCESS::default();
        let mut bytes_needed = 0;
        let ok = QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut bytes_needed,
        );
        let error = if ok == 0 { Some(GetLastError()) } else { None };
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
        if let Some(error) = error {
            return ServiceObservation {
                name: name.to_owned(),
                query_succeeded: false,
                installed: true,
                state: ServiceState::Unavailable,
                process_id: None,
                error_code: Some(error),
            };
        }
        let state = match status.dwCurrentState {
            SERVICE_RUNNING => ServiceState::Running,
            SERVICE_STOPPED => ServiceState::Stopped,
            SERVICE_START_PENDING => ServiceState::StartPending,
            SERVICE_STOP_PENDING => ServiceState::StopPending,
            SERVICE_CONTINUE_PENDING => ServiceState::ContinuePending,
            SERVICE_PAUSE_PENDING => ServiceState::PausePending,
            SERVICE_PAUSED => ServiceState::Paused,
            _ => ServiceState::Unknown,
        };
        ServiceObservation {
            name: name.to_owned(),
            query_succeeded: true,
            installed: true,
            state,
            process_id: (status.dwProcessId != 0).then_some(status.dwProcessId),
            error_code: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ProbeReport {
        serde_json::from_str(include_str!(
            "../../ps5cam-usb/tests/fixtures/boot-report.json"
        ))
        .expect("valid ps5cam-usb fixture")
    }

    #[test]
    fn superspeed_fixture_passes_speed_gate() {
        let readiness = evaluate_probe(&fixture());
        assert_eq!(readiness.probe_status, "boot");
        assert!(readiness.superspeed.is_ready());
        assert_eq!(readiness.devices[0].speed, "super");
    }

    #[test]
    fn usb2_high_speed_is_blocked() {
        let mut report = fixture();
        report.devices[0].locator.speed = "high".to_owned();
        let readiness = evaluate_probe(&report);
        assert_eq!(readiness.superspeed.state, GateState::Blocked);
        assert!(readiness.superspeed.blockers[0].contains("abaixo de SuperSpeed"));
    }

    #[test]
    fn problem_code_28_is_reported_as_missing_binding() {
        let mut report = fixture();
        report.devices[0].windows_pnp = Some(ps5cam_usb::WindowsPnpReport {
            instance_id: "USB\\VID_05A9&PID_0580\\fixture".to_owned(),
            status: Some(0x400),
            problem_code: Some(28),
        });
        let readiness = evaluate_probe(&report);
        assert_eq!(readiness.boot_binding.state, GateState::Blocked);
        assert!(readiness.boot_binding.blockers[0].contains("Problem Code 28"));
    }

    #[test]
    fn usbpcap_requires_driver_and_all_cli_tools() {
        let driver = ServiceObservation {
            name: USBPCAP_SERVICE_NAME.to_owned(),
            query_succeeded: true,
            installed: true,
            state: ServiceState::Running,
            process_id: None,
            error_code: None,
        };
        let ready = evaluate_usbpcap(
            driver.clone(),
            vec![
                ToolObservation::found("USBPcapCMD.exe", "fixture"),
                ToolObservation::found("tshark.exe", "fixture"),
                ToolObservation::found("dumpcap.exe", "fixture"),
            ],
        );
        assert!(ready.gate.is_ready());

        let blocked = evaluate_usbpcap(driver, vec![ToolObservation::missing("USBPcapCMD.exe")]);
        assert_eq!(blocked.gate.state, GateState::Blocked);
    }

    #[test]
    fn wdk_requires_all_packaging_tools() {
        let readiness = evaluate_wdk(
            vec!["fixture".to_owned()],
            vec![
                ToolObservation::found("signtool.exe", "fixture"),
                ToolObservation::missing("infverif.exe"),
                ToolObservation::found("inf2cat.exe", "fixture"),
            ],
        );
        assert_eq!(readiness.gate.state, GateState::Blocked);
        assert!(readiness.gate.blockers[0].contains("infverif.exe"));
    }

    #[test]
    fn empty_tool_observation_never_passes_tool_gates() {
        let driver = ServiceObservation {
            name: USBPCAP_SERVICE_NAME.to_owned(),
            query_succeeded: true,
            installed: true,
            state: ServiceState::Running,
            process_id: None,
            error_code: None,
        };
        assert_eq!(
            evaluate_usbpcap(driver, Vec::new()).gate.state,
            GateState::Blocked
        );
        assert_eq!(
            evaluate_wdk(Vec::new(), Vec::new()).gate.state,
            GateState::Blocked
        );
    }

    #[test]
    fn non_windows_platform_cannot_be_installation_ready() {
        let platform = PlatformObservation {
            os: "linux".to_owned(),
            architecture: "x86_64".to_owned(),
            windows_checks_supported: false,
        };
        let camera = evaluate_probe(&fixture());
        let unavailable_service = ServiceObservation::unavailable(DEFAULT_WINDOWS_SERVICE_NAME);
        let usbpcap = UsbPcapReadiness {
            gate: Gate::unavailable("unsupported"),
            driver: ServiceObservation::unavailable(USBPCAP_SERVICE_NAME),
            tools: Vec::new(),
        };
        let wdk = WdkReadiness {
            gate: Gate::unavailable("unsupported"),
            roots_checked: Vec::new(),
            tools: Vec::new(),
        };
        let summary = summarize(&platform, &camera, &usbpcap, &wdk, &unavailable_service);
        assert!(!summary.development_ready);
        assert!(!summary.package_build_ready);
        assert!(!summary.installed_runtime_ready);
    }

    #[test]
    fn report_schema_marks_collection_as_read_only() {
        let report = DiagnosticReport {
            schema_version: DIAGNOSTICS_SCHEMA_VERSION,
            captured_at_unix_ms: 1,
            read_only: true,
            platform: PlatformObservation {
                os: "windows".to_owned(),
                architecture: "x86_64".to_owned(),
                windows_checks_supported: true,
            },
            camera: evaluate_probe(&fixture()),
            usbpcap: UsbPcapReadiness {
                gate: Gate::ready(),
                driver: ServiceObservation {
                    name: USBPCAP_SERVICE_NAME.to_owned(),
                    query_succeeded: true,
                    installed: true,
                    state: ServiceState::Running,
                    process_id: None,
                    error_code: None,
                },
                tools: Vec::new(),
            },
            wdk: evaluate_wdk(Vec::new(), Vec::new()),
            service: ServiceObservation::unavailable(DEFAULT_WINDOWS_SERVICE_NAME),
            summary: ReadinessSummary {
                development_ready: false,
                package_build_ready: false,
                installed_runtime_ready: false,
                blockers: Vec::new(),
            },
        };
        let json = serde_json::to_string(&report).expect("serialize diagnostics");
        assert!(json.contains("\"read_only\":true"));
        assert!(json.contains("\"schema_version\":1"));
    }

    #[test]
    fn timeout_validation_rejects_zero_with_structured_bounds() {
        let error = validate_probe_timeout_ms(0).expect_err("zero must be rejected");
        assert_eq!(error.code, "invalid_timeout_ms");
        assert_eq!(error.provided_ms, 0);
        assert_eq!(error.minimum_ms, 1);
        assert_eq!(error.maximum_ms, MAX_PROBE_TIMEOUT_MS);
        assert_eq!(
            serde_json::to_value(error).expect("serialize timeout error"),
            serde_json::json!({
                "code": "invalid_timeout_ms",
                "provided_ms": 0,
                "minimum_ms": 1,
                "maximum_ms": 30_000
            })
        );
    }

    #[test]
    fn timeout_validation_accepts_the_safe_operational_limit() {
        assert_eq!(
            validate_probe_timeout_ms(MAX_PROBE_TIMEOUT_MS),
            Ok(Duration::from_millis(MAX_PROBE_TIMEOUT_MS))
        );
    }

    #[test]
    fn timeout_validation_rejects_extreme_values_without_duration_conversion() {
        let error = validate_probe_timeout_ms(u64::MAX).expect_err("extreme value must fail");
        assert_eq!(error.provided_ms, u64::MAX);
        assert_eq!(error.maximum_ms, MAX_PROBE_TIMEOUT_MS);
    }
}
