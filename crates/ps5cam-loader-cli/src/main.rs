use clap::{Args, Parser, Subcommand};
use ov580_loader::{
    wait_for_camera_mode, BootDeviceLocator, CancellationCheck, ExecuteDisposition, FirmwareImage,
    LoaderConfig, LoaderError, ObservedDeviceState, Ov580BootDevice, ReenumerationBackend,
    ReenumerationConfig, ReenumerationError, ReenumerationOutcome, UploadReport, CHUNK_SIZE,
};
use ps5cam_usb::DeviceMode;
use rusb::UsbContext;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};
use thiserror::Error;

const BOOT_DEVICE_CONFIRMATION: &str = r"USB\VID_05A9&PID_0580";
const MAX_EVIDENCE_LENGTH: usize = 1_024;
const MAX_PREFLIGHT_TIMEOUT_MS: u64 = 30_000;
const MAX_TRANSFER_TIMEOUT_MS: u64 = 30_000;
const MAX_UPLOAD_DEADLINE_MS: u64 = 300_000;
const MAX_REENUMERATION_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Parser)]
#[command(
    name = "ps5cam-loader",
    version,
    about = "Validate and explicitly upload an authorized OV580 firmware image"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate an image and print the transfer plan without accessing USB.
    Inspect(ImageArguments),
    /// Upload an image to one correlated SuperSpeed boot device and request execution.
    Upload(UploadArguments),
}

#[derive(Debug, Args)]
struct ImageArguments {
    /// Operator-supplied firmware path; firmware is never bundled.
    #[arg(value_name = "FIRMWARE")]
    firmware: PathBuf,

    /// Expected SHA-256 (64 hexadecimal characters).
    #[arg(long, value_name = "SHA256")]
    expected_sha256: String,

    /// Human-readable origin of the firmware (supplier, repository, or acquisition record).
    #[arg(long, value_name = "ORIGIN")]
    provenance: String,

    /// Ticket, license, approval, or other authorization record for local use.
    #[arg(long, value_name = "REFERENCE")]
    authorization_reference: String,
}

#[derive(Debug, Args)]
struct UploadArguments {
    #[command(flatten)]
    image: ImageArguments,

    /// Confirm that the operator is authorized to use this firmware image.
    #[arg(long, action = clap::ArgAction::SetTrue, required = true)]
    acknowledge_authorized_firmware: bool,

    /// Confirm the exact boot hardware ID; must be USB\VID_05A9&PID_0580.
    #[arg(long, value_name = "HARDWARE_ID")]
    confirm_device: String,

    /// Absolute timeout for the lightweight read-only preflight discovery.
    #[arg(long, default_value_t = 1000, value_name = "MILLISECONDS")]
    preflight_timeout_ms: u64,

    /// Timeout for each USB control transfer.
    #[arg(long, default_value_t = 1000, value_name = "MILLISECONDS")]
    transfer_timeout_ms: u64,

    /// Overall upload deadline, checked between control transfers.
    #[arg(long, default_value_t = 120000, value_name = "MILLISECONDS")]
    upload_deadline_ms: u64,

    /// Existing file requests cancellation before the next control transfer.
    #[arg(long, value_name = "PATH")]
    cancel_file: Option<PathBuf>,

    /// Maximum time to wait for the correlated PID 058C after execution.
    #[arg(long, default_value_t = 10000, value_name = "MILLISECONDS")]
    reenumeration_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct Evidence {
    provenance: String,
    authorization_reference: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct InspectionReport {
    schema_version: u8,
    status: &'static str,
    operation: &'static str,
    evidence: Evidence,
    firmware_bytes: usize,
    sha256: String,
    chunks: usize,
    chunk_size: usize,
    first_address: TransferAddress,
    last_address: TransferAddress,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TransferAddress {
    offset: u64,
    value: u16,
    index: u16,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct TargetLocation {
    controller_id: String,
    bus_number: u8,
    device_address: u8,
    port_path: Vec<u8>,
    speed: String,
    windows_instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveredDevice {
    mode: DeviceMode,
    target: TargetLocation,
    accessible: bool,
    access_error: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
struct UploadProgress {
    bytes_uploaded: u64,
    chunks_uploaded: u64,
}

#[derive(Debug, Serialize)]
struct UploadResult {
    schema_version: u8,
    status: &'static str,
    operation: &'static str,
    evidence: Evidence,
    firmware_bytes: usize,
    sha256: String,
    target: TargetLocation,
    progress: UploadProgress,
    execute: &'static str,
    reenumeration: &'static str,
    elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SuccessReport {
    Inspection(InspectionReport),
    Upload(UploadResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Arguments,
    Inspect,
    Preflight,
    Open,
    Upload,
    Execute,
    Release,
    Reenumeration,
    Complete,
}

impl Phase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Arguments => "arguments",
            Self::Inspect => "inspect",
            Self::Preflight => "preflight",
            Self::Open => "open",
            Self::Upload => "upload",
            Self::Execute => "execute",
            Self::Release => "release",
            Self::Reenumeration => "reenumeration",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug)]
struct AuditState {
    operation: &'static str,
    phase: Phase,
    evidence: Option<Evidence>,
    firmware_sha256: Option<String>,
    firmware_bytes: Option<usize>,
    target: Option<TargetLocation>,
    progress: UploadProgress,
    execute: Option<&'static str>,
    last_observed_state: Option<&'static str>,
    cleanup_error: Option<String>,
}

impl AuditState {
    fn from_cli(cli: &Cli) -> Self {
        let (operation, image) = match &cli.command {
            Command::Inspect(image) => ("inspect", image),
            Command::Upload(arguments) => ("upload", &arguments.image),
        };
        Self {
            operation,
            phase: Phase::Arguments,
            evidence: Some(Evidence {
                provenance: image.provenance.clone(),
                authorization_reference: image.authorization_reference.clone(),
            }),
            firmware_sha256: None,
            firmware_bytes: None,
            target: None,
            progress: UploadProgress::default(),
            execute: None,
            last_observed_state: None,
            cleanup_error: None,
        }
    }

    fn arguments() -> Self {
        Self {
            operation: "unknown",
            phase: Phase::Arguments,
            evidence: None,
            firmware_sha256: None,
            firmware_bytes: None,
            target: None,
            progress: UploadProgress::default(),
            execute: None,
            last_observed_state: None,
            cleanup_error: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct FailureEnvelope<'a> {
    schema_version: u8,
    status: &'static str,
    operation: &'a str,
    phase: &'static str,
    evidence: Option<&'a Evidence>,
    firmware_sha256: Option<&'a str>,
    firmware_bytes: Option<usize>,
    target: Option<&'a TargetLocation>,
    progress: UploadProgress,
    execute: Option<&'static str>,
    last_observed_state: Option<&'static str>,
    cleanup_error: Option<&'a str>,
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    kind: &'static str,
    message: String,
}

impl<'a> FailureEnvelope<'a> {
    fn new(audit: &'a AuditState, kind: &'static str, message: String) -> Self {
        Self {
            schema_version: 1,
            status: "error",
            operation: audit.operation,
            phase: audit.phase.as_str(),
            evidence: audit.evidence.as_ref(),
            firmware_sha256: audit.firmware_sha256.as_deref(),
            firmware_bytes: audit.firmware_bytes,
            target: audit.target.as_ref(),
            progress: audit.progress,
            execute: audit.execute,
            last_observed_state: audit.last_observed_state,
            cleanup_error: audit.cleanup_error.as_deref(),
            error: ErrorPayload { kind, message },
        }
    }
}

#[derive(Debug, Error)]
enum PreflightError {
    #[error(
        "expected exactly one supported boot device and no camera devices; found {boot} boot and {camera} camera devices"
    )]
    AmbiguousDevices { boot: usize, camera: usize },
    #[error("boot device is not accessible through libusb/WinUSB: {reason}")]
    Inaccessible {
        reason: String,
        target: Box<TargetLocation>,
    },
    #[error("boot device topology has no controller or port path")]
    MissingTopology { target: Box<TargetLocation> },
    #[error("boot device speed '{speed}' is not SuperSpeed")]
    NotSuperSpeed {
        speed: String,
        target: Box<TargetLocation>,
    },
}

impl PreflightError {
    fn target(&self) -> Option<&TargetLocation> {
        match self {
            Self::Inaccessible { target, .. }
            | Self::MissingTopology { target }
            | Self::NotSuperSpeed { target, .. } => Some(target.as_ref()),
            Self::AmbiguousDevices { .. } => None,
        }
    }
}

#[derive(Debug, Error)]
enum CorrelationError {
    #[error("lightweight USB discovery failed: {0}")]
    Discovery(String),
    #[error("multiple PID 058C camera devices detected during correlated re-enumeration: {count}")]
    MultipleCameras { count: usize },
    #[error(
        "PID 058C appeared at controller '{controller_id}' port {port_path:?}, not at the upload target"
    )]
    CameraTopologyMismatch {
        controller_id: String,
        port_path: Vec<u8>,
    },
}

#[derive(Debug, Error)]
enum DiscoveryError {
    #[error("USB discovery exceeded its absolute {limit_ms} ms deadline")]
    Deadline { limit_ms: u128 },
    #[error("failed to enumerate USB devices: {0}")]
    Enumeration(#[source] rusb::Error),
    #[error("failed to read a USB device descriptor: {0}")]
    Descriptor(#[source] rusb::Error),
    #[error("failed to read USB port topology: {0}")]
    PortPath(#[source] rusb::Error),
}

#[derive(Debug, Error)]
enum CliError {
    #[error("failed to read firmware {path}: {source}")]
    ReadFirmware { path: PathBuf, source: io::Error },
    #[error("expected SHA-256 must contain exactly 64 hexadecimal characters")]
    InvalidExpectedHash,
    #[error("firmware SHA-256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("invalid firmware image: {0}")]
    InvalidImage(#[from] ov580_loader::FirmwareImageError),
    #[error("{field} must be non-empty and at most {MAX_EVIDENCE_LENGTH} characters")]
    InvalidEvidence { field: &'static str },
    #[error("firmware authorization acknowledgement is required")]
    MissingAuthorization,
    #[error("hardware confirmation must be exactly {BOOT_DEVICE_CONFIRMATION}")]
    InvalidDeviceConfirmation,
    #[error("{name} must be between 1 and {maximum_ms} milliseconds")]
    InvalidTimeout { name: &'static str, maximum_ms: u64 },
    #[error("transfer timeout must not exceed the overall upload deadline")]
    TransferExceedsDeadline,
    #[error("read-only USB preflight failed: {0}")]
    Probe(String),
    #[error("preflight rejected upload: {0}")]
    Preflight(#[from] PreflightError),
    #[error("bootloader operation failed: {0}")]
    Loader(#[from] LoaderError),
    #[error("upload stopped by {reason} after {bytes_uploaded} bytes")]
    OperationalCancellation {
        reason: &'static str,
        bytes_uploaded: u64,
    },
    #[error("camera re-enumeration failed: {0}")]
    Reenumeration(String),
}

impl CliError {
    const fn kind(&self) -> &'static str {
        match self {
            Self::ReadFirmware { .. } => "read_firmware",
            Self::InvalidExpectedHash => "invalid_expected_hash",
            Self::HashMismatch { .. } => "hash_mismatch",
            Self::InvalidImage(_) => "invalid_image",
            Self::InvalidEvidence { .. } => "invalid_evidence",
            Self::MissingAuthorization => "missing_authorization",
            Self::InvalidDeviceConfirmation => "invalid_device_confirmation",
            Self::InvalidTimeout { .. } | Self::TransferExceedsDeadline => "invalid_timeout",
            Self::Probe(_) => "probe",
            Self::Preflight(_) => "preflight",
            Self::Loader(_) => "loader",
            Self::OperationalCancellation { .. } => "cancelled",
            Self::Reenumeration(_) => "reenumeration",
        }
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let audit = AuditState::arguments();
            let failure = FailureEnvelope::new(&audit, "arguments", error.to_string());
            let _ = write_json(io::stderr().lock(), &failure);
            return ExitCode::from(2);
        }
    };
    let mut audit = AuditState::from_cli(&cli);
    match run(cli, &mut audit) {
        Ok(report) => match write_json(io::stdout().lock(), &report) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let failure = FailureEnvelope::new(&audit, "serialize", error.to_string());
                let _ = write_json(io::stderr().lock(), &failure);
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            let failure = FailureEnvelope::new(&audit, error.kind(), error.to_string());
            let _ = write_json(io::stderr().lock(), &failure);
            ExitCode::FAILURE
        }
    }
}

fn write_json(mut writer: impl Write, value: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer_pretty(&mut writer, value).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())
}

fn run(cli: Cli, audit: &mut AuditState) -> Result<SuccessReport, CliError> {
    match cli.command {
        Command::Inspect(arguments) => {
            let (_, report) = inspect_image(&arguments, audit)?;
            audit.phase = Phase::Complete;
            Ok(SuccessReport::Inspection(report))
        }
        Command::Upload(arguments) => upload(arguments, audit).map(SuccessReport::Upload),
    }
}

fn inspect_image(
    arguments: &ImageArguments,
    audit: &mut AuditState,
) -> Result<(FirmwareImage, InspectionReport), CliError> {
    audit.phase = Phase::Inspect;
    let evidence = validate_evidence(arguments)?;
    audit.evidence = Some(evidence.clone());
    let expected = normalize_expected_hash(&arguments.expected_sha256)?;
    let bytes = fs::read(&arguments.firmware).map_err(|source| CliError::ReadFirmware {
        path: arguments.firmware.clone(),
        source,
    })?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    audit.firmware_bytes = Some(bytes.len());
    audit.firmware_sha256 = Some(actual.clone());
    if actual != expected {
        return Err(CliError::HashMismatch { expected, actual });
    }

    let image = FirmwareImage::from_bytes(bytes)?;
    let first = image.chunks().next().expect("validated image is non-empty");
    let last = image.chunks().last().expect("validated image is non-empty");
    let report = InspectionReport {
        schema_version: 1,
        status: "ok",
        operation: "inspect",
        evidence,
        firmware_bytes: image.len(),
        sha256: actual,
        chunks: image.chunks().len(),
        chunk_size: CHUNK_SIZE,
        first_address: address_report(first),
        last_address: address_report(last),
    };
    Ok((image, report))
}

fn validate_evidence(arguments: &ImageArguments) -> Result<Evidence, CliError> {
    fn field(value: &str, name: &'static str) -> Result<String, CliError> {
        let value = value.trim();
        if value.is_empty() || value.chars().count() > MAX_EVIDENCE_LENGTH {
            return Err(CliError::InvalidEvidence { field: name });
        }
        Ok(value.to_owned())
    }
    Ok(Evidence {
        provenance: field(&arguments.provenance, "provenance")?,
        authorization_reference: field(
            &arguments.authorization_reference,
            "authorization-reference",
        )?,
    })
}

fn address_report(chunk: ov580_loader::FirmwareChunk<'_>) -> TransferAddress {
    TransferAddress {
        offset: chunk.offset,
        value: chunk.address.value,
        index: chunk.address.index,
        bytes: chunk.bytes.len(),
    }
}

fn normalize_expected_hash(value: &str) -> Result<String, CliError> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CliError::InvalidExpectedHash);
    }
    Ok(value.to_ascii_lowercase())
}

fn validate_timeout(value: u64, name: &'static str, maximum_ms: u64) -> Result<(), CliError> {
    if value == 0 || value > maximum_ms {
        return Err(CliError::InvalidTimeout { name, maximum_ms });
    }
    Ok(())
}

fn validate_upload_arguments(arguments: &UploadArguments) -> Result<(), CliError> {
    if !arguments.acknowledge_authorized_firmware {
        return Err(CliError::MissingAuthorization);
    }
    if arguments.confirm_device != BOOT_DEVICE_CONFIRMATION {
        return Err(CliError::InvalidDeviceConfirmation);
    }
    validate_timeout(
        arguments.preflight_timeout_ms,
        "preflight-timeout-ms",
        MAX_PREFLIGHT_TIMEOUT_MS,
    )?;
    validate_timeout(
        arguments.transfer_timeout_ms,
        "transfer-timeout-ms",
        MAX_TRANSFER_TIMEOUT_MS,
    )?;
    validate_timeout(
        arguments.upload_deadline_ms,
        "upload-deadline-ms",
        MAX_UPLOAD_DEADLINE_MS,
    )?;
    validate_timeout(
        arguments.reenumeration_timeout_ms,
        "reenumeration-timeout-ms",
        MAX_REENUMERATION_TIMEOUT_MS,
    )?;
    if arguments.transfer_timeout_ms > arguments.upload_deadline_ms {
        return Err(CliError::TransferExceedsDeadline);
    }
    Ok(())
}

fn evaluate_preflight(devices: &[DiscoveredDevice]) -> Result<TargetLocation, PreflightError> {
    let boot = devices
        .iter()
        .filter(|device| device.mode == DeviceMode::Boot)
        .collect::<Vec<_>>();
    let camera = devices
        .iter()
        .filter(|device| device.mode == DeviceMode::Camera)
        .count();
    if boot.len() != 1 || camera != 0 || devices.len() != 1 {
        return Err(PreflightError::AmbiguousDevices {
            boot: boot.len(),
            camera,
        });
    }

    let device = boot[0];
    let target = device.target.clone();
    if !device.accessible {
        return Err(PreflightError::Inaccessible {
            reason: device
                .access_error
                .clone()
                .unwrap_or_else(|| "device could not be opened".into()),
            target: Box::new(target),
        });
    }
    if target.controller_id.is_empty() || target.port_path.is_empty() {
        return Err(PreflightError::MissingTopology {
            target: Box::new(target),
        });
    }
    if !matches!(target.speed.as_str(), "super" | "superplus" | "super_plus") {
        return Err(PreflightError::NotSuperSpeed {
            speed: target.speed.clone(),
            target: Box::new(target),
        });
    }
    Ok(target)
}

fn remaining_budget(
    started: Instant,
    now: Instant,
    limit: Duration,
) -> Result<Duration, DiscoveryError> {
    let elapsed = now.saturating_duration_since(started);
    limit
        .checked_sub(elapsed)
        .filter(|remaining| !remaining.is_zero())
        .ok_or(DiscoveryError::Deadline {
            limit_ms: limit.as_millis(),
        })
}

fn discover_supported_devices(
    started: Instant,
    limit: Duration,
    verify_boot_access: bool,
) -> Result<Vec<DiscoveredDevice>, DiscoveryError> {
    remaining_budget(started, Instant::now(), limit)?;
    let context = rusb::Context::new().map_err(DiscoveryError::Enumeration)?;
    remaining_budget(started, Instant::now(), limit)?;
    let devices = context.devices().map_err(DiscoveryError::Enumeration)?;
    let mut discovered = Vec::new();

    for device in devices.iter() {
        remaining_budget(started, Instant::now(), limit)?;
        let descriptor = device
            .device_descriptor()
            .map_err(DiscoveryError::Descriptor)?;
        let Some(mode) = DeviceMode::from_ids(descriptor.vendor_id(), descriptor.product_id())
        else {
            continue;
        };
        let port_path = device.port_numbers().map_err(DiscoveryError::PortPath)?;
        let (accessible, access_error) = if verify_boot_access && mode == DeviceMode::Boot {
            match device.open() {
                Ok(handle) => {
                    drop(handle);
                    (true, None)
                }
                Err(error) => (false, Some(error.to_string())),
            }
        } else {
            (true, None)
        };
        remaining_budget(started, Instant::now(), limit)?;
        discovered.push(DiscoveredDevice {
            mode,
            target: TargetLocation {
                controller_id: format!("libusb-bus-{}", device.bus_number()),
                bus_number: device.bus_number(),
                device_address: device.address(),
                port_path,
                speed: format!("{:?}", device.speed()).to_ascii_lowercase(),
                windows_instance_id: None,
            },
            accessible,
            access_error,
        });
    }
    remaining_budget(started, Instant::now(), limit)?;
    Ok(discovered)
}

fn boot_locator(target: &TargetLocation) -> BootDeviceLocator {
    BootDeviceLocator {
        controller_id: target.controller_id.clone(),
        bus_number: target.bus_number,
        device_address: target.device_address,
        port_path: target.port_path.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    CancelFile,
    Deadline,
}

impl StopReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CancelFile => "cancel_file",
            Self::Deadline => "deadline",
        }
    }
}

fn cancellation_reason(
    elapsed: Duration,
    deadline: Duration,
    cancel_requested: bool,
) -> Option<StopReason> {
    if cancel_requested {
        Some(StopReason::CancelFile)
    } else if elapsed >= deadline {
        Some(StopReason::Deadline)
    } else {
        None
    }
}

struct OperationalCancellation<'a> {
    started: Instant,
    deadline: Duration,
    cancel_file: Option<&'a Path>,
    reason: Cell<Option<StopReason>>,
}

impl<'a> OperationalCancellation<'a> {
    fn new(deadline: Duration, cancel_file: Option<&'a Path>) -> Self {
        Self {
            started: Instant::now(),
            deadline,
            cancel_file,
            reason: Cell::new(None),
        }
    }

    fn reason(&self) -> Option<StopReason> {
        self.reason.get()
    }
}

impl CancellationCheck for OperationalCancellation<'_> {
    fn is_cancelled(&self) -> bool {
        let reason = cancellation_reason(
            self.started.elapsed(),
            self.deadline,
            self.cancel_file.is_some_and(Path::exists),
        );
        self.reason.set(reason);
        reason.is_some()
    }
}

fn progress_from_report(report: UploadReport) -> UploadProgress {
    UploadProgress {
        bytes_uploaded: report.bytes_uploaded,
        chunks_uploaded: report.chunks_uploaded,
    }
}

fn progress_from_loader_error(error: &LoaderError) -> Option<UploadProgress> {
    match error {
        LoaderError::Cancelled { bytes_uploaded } => Some(UploadProgress {
            bytes_uploaded: *bytes_uploaded,
            chunks_uploaded: *bytes_uploaded / CHUNK_SIZE as u64,
        }),
        LoaderError::UploadTransfer { offset, .. } => Some(UploadProgress {
            bytes_uploaded: *offset,
            chunks_uploaded: *offset / CHUNK_SIZE as u64,
        }),
        LoaderError::ShortWrite { offset, actual, .. } => Some(UploadProgress {
            bytes_uploaded: *offset + *actual as u64,
            chunks_uploaded: *offset / CHUNK_SIZE as u64,
        }),
        _ => None,
    }
}

fn upload(arguments: UploadArguments, audit: &mut AuditState) -> Result<UploadResult, CliError> {
    validate_upload_arguments(&arguments)?;
    let (image, inspection) = inspect_image(&arguments.image, audit)?;

    audit.phase = Phase::Preflight;
    let probe_timeout = Duration::from_millis(arguments.preflight_timeout_ms);
    let preflight_started = Instant::now();
    let discovered = discover_supported_devices(preflight_started, probe_timeout, true)
        .map_err(|error| CliError::Probe(error.to_string()))?;
    let target = match evaluate_preflight(&discovered) {
        Ok(target) => target,
        Err(error) => {
            audit.target = error.target().cloned();
            return Err(CliError::Preflight(error));
        }
    };
    audit.target = Some(target.clone());
    remaining_budget(preflight_started, Instant::now(), probe_timeout)
        .map_err(|error| CliError::Probe(error.to_string()))?;
    let exact_locator = boot_locator(&target);

    audit.phase = Phase::Open;
    let mut device = Ov580BootDevice::open_at(
        &exact_locator,
        LoaderConfig {
            transfer_timeout: Duration::from_millis(arguments.transfer_timeout_ms),
            ..LoaderConfig::default()
        },
    )?;

    audit.phase = Phase::Upload;
    let cancellation = OperationalCancellation::new(
        Duration::from_millis(arguments.upload_deadline_ms),
        arguments.cancel_file.as_deref(),
    );
    let uploaded = match device.upload_at_with_cancellation(&exact_locator, &image, &cancellation) {
        Ok(report) => report,
        Err(error) => {
            if let Some(progress) = progress_from_loader_error(&error) {
                audit.progress = progress;
            }
            let cancellation_error = if let LoaderError::Cancelled { bytes_uploaded } = &error {
                cancellation
                    .reason()
                    .map(|reason| CliError::OperationalCancellation {
                        reason: reason.as_str(),
                        bytes_uploaded: *bytes_uploaded,
                    })
            } else {
                None
            };
            if let Err(release_error) = device.release() {
                audit.cleanup_error = Some(release_error.to_string());
            }
            return Err(cancellation_error.unwrap_or(CliError::Loader(error)));
        }
    };
    audit.progress = progress_from_report(uploaded);

    audit.phase = Phase::Execute;
    let execute = match device.execute_with_cancellation(&cancellation, uploaded.bytes_uploaded) {
        Ok(execute) => execute,
        Err(error) => {
            let cancellation_error = if let LoaderError::Cancelled { bytes_uploaded } = &error {
                cancellation
                    .reason()
                    .map(|reason| CliError::OperationalCancellation {
                        reason: reason.as_str(),
                        bytes_uploaded: *bytes_uploaded,
                    })
            } else {
                None
            };
            if let Err(release_error) = device.release() {
                audit.cleanup_error = Some(release_error.to_string());
            }
            return Err(cancellation_error.unwrap_or(CliError::Loader(error)));
        }
    };
    let execute_name = match execute {
        ExecuteDisposition::CommandAccepted => "command_accepted",
        ExecuteDisposition::DeviceDisconnected => "device_disconnected",
    };
    audit.execute = Some(execute_name);

    audit.phase = Phase::Release;
    device.release()?;

    audit.phase = Phase::Reenumeration;
    let reenumeration_timeout = Duration::from_millis(arguments.reenumeration_timeout_ms);
    let mut backend = CorrelatedReenumerationBackend::new(target.clone(), reenumeration_timeout);
    let outcome = match wait_for_camera_mode(
        &mut backend,
        ReenumerationConfig {
            timeout: reenumeration_timeout,
            poll_interval: Duration::from_millis(100),
        },
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            if let ReenumerationError::Timeout { last_state, .. } = &error {
                audit.last_observed_state = Some(observed_state_name(*last_state));
            }
            return Err(CliError::Reenumeration(error.to_string()));
        }
    };
    let (reenumeration, elapsed) = match outcome {
        ReenumerationOutcome::CameraReady { elapsed } => ("camera_ready", elapsed),
        ReenumerationOutcome::AlreadyCamera { elapsed } => ("camera_ready_on_first_probe", elapsed),
    };
    audit.last_observed_state = Some("camera");
    audit.phase = Phase::Complete;

    Ok(UploadResult {
        schema_version: 1,
        status: "ok",
        operation: "upload",
        evidence: inspection.evidence,
        firmware_bytes: inspection.firmware_bytes,
        sha256: inspection.sha256,
        target,
        progress: audit.progress,
        execute: execute_name,
        reenumeration,
        elapsed_ms: elapsed.as_millis(),
    })
}

fn locations_match(target: &TargetLocation, device: &DiscoveredDevice) -> bool {
    target.controller_id == device.target.controller_id
        && target.bus_number == device.target.bus_number
        && target.port_path == device.target.port_path
}

fn evaluate_correlated_state(
    devices: &[DiscoveredDevice],
    target: &TargetLocation,
) -> Result<ObservedDeviceState, CorrelationError> {
    let cameras = devices
        .iter()
        .filter(|device| device.mode == DeviceMode::Camera)
        .collect::<Vec<_>>();
    if cameras.len() > 1 {
        return Err(CorrelationError::MultipleCameras {
            count: cameras.len(),
        });
    }
    if let Some(camera) = cameras.first() {
        if !locations_match(target, camera) {
            return Err(CorrelationError::CameraTopologyMismatch {
                controller_id: camera.target.controller_id.clone(),
                port_path: camera.target.port_path.clone(),
            });
        }
    }
    let target_boot = devices
        .iter()
        .any(|device| device.mode == DeviceMode::Boot && locations_match(target, device));
    Ok(match (target_boot, !cameras.is_empty()) {
        (false, false) => ObservedDeviceState::Absent,
        (true, false) => ObservedDeviceState::Boot,
        (false, true) => ObservedDeviceState::Camera,
        (true, true) => ObservedDeviceState::BootAndCamera,
    })
}

fn observed_state_name(state: ObservedDeviceState) -> &'static str {
    match state {
        ObservedDeviceState::Absent => "absent",
        ObservedDeviceState::Boot => "boot",
        ObservedDeviceState::Camera => "camera",
        ObservedDeviceState::BootAndCamera => "boot_and_camera",
    }
}

struct CorrelatedReenumerationBackend {
    target: TargetLocation,
    started: Instant,
    timeout: Duration,
}

impl CorrelatedReenumerationBackend {
    fn new(target: TargetLocation, timeout: Duration) -> Self {
        Self {
            target,
            started: Instant::now(),
            timeout,
        }
    }
}

impl ReenumerationBackend for CorrelatedReenumerationBackend {
    type Error = CorrelationError;

    fn observe(&mut self) -> Result<ObservedDeviceState, Self::Error> {
        let devices = discover_supported_devices(self.started, self.timeout, false)
            .map_err(|error| CorrelationError::Discovery(error.to_string()))?;
        evaluate_correlated_state(&devices, &self.target)
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<DiscoveredDevice> {
        let report: ps5cam_usb::ProbeReport = serde_json::from_str(include_str!(
            "../../ps5cam-usb/tests/fixtures/boot-report.json"
        ))
        .expect("valid ps5cam-usb fixture");
        report
            .devices
            .into_iter()
            .map(|device| DiscoveredDevice {
                mode: device.mode,
                target: TargetLocation {
                    controller_id: device.locator.controller_id,
                    bus_number: device.locator.bus_number,
                    device_address: device.locator.device_address,
                    port_path: device.locator.port_path,
                    speed: device.locator.speed.to_ascii_lowercase(),
                    windows_instance_id: device.windows_pnp.map(|pnp| pnp.instance_id),
                },
                accessible: true,
                access_error: None,
            })
            .collect()
    }

    fn image_arguments() -> ImageArguments {
        ImageArguments {
            firmware: PathBuf::from("firmware.bin"),
            expected_sha256: "a".repeat(64),
            provenance: "authorized local acquisition record".to_owned(),
            authorization_reference: "ticket-123".to_owned(),
        }
    }

    #[test]
    fn inspect_is_non_usb_and_requires_evidence_arguments() {
        let hash = "a".repeat(64);
        let cli = Cli::try_parse_from([
            "ps5cam-loader",
            "inspect",
            "firmware.bin",
            "--expected-sha256",
            &hash,
            "--provenance",
            "local-record",
            "--authorization-reference",
            "ticket-123",
        ])
        .expect("valid inspect command");
        let Command::Inspect(arguments) = cli.command else {
            panic!("wrong command")
        };
        assert_eq!(arguments.firmware, Path::new("firmware.bin"));
        assert_eq!(arguments.provenance, "local-record");
    }

    #[test]
    fn upload_requires_authorization() {
        let hash = "a".repeat(64);
        let result = Cli::try_parse_from([
            "ps5cam-loader",
            "upload",
            "firmware.bin",
            "--expected-sha256",
            &hash,
            "--provenance",
            "local-record",
            "--authorization-reference",
            "ticket-123",
            "--confirm-device",
            BOOT_DEVICE_CONFIRMATION,
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn expected_hash_is_normalized_and_strict() {
        assert_eq!(
            normalize_expected_hash(&"A".repeat(64)).unwrap(),
            "a".repeat(64)
        );
        assert!(matches!(
            normalize_expected_hash("abc"),
            Err(CliError::InvalidExpectedHash)
        ));
        assert!(matches!(
            normalize_expected_hash(&"z".repeat(64)),
            Err(CliError::InvalidExpectedHash)
        ));
    }

    #[test]
    fn evidence_is_trimmed_and_empty_values_are_rejected() {
        let mut arguments = image_arguments();
        arguments.provenance = "  local record  ".to_owned();
        assert_eq!(
            validate_evidence(&arguments).unwrap().provenance,
            "local record"
        );
        arguments.authorization_reference = " ".to_owned();
        assert!(matches!(
            validate_evidence(&arguments),
            Err(CliError::InvalidEvidence { .. })
        ));
    }

    #[test]
    fn preflight_requires_one_accessible_superspeed_boot_device() {
        let target = evaluate_preflight(&fixture()).expect("fixture is SuperSpeed boot");
        assert_eq!(target.speed, "super");
        assert!(!target.port_path.is_empty());
    }

    #[test]
    fn preflight_high_speed_failure_preserves_target() {
        let mut report = fixture();
        let expected_port_path = report[0].target.port_path.clone();
        report[0].target.speed = "high".to_owned();
        let error = evaluate_preflight(&report).unwrap_err();
        assert!(matches!(error, PreflightError::NotSuperSpeed { .. }));
        assert_eq!(error.target().unwrap().port_path, expected_port_path);
    }

    #[test]
    fn preflight_rejects_multiple_supported_devices() {
        let mut report = fixture();
        report.push(report[0].clone());
        assert!(matches!(
            evaluate_preflight(&report),
            Err(PreflightError::AmbiguousDevices { boot: 2, .. })
        ));
    }

    #[test]
    fn correlated_camera_must_match_controller_and_port() {
        let mut report = fixture();
        let target = report[0].target.clone();
        report[0].mode = DeviceMode::Camera;
        assert_eq!(
            evaluate_correlated_state(&report, &target).unwrap(),
            ObservedDeviceState::Camera
        );
        report[0].target.port_path = vec![9];
        assert!(matches!(
            evaluate_correlated_state(&report, &target),
            Err(CorrelationError::CameraTopologyMismatch { .. })
        ));
    }

    #[test]
    fn correlated_state_rejects_multiple_camera_devices() {
        let mut report = fixture();
        let target = report[0].target.clone();
        report[0].mode = DeviceMode::Camera;
        report.push(report[0].clone());
        assert!(matches!(
            evaluate_correlated_state(&report, &target),
            Err(CorrelationError::MultipleCameras { count: 2 })
        ));
    }

    #[test]
    fn cancellation_prefers_explicit_file_and_enforces_deadline() {
        assert_eq!(
            cancellation_reason(Duration::ZERO, Duration::from_secs(1), true),
            Some(StopReason::CancelFile)
        );
        assert_eq!(
            cancellation_reason(Duration::from_secs(1), Duration::from_secs(1), false),
            Some(StopReason::Deadline)
        );
        assert_eq!(
            cancellation_reason(Duration::ZERO, Duration::from_secs(1), false),
            None
        );
    }

    #[test]
    fn timeout_bounds_reject_unlimited_operations() {
        assert!(validate_timeout(1, "fixture", 10).is_ok());
        assert!(validate_timeout(0, "fixture", 10).is_err());
        assert!(validate_timeout(11, "fixture", 10).is_err());
    }

    #[test]
    fn discovery_uses_one_absolute_remaining_budget() {
        let started = Instant::now();
        let limit = Duration::from_millis(100);
        assert_eq!(
            remaining_budget(started, started + Duration::from_millis(25), limit).unwrap(),
            Duration::from_millis(75)
        );
        assert!(matches!(
            remaining_budget(started, started + limit, limit),
            Err(DiscoveryError::Deadline { limit_ms: 100 })
        ));
        assert!(remaining_budget(started, started + Duration::from_millis(150), limit).is_err());
    }

    #[test]
    fn preflight_locator_keeps_address_for_final_toctou_check() {
        let target = evaluate_preflight(&fixture()).unwrap();
        let locator = boot_locator(&target);
        assert_eq!(locator.bus_number, target.bus_number);
        assert_eq!(locator.device_address, target.device_address);
        assert_eq!(locator.port_path, target.port_path);
        assert!(!locator.matches_observation(
            0x05a9,
            0x0580,
            &target.controller_id,
            target.bus_number,
            target.device_address.wrapping_add(1),
            &target.port_path,
        ));
    }

    #[test]
    fn partial_write_is_reflected_in_audit_progress() {
        let error = LoaderError::ShortWrite {
            offset: 1_024,
            expected: CHUNK_SIZE,
            actual: 7,
        };
        assert_eq!(
            progress_from_loader_error(&error),
            Some(UploadProgress {
                bytes_uploaded: 1_031,
                chunks_uploaded: 2,
            })
        );
    }

    #[test]
    fn failure_envelope_contains_phase_evidence_and_progress() {
        let cli = Cli {
            command: Command::Inspect(image_arguments()),
        };
        let mut audit = AuditState::from_cli(&cli);
        audit.phase = Phase::Upload;
        audit.firmware_sha256 = Some("b".repeat(64));
        audit.progress.bytes_uploaded = 512;
        let envelope = FailureEnvelope::new(&audit, "loader", "fixture".to_owned());
        let json = serde_json::to_value(envelope).unwrap();
        assert_eq!(json["phase"], "upload");
        assert_eq!(json["progress"]["bytes_uploaded"], 512);
        assert_eq!(json["evidence"]["authorization_reference"], "ticket-123");
    }
}
