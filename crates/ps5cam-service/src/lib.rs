//! Deterministic service core for automatic OV580 firmware loading.
//!
//! Device notifications, upload operations, cancellation, and structured event
//! output are injected. The core never polls USB. V1 embeds one pinned
//! third-party MIT reference artifact; its digest is verified before every
//! upload and its provenance is shipped with the release. A clean-room payload
//! remains the V2 replacement.

use ov580_loader::{
    ExecuteDisposition, FirmwareImage, InterfacePolicy, LoaderConfig, LoaderError, Ov580BootDevice,
    UsbTransport,
};
use ps5cam_usb::DeviceMode;
use rusb::{DeviceHandle, GlobalContext};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;

mod scm;
pub use scm::*;

mod device_notifications;
pub use device_notifications::*;

mod event_log;
pub use event_log::*;

mod cli;
pub use cli::*;

#[cfg(windows)]
mod windows_scm;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn parse_hex(value: &str) -> Result<Self, DigestParseError> {
        if value.len() != 64 {
            return Err(DigestParseError::WrongLength {
                actual: value.len(),
            });
        }

        let mut bytes = [0_u8; 32];
        for (index, output) in bytes.iter_mut().enumerate() {
            let start = index * 2;
            let high = hex_nibble(value.as_bytes()[start])
                .ok_or(DigestParseError::InvalidHex { index: start })?;
            let low = hex_nibble(value.as_bytes()[start + 1])
                .ok_or(DigestParseError::InvalidHex { index: start + 1 })?;
            *output = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    pub fn calculate(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DigestParseError {
    #[error("SHA-256 must contain exactly 64 hexadecimal characters, got {actual}")]
    WrongLength { actual: usize },
    #[error("invalid hexadecimal pair at character {index}")]
    InvalidHex { index: usize },
}

/// Immutable payload paired with a digest pinned by the trusted service config.
#[derive(Clone)]
pub struct FirmwarePayload {
    version: Arc<str>,
    bytes: Arc<[u8]>,
    expected_sha256: Sha256Digest,
}

impl fmt::Debug for FirmwarePayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FirmwarePayload")
            .field("version", &self.version)
            .field("length", &self.bytes.len())
            .field("expected_sha256", &self.expected_sha256)
            .finish()
    }
}

impl FirmwarePayload {
    pub fn new(
        version: impl Into<Arc<str>>,
        bytes: impl Into<Arc<[u8]>>,
        expected_sha256: Sha256Digest,
    ) -> Self {
        Self {
            version: version.into(),
            bytes: bytes.into(),
            expected_sha256,
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn verify(&self) -> Result<FirmwareImage, PayloadError> {
        self.verify_bounded(usize::MAX)
    }

    pub fn verify_bounded(
        &self,
        maximum_image_bytes: usize,
    ) -> Result<FirmwareImage, PayloadError> {
        if self.bytes.len() > maximum_image_bytes {
            return Err(PayloadError::ImageTooLarge {
                actual: self.bytes.len(),
                maximum: maximum_image_bytes,
            });
        }
        let actual = Sha256Digest::calculate(&self.bytes);
        if actual != self.expected_sha256 {
            return Err(PayloadError::DigestMismatch {
                expected: self.expected_sha256,
                actual,
            });
        }

        FirmwareImage::from_bytes(self.bytes.to_vec()).map_err(PayloadError::InvalidImage)
    }
}

pub const BUNDLED_REFERENCE_FIRMWARE_SHA256: &str =
    "10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54";
pub const BUNDLED_REFERENCE_FIRMWARE_VERSION: &str = "21.01-03.20.00.04-00.00.00";
const BUNDLED_REFERENCE_FIRMWARE: &[u8] =
    include_bytes!("../../../firmware/reference/21.01-03.20.00.04-00.00.00.bin");

/// The only third-party payload allowed in V1. The same SHA-256 is checked at
/// runtime, so a modified installed service cannot silently upload a different
/// image through this constructor.
pub fn bundled_reference_firmware_payload() -> FirmwarePayload {
    let digest = Sha256Digest::parse_hex(BUNDLED_REFERENCE_FIRMWARE_SHA256)
        .expect("the bundled firmware SHA-256 constant must be valid");
    FirmwarePayload::new(
        BUNDLED_REFERENCE_FIRMWARE_VERSION,
        Arc::<[u8]>::from(BUNDLED_REFERENCE_FIRMWARE),
        digest,
    )
}

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("firmware image exceeds service bound: maximum {maximum} bytes, got {actual}")]
    ImageTooLarge { actual: usize, maximum: usize },
    #[error("firmware digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    #[error("firmware image is invalid: {0}")]
    InvalidImage(#[source] ov580_loader::FirmwareImageError),
}

pub trait CancellationSignal {
    fn is_cancelled(&self) -> bool;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NeverCancelled;

impl CancellationSignal for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DeviceEvent {
    Arrived {
        mode: DeviceMode,
        instance_id: String,
        locator: Option<StableUsbLocator>,
        at: Duration,
    },
    Removed {
        instance_id: String,
        at: Duration,
    },
    Timer {
        at: Duration,
    },
    Cancel {
        at: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbLinkSpeed {
    Low,
    Full,
    High,
    Super,
    SuperPlus,
    Unknown,
}

impl UsbLinkSpeed {
    pub const fn supports_camera_upload(self) -> bool {
        matches!(self, Self::Super | Self::SuperPlus)
    }

    fn from_rusb(speed: rusb::Speed) -> Self {
        match speed {
            rusb::Speed::Low => Self::Low,
            rusb::Speed::Full => Self::Full,
            rusb::Speed::High => Self::High,
            rusb::Speed::Super => Self::Super,
            rusb::Speed::SuperPlus => Self::SuperPlus,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StableUsbLocator {
    pub controller_id: String,
    pub port_path: Vec<u8>,
    pub speed: UsbLinkSpeed,
}

impl StableUsbLocator {
    pub fn new(controller_id: impl Into<String>, port_path: Vec<u8>, speed: UsbLinkSpeed) -> Self {
        Self {
            controller_id: controller_id.into(),
            port_path,
            speed,
        }
    }

    pub fn is_complete(&self) -> bool {
        !self.controller_id.is_empty() && !self.port_path.is_empty()
    }

    pub fn same_physical_port(&self, other: &Self) -> bool {
        self.controller_id == other.controller_id && self.port_path == other.port_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedUsbObservation {
    pub mode: DeviceMode,
    pub locator: StableUsbLocator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSupportedDevice {
    pub mode: DeviceMode,
    pub instance_id: String,
    pub locator: StableUsbLocator,
}

/// Enumerates the one supported physical camera without opening it. This is
/// used at service startup and after a matching Windows device notification;
/// it never runs as an unbounded USB polling loop.
pub fn discover_single_supported_device() -> Result<Option<DiscoveredSupportedDevice>, FailureCode>
{
    let devices = rusb::devices().map_err(|_| FailureCode::DeviceUnavailable)?;
    let mut supported = Vec::new();
    for device in devices.iter() {
        let Ok(descriptor) = device.device_descriptor() else {
            continue;
        };
        let Some(mode) = DeviceMode::from_ids(descriptor.vendor_id(), descriptor.product_id())
        else {
            continue;
        };
        let port_path = device
            .port_numbers()
            .map_err(|_| FailureCode::TopologyUnavailable)?;
        let locator = StableUsbLocator::new(
            format!("libusb-bus-{}", device.bus_number()),
            port_path,
            UsbLinkSpeed::from_rusb(device.speed()),
        );
        let port = locator
            .port_path
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(".");
        supported.push(DiscoveredSupportedDevice {
            mode,
            instance_id: format!("{}:{port}", locator.controller_id),
            locator,
        });
    }
    match supported.len() {
        0 => Ok(None),
        1 => Ok(supported.pop()),
        _ => Err(FailureCode::MultipleSupportedDevices),
    }
}

/// Converts two point-in-time discoveries into the minimum deterministic
/// event sequence consumed by the service state machine.
pub fn reconcile_discovered_device(
    previous: Option<&DiscoveredSupportedDevice>,
    current: Option<&DiscoveredSupportedDevice>,
    at: Duration,
) -> Vec<DeviceEvent> {
    if previous == current {
        return Vec::new();
    }
    let mut events = Vec::with_capacity(2);
    if let Some(previous) = previous {
        events.push(DeviceEvent::Removed {
            instance_id: previous.instance_id.clone(),
            at,
        });
    }
    if let Some(current) = current {
        events.push(DeviceEvent::Arrived {
            mode: current.mode,
            instance_id: current.instance_id.clone(),
            locator: Some(current.locator.clone()),
            at,
        });
    }
    events
}

pub fn validate_single_supported_device(
    observations: &[SupportedUsbObservation],
    expected_boot: &StableUsbLocator,
) -> Result<DeviceMode, FailureCode> {
    let [observed] = observations else {
        return Err(if observations.is_empty() {
            FailureCode::DeviceUnavailable
        } else {
            FailureCode::MultipleSupportedDevices
        });
    };
    if !observed.locator.is_complete() {
        return Err(FailureCode::TopologyUnavailable);
    }
    if !observed.locator.speed.supports_camera_upload() {
        return Err(FailureCode::UnsupportedLinkSpeed);
    }
    if !expected_boot.same_physical_port(&observed.locator) {
        return Err(FailureCode::TopologyMismatch);
    }
    if observed.mode == DeviceMode::Boot && observed.locator != *expected_boot {
        return Err(FailureCode::TopologyMismatch);
    }
    Ok(observed.mode)
}

pub fn bounded_transfer_timeout(
    per_transfer: Duration,
    deadline_remaining: Duration,
) -> Option<Duration> {
    if per_transfer.is_zero() || deadline_remaining.is_zero() {
        None
    } else {
        Some(per_transfer.min(deadline_remaining))
    }
}

/// Final fail-closed gate between the last uploaded chunk and the execute
/// control transfer. Callers must evaluate the monotonic deadline immediately
/// before invoking this function.
pub fn check_pre_execute_gate(
    cancellation: &dyn CancellationSignal,
    deadline_reached: bool,
) -> Result<(), UploadFailure> {
    if cancellation.is_cancelled() {
        Err(UploadFailure {
            code: FailureCode::Cancelled,
        })
    } else if deadline_reached {
        Err(UploadFailure {
            code: FailureCode::UploadDeadlineExceeded,
        })
    } else {
        Ok(())
    }
}

impl DeviceEvent {
    pub fn at(&self) -> Duration {
        match self {
            Self::Arrived { at, .. }
            | Self::Removed { at, .. }
            | Self::Timer { at }
            | Self::Cancel { at } => *at,
        }
    }
}

pub trait DeviceEventSource {
    fn next_event(
        &mut self,
        cancellation: &dyn CancellationSignal,
    ) -> Result<Option<DeviceEvent>, Box<dyn std::error::Error + Send + Sync>>;
}

/// Receives lifecycle changes for the native UVC function only. The service
/// core owns device selection; observers never receive a boot-mode device and
/// cannot trigger an upload.
pub trait UvcLifecycleObserver {
    fn camera_ready(&mut self, instance_id: &str, locator: &StableUsbLocator);
    fn camera_removed(&mut self, instance_id: &str, locator: &StableUsbLocator);
}

#[derive(Debug, Default)]
pub struct NoopUvcLifecycleObserver;

impl UvcLifecycleObserver for NoopUvcLifecycleObserver {
    fn camera_ready(&mut self, _: &str, _: &StableUsbLocator) {}
    fn camera_removed(&mut self, _: &str, _: &StableUsbLocator) {}
}

pub trait StructuredEventSink {
    fn emit(
        &mut self,
        record: &ServiceRecord,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateName {
    Absent,
    Boot,
    Uploading,
    Reenlisting,
    RetryWaiting,
    Ready,
    FailedPermanent,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Absent,
    Boot {
        instance_id: String,
        locator: StableUsbLocator,
        attempt: u32,
    },
    Uploading {
        instance_id: String,
        locator: StableUsbLocator,
        attempt: u32,
    },
    Reenlisting {
        boot_instance_id: String,
        boot_locator: StableUsbLocator,
        attempt: u32,
        deadline: Duration,
    },
    RetryWaiting {
        boot_instance_id: String,
        boot_locator: StableUsbLocator,
        attempt: u32,
        retry_at: Duration,
        failure: FailureCode,
    },
    Ready {
        camera_instance_id: String,
        camera_locator: StableUsbLocator,
    },
    FailedPermanent {
        boot_instance_id: String,
        boot_locator: StableUsbLocator,
        attempts: u32,
        failure: FailureCode,
    },
    Stopped,
}

impl ServiceState {
    pub fn name(&self) -> StateName {
        match self {
            Self::Absent => StateName::Absent,
            Self::Boot { .. } => StateName::Boot,
            Self::Uploading { .. } => StateName::Uploading,
            Self::Reenlisting { .. } => StateName::Reenlisting,
            Self::RetryWaiting { .. } => StateName::RetryWaiting,
            Self::Ready { .. } => StateName::Ready,
            Self::FailedPermanent { .. } => StateName::FailedPermanent,
            Self::Stopped => StateName::Stopped,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    PayloadDigestMismatch,
    InvalidFirmware,
    InvalidUploadLimits,
    FirmwareImageTooLarge,
    TopologyUnavailable,
    UnsupportedLinkSpeed,
    MultipleSupportedDevices,
    TopologyMismatch,
    DeviceDisconnected,
    TransferTimeout,
    UploadDeadlineExceeded,
    DeviceUnavailable,
    UploadFailed,
    ReenumerationTimeout,
    Cancelled,
}

impl FailureCode {
    fn recoverable(self) -> bool {
        matches!(
            self,
            Self::DeviceDisconnected
                | Self::TransferTimeout
                | Self::UploadDeadlineExceeded
                | Self::DeviceUnavailable
                | Self::UploadFailed
                | Self::ReenumerationTimeout
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    BootObserved,
    UploadStarted,
    UploadCompleted,
    ReenumerationStarted,
    CameraReady,
    RetryScheduled,
    ReenumerationTimedOut,
    DeviceRemoved,
    DuplicateIgnored,
    TopologyRejected,
    PermanentFailure,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceRecord {
    pub sequence: u64,
    pub at: Duration,
    pub level: RecordLevel,
    pub kind: RecordKind,
    pub from: StateName,
    pub to: StateName,
    pub attempt: Option<u32>,
    pub failure: Option<FailureCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceConfig {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub maximum_backoff: Duration,
    pub reenumeration_timeout: Duration,
    pub upload_limits: UploadLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadLimits {
    pub maximum_image_bytes: usize,
    pub transfer_timeout: Duration,
    pub total_timeout: Duration,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            maximum_image_bytes: 4 * 1024 * 1024,
            transfer_timeout: Duration::from_secs(1),
            total_timeout: Duration::from_secs(120),
        }
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_secs(1),
            maximum_backoff: Duration::from_secs(30),
            reenumeration_timeout: Duration::from_secs(10),
            upload_limits: UploadLimits::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServiceConfigError {
    #[error("max_attempts must be greater than zero")]
    ZeroAttempts,
    #[error("backoff and re-enumeration timeout durations must be non-zero")]
    ZeroDuration,
    #[error("maximum_backoff must be at least initial_backoff")]
    InvalidBackoffRange,
    #[error("upload image bound and timing limits must be non-zero")]
    InvalidUploadLimits,
    #[error("per-transfer timeout must not exceed the total upload timeout")]
    TransferTimeoutExceedsTotal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadOutcome {
    Reenumerating,
    AlreadyCamera,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("firmware upload failed: {code:?}")]
pub struct UploadFailure {
    pub code: FailureCode,
}

pub trait FirmwareUploader {
    fn upload_and_execute(
        &mut self,
        image: &FirmwareImage,
        boot_locator: &StableUsbLocator,
        limits: UploadLimits,
        cancellation: &dyn CancellationSignal,
    ) -> Result<UploadOutcome, UploadFailure>;
}

#[derive(Debug, Clone, Copy)]
pub struct RusbFirmwareUploader {
    pub interface: u8,
    pub interface_policy: InterfacePolicy,
}

impl Default for RusbFirmwareUploader {
    fn default() -> Self {
        Self {
            interface: 0,
            interface_policy: InterfacePolicy::PlatformDefault,
        }
    }
}

struct LoaderCancellation<'a> {
    external: &'a dyn CancellationSignal,
    deadline: Instant,
}

impl ov580_loader::CancellationCheck for LoaderCancellation<'_> {
    fn is_cancelled(&self) -> bool {
        self.external.is_cancelled() || Instant::now() >= self.deadline
    }
}

struct DeadlineTransport {
    inner: DeviceHandle<GlobalContext>,
    deadline: Instant,
}

impl UsbTransport for DeadlineTransport {
    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, rusb::Error> {
        self.inner.kernel_driver_active(interface)
    }

    fn detach_kernel_driver(&mut self, interface: u8) -> Result<(), rusb::Error> {
        self.inner.detach_kernel_driver(interface)
    }

    fn claim_interface(&mut self, interface: u8) -> Result<(), rusb::Error> {
        self.inner.claim_interface(interface)
    }

    fn release_interface(&mut self, interface: u8) -> Result<(), rusb::Error> {
        self.inner.release_interface(interface)
    }

    fn write_control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
        timeout: Duration,
    ) -> Result<usize, rusb::Error> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let timeout = bounded_transfer_timeout(timeout, remaining).ok_or(rusb::Error::Timeout)?;
        self.inner
            .write_control(request_type, request, value, index, data, timeout)
    }
}

impl FirmwareUploader for RusbFirmwareUploader {
    fn upload_and_execute(
        &mut self,
        image: &FirmwareImage,
        boot_locator: &StableUsbLocator,
        limits: UploadLimits,
        cancellation: &dyn CancellationSignal,
    ) -> Result<UploadOutcome, UploadFailure> {
        if limits.maximum_image_bytes == 0
            || limits.transfer_timeout.is_zero()
            || limits.total_timeout.is_zero()
            || limits.transfer_timeout > limits.total_timeout
        {
            return Err(UploadFailure {
                code: FailureCode::InvalidUploadLimits,
            });
        }
        if image.len() > limits.maximum_image_bytes {
            return Err(UploadFailure {
                code: FailureCode::FirmwareImageTooLarge,
            });
        }
        if !boot_locator.is_complete() {
            return Err(UploadFailure {
                code: FailureCode::TopologyUnavailable,
            });
        }
        if !boot_locator.speed.supports_camera_upload() {
            return Err(UploadFailure {
                code: FailureCode::UnsupportedLinkSpeed,
            });
        }
        if cancellation.is_cancelled() {
            return Err(UploadFailure {
                code: FailureCode::Cancelled,
            });
        }

        let deadline = Instant::now()
            .checked_add(limits.total_timeout)
            .ok_or(UploadFailure {
                code: FailureCode::UploadDeadlineExceeded,
            })?;

        let opened = open_single_supported_device(boot_locator)?;
        if Instant::now() >= deadline {
            return Err(UploadFailure {
                code: FailureCode::UploadDeadlineExceeded,
            });
        }
        let (observed_locator, handle) = match opened {
            OpenedSupportedDevice::Camera => return Ok(UploadOutcome::AlreadyCamera),
            OpenedSupportedDevice::Boot { locator, handle } => (locator, handle),
        };
        debug_assert_eq!(observed_locator, *boot_locator);

        let transport = DeadlineTransport {
            inner: handle,
            deadline,
        };
        let loader_config = LoaderConfig {
            interface: self.interface,
            transfer_timeout: limits.transfer_timeout,
            interface_policy: self.interface_policy,
        };
        let mut device =
            Ov580BootDevice::from_transport(transport, loader_config).map_err(map_loader_error)?;
        let bridge = LoaderCancellation {
            external: cancellation,
            deadline,
        };
        if let Err(error) = device.upload_with_cancellation(image, &bridge) {
            if !cancellation.is_cancelled() && Instant::now() >= deadline {
                return Err(UploadFailure {
                    code: FailureCode::UploadDeadlineExceeded,
                });
            }
            return Err(map_loader_error(error));
        }
        check_pre_execute_gate(cancellation, Instant::now() >= deadline)?;
        let _disposition: ExecuteDisposition = device
            .execute_with_cancellation(&bridge, image.len() as u64)
            .map_err(|error| {
                check_pre_execute_gate(cancellation, Instant::now() >= deadline)
                    .err()
                    .unwrap_or_else(|| map_loader_error(error))
            })?;
        Ok(UploadOutcome::Reenumerating)
    }
}

enum OpenedSupportedDevice {
    Boot {
        locator: StableUsbLocator,
        handle: DeviceHandle<GlobalContext>,
    },
    Camera,
}

fn open_single_supported_device(
    expected_boot: &StableUsbLocator,
) -> Result<OpenedSupportedDevice, UploadFailure> {
    let devices = rusb::devices().map_err(|_| UploadFailure {
        code: FailureCode::DeviceUnavailable,
    })?;
    let mut supported = Vec::new();
    for device in devices.iter() {
        let descriptor = device.device_descriptor().map_err(|_| UploadFailure {
            code: FailureCode::DeviceUnavailable,
        })?;
        let Some(mode) = DeviceMode::from_ids(descriptor.vendor_id(), descriptor.product_id())
        else {
            continue;
        };
        let port_path = device.port_numbers().map_err(|_| UploadFailure {
            code: FailureCode::TopologyUnavailable,
        })?;
        supported.push((
            mode,
            StableUsbLocator::new(
                format!("libusb-bus-{}", device.bus_number()),
                port_path,
                UsbLinkSpeed::from_rusb(device.speed()),
            ),
            device,
        ));
    }
    let observations = supported
        .iter()
        .map(|(mode, locator, _)| SupportedUsbObservation {
            mode: *mode,
            locator: locator.clone(),
        })
        .collect::<Vec<_>>();
    let mode = validate_single_supported_device(&observations, expected_boot)
        .map_err(|code| UploadFailure { code })?;
    let Some((_, locator, device)) = supported.pop() else {
        return Err(UploadFailure {
            code: FailureCode::DeviceUnavailable,
        });
    };
    if mode == DeviceMode::Camera {
        return Ok(OpenedSupportedDevice::Camera);
    }
    let handle = device.open().map_err(|_| UploadFailure {
        code: FailureCode::DeviceUnavailable,
    })?;
    Ok(OpenedSupportedDevice::Boot { locator, handle })
}

fn map_loader_error(error: LoaderError) -> UploadFailure {
    let code = match error {
        LoaderError::Cancelled { .. } => FailureCode::Cancelled,
        LoaderError::UploadTransfer {
            source: rusb::Error::NoDevice,
            ..
        }
        | LoaderError::ExecuteTransfer(rusb::Error::NoDevice)
        | LoaderError::DeviceNotFound => FailureCode::DeviceDisconnected,
        LoaderError::UploadTransfer {
            source: rusb::Error::Timeout,
            ..
        }
        | LoaderError::ExecuteTransfer(rusb::Error::Timeout) => FailureCode::TransferTimeout,
        LoaderError::Open(_) | LoaderError::Enumeration(_) | LoaderError::DeviceDescriptor(_) => {
            FailureCode::DeviceUnavailable
        }
        _ => FailureCode::UploadFailed,
    };
    UploadFailure { code }
}

pub struct ServiceEngine {
    config: ServiceConfig,
    payload: FirmwarePayload,
    state: ServiceState,
    sequence: u64,
    last_at: Duration,
}

#[derive(Debug, Clone)]
struct BootTarget {
    instance_id: String,
    locator: StableUsbLocator,
}

impl ServiceEngine {
    pub fn new(
        config: ServiceConfig,
        payload: FirmwarePayload,
    ) -> Result<Self, ServiceConfigError> {
        validate_config(config)?;
        Ok(Self {
            config,
            payload,
            state: ServiceState::Absent,
            sequence: 0,
            last_at: Duration::ZERO,
        })
    }

    pub fn state(&self) -> &ServiceState {
        &self.state
    }

    pub fn last_event_time(&self) -> Duration {
        self.last_at
    }

    pub fn handle<U: FirmwareUploader>(
        &mut self,
        event: DeviceEvent,
        uploader: &mut U,
        cancellation: &dyn CancellationSignal,
    ) -> Vec<ServiceRecord> {
        let at = event.at().max(self.last_at);
        self.last_at = at;
        let mut records = Vec::new();

        match event {
            DeviceEvent::Cancel { .. } => {
                self.change_state(
                    ServiceState::Stopped,
                    at,
                    RecordLevel::Info,
                    RecordKind::Cancelled,
                    None,
                    Some(FailureCode::Cancelled),
                    &mut records,
                );
            }
            DeviceEvent::Arrived {
                mode: DeviceMode::Camera,
                instance_id,
                locator,
                ..
            } => self.camera_arrived(instance_id, locator, at, &mut records),
            DeviceEvent::Arrived {
                mode: DeviceMode::Boot,
                instance_id,
                locator,
                ..
            } => self.boot_arrived(
                instance_id,
                locator,
                at,
                uploader,
                cancellation,
                &mut records,
            ),
            DeviceEvent::Removed { instance_id, .. } => {
                self.device_removed(instance_id, at, &mut records)
            }
            DeviceEvent::Timer { .. } => self.timer(at, uploader, cancellation, &mut records),
        }

        records
    }

    pub fn handle_with_uvc_observer<U: FirmwareUploader, O: UvcLifecycleObserver>(
        &mut self,
        event: DeviceEvent,
        uploader: &mut U,
        cancellation: &dyn CancellationSignal,
        observer: &mut O,
    ) -> Vec<ServiceRecord> {
        let previous_ready = match &self.state {
            ServiceState::Ready {
                camera_instance_id,
                camera_locator,
            } => Some((camera_instance_id.clone(), camera_locator.clone())),
            _ => None,
        };
        let records = self.handle(event, uploader, cancellation);
        match (&previous_ready, &self.state) {
            (
                None,
                ServiceState::Ready {
                    camera_instance_id,
                    camera_locator,
                },
            ) => {
                observer.camera_ready(camera_instance_id, camera_locator);
            }
            (Some((instance_id, locator)), ServiceState::Absent | ServiceState::Stopped) => {
                observer.camera_removed(instance_id, locator);
            }
            _ => {}
        }
        records
    }

    fn camera_arrived(
        &mut self,
        instance_id: String,
        locator: Option<StableUsbLocator>,
        at: Duration,
        records: &mut Vec<ServiceRecord>,
    ) {
        if matches!(self.state, ServiceState::Stopped) {
            self.ignored(at, records);
            return;
        }
        let Some(locator) = self.accept_locator(locator, at, records) else {
            return;
        };
        if let ServiceState::Reenlisting { boot_locator, .. } = &self.state {
            if !boot_locator.same_physical_port(&locator) {
                self.record_without_transition(
                    at,
                    RecordLevel::Error,
                    RecordKind::TopologyRejected,
                    None,
                    Some(FailureCode::TopologyMismatch),
                    records,
                );
                return;
            }
        } else if let ServiceState::Ready { camera_locator, .. } = &self.state {
            if !camera_locator.same_physical_port(&locator) {
                self.record_without_transition(
                    at,
                    RecordLevel::Error,
                    RecordKind::TopologyRejected,
                    None,
                    Some(FailureCode::MultipleSupportedDevices),
                    records,
                );
                return;
            }
        } else if !matches!(
            self.state,
            ServiceState::Absent | ServiceState::Ready { .. }
        ) {
            self.record_without_transition(
                at,
                RecordLevel::Error,
                RecordKind::TopologyRejected,
                None,
                Some(FailureCode::MultipleSupportedDevices),
                records,
            );
            return;
        }
        self.change_state(
            ServiceState::Ready {
                camera_instance_id: instance_id,
                camera_locator: locator,
            },
            at,
            RecordLevel::Info,
            RecordKind::CameraReady,
            None,
            None,
            records,
        );
    }

    fn boot_arrived<U: FirmwareUploader>(
        &mut self,
        instance_id: String,
        locator: Option<StableUsbLocator>,
        at: Duration,
        uploader: &mut U,
        cancellation: &dyn CancellationSignal,
        records: &mut Vec<ServiceRecord>,
    ) {
        let Some(locator) = self.accept_locator(locator, at, records) else {
            return;
        };
        match &self.state {
            ServiceState::Absent => self.start_upload(
                BootTarget {
                    instance_id,
                    locator,
                },
                1,
                at,
                uploader,
                cancellation,
                records,
            ),
            ServiceState::Reenlisting {
                attempt,
                boot_locator,
                ..
            } if boot_locator.same_physical_port(&locator) => {
                let attempt = *attempt;
                self.fail_attempt(
                    instance_id,
                    locator,
                    attempt,
                    FailureCode::DeviceDisconnected,
                    at,
                    records,
                );
            }
            ServiceState::RetryWaiting { .. }
            | ServiceState::Boot { .. }
            | ServiceState::Uploading { .. }
            | ServiceState::Ready { .. }
            | ServiceState::FailedPermanent { .. }
            | ServiceState::Stopped => self.ignored(at, records),
            ServiceState::Reenlisting { .. } => self.record_without_transition(
                at,
                RecordLevel::Error,
                RecordKind::TopologyRejected,
                None,
                Some(FailureCode::MultipleSupportedDevices),
                records,
            ),
        }
    }

    fn start_upload<U: FirmwareUploader>(
        &mut self,
        target: BootTarget,
        attempt: u32,
        at: Duration,
        uploader: &mut U,
        cancellation: &dyn CancellationSignal,
        records: &mut Vec<ServiceRecord>,
    ) {
        let BootTarget {
            instance_id,
            locator,
        } = target;
        self.change_state(
            ServiceState::Boot {
                instance_id: instance_id.clone(),
                locator: locator.clone(),
                attempt,
            },
            at,
            RecordLevel::Info,
            RecordKind::BootObserved,
            Some(attempt),
            None,
            records,
        );
        self.change_state(
            ServiceState::Uploading {
                instance_id: instance_id.clone(),
                locator: locator.clone(),
                attempt,
            },
            at,
            RecordLevel::Info,
            RecordKind::UploadStarted,
            Some(attempt),
            None,
            records,
        );

        if cancellation.is_cancelled() {
            self.cancel_upload(at, attempt, records);
            return;
        }

        let image = match self
            .payload
            .verify_bounded(self.config.upload_limits.maximum_image_bytes)
        {
            Ok(image) => image,
            Err(PayloadError::DigestMismatch { .. }) => {
                self.fail_attempt(
                    instance_id,
                    locator,
                    attempt,
                    FailureCode::PayloadDigestMismatch,
                    at,
                    records,
                );
                return;
            }
            Err(PayloadError::InvalidImage(_)) => {
                self.fail_attempt(
                    instance_id,
                    locator,
                    attempt,
                    FailureCode::InvalidFirmware,
                    at,
                    records,
                );
                return;
            }
            Err(PayloadError::ImageTooLarge { .. }) => {
                self.fail_attempt(
                    instance_id,
                    locator,
                    attempt,
                    FailureCode::FirmwareImageTooLarge,
                    at,
                    records,
                );
                return;
            }
        };

        match uploader.upload_and_execute(&image, &locator, self.config.upload_limits, cancellation)
        {
            Ok(UploadOutcome::AlreadyCamera) => self.change_state(
                ServiceState::Ready {
                    camera_instance_id: instance_id,
                    camera_locator: locator,
                },
                at,
                RecordLevel::Info,
                RecordKind::CameraReady,
                Some(attempt),
                None,
                records,
            ),
            Ok(UploadOutcome::Reenumerating) => {
                self.record_without_transition(
                    at,
                    RecordLevel::Info,
                    RecordKind::UploadCompleted,
                    Some(attempt),
                    None,
                    records,
                );
                self.change_state(
                    ServiceState::Reenlisting {
                        boot_instance_id: instance_id,
                        boot_locator: locator,
                        attempt,
                        deadline: at.saturating_add(self.config.reenumeration_timeout),
                    },
                    at,
                    RecordLevel::Info,
                    RecordKind::ReenumerationStarted,
                    Some(attempt),
                    None,
                    records,
                );
            }
            Err(UploadFailure {
                code: FailureCode::Cancelled,
            }) => self.cancel_upload(at, attempt, records),
            Err(failure) => {
                self.fail_attempt(instance_id, locator, attempt, failure.code, at, records);
            }
        }
    }

    fn timer<U: FirmwareUploader>(
        &mut self,
        at: Duration,
        uploader: &mut U,
        cancellation: &dyn CancellationSignal,
        records: &mut Vec<ServiceRecord>,
    ) {
        let action = match &self.state {
            ServiceState::Reenlisting {
                boot_instance_id,
                boot_locator,
                attempt,
                deadline,
            } if at >= *deadline => Some((
                boot_instance_id.clone(),
                boot_locator.clone(),
                *attempt,
                true,
            )),
            ServiceState::RetryWaiting {
                boot_instance_id,
                boot_locator,
                attempt,
                retry_at,
                ..
            } if at >= *retry_at => Some((
                boot_instance_id.clone(),
                boot_locator.clone(),
                *attempt + 1,
                false,
            )),
            _ => None,
        };

        match action {
            Some((instance_id, locator, attempt, true)) => {
                self.record_without_transition(
                    at,
                    RecordLevel::Warning,
                    RecordKind::ReenumerationTimedOut,
                    Some(attempt),
                    Some(FailureCode::ReenumerationTimeout),
                    records,
                );
                self.fail_attempt(
                    instance_id,
                    locator,
                    attempt,
                    FailureCode::ReenumerationTimeout,
                    at,
                    records,
                );
            }
            Some((instance_id, locator, attempt, false)) => self.start_upload(
                BootTarget {
                    instance_id,
                    locator,
                },
                attempt,
                at,
                uploader,
                cancellation,
                records,
            ),
            None => {}
        }
    }

    fn device_removed(
        &mut self,
        instance_id: String,
        at: Duration,
        records: &mut Vec<ServiceRecord>,
    ) {
        let expected_reenlist = matches!(
            &self.state,
            ServiceState::Reenlisting {
                boot_instance_id,
                ..
            } if *boot_instance_id == instance_id
        );
        if expected_reenlist {
            self.record_without_transition(
                at,
                RecordLevel::Info,
                RecordKind::DeviceRemoved,
                None,
                None,
                records,
            );
            return;
        }

        let matches_active = match &self.state {
            ServiceState::Boot {
                instance_id: active,
                ..
            }
            | ServiceState::Uploading {
                instance_id: active,
                ..
            } => *active == instance_id,
            ServiceState::RetryWaiting {
                boot_instance_id: active,
                ..
            }
            | ServiceState::FailedPermanent {
                boot_instance_id: active,
                ..
            } => *active == instance_id,
            ServiceState::Ready {
                camera_instance_id: active,
                ..
            } => *active == instance_id,
            _ => false,
        };

        if matches_active {
            self.change_state(
                ServiceState::Absent,
                at,
                RecordLevel::Info,
                RecordKind::DeviceRemoved,
                None,
                None,
                records,
            );
        } else {
            self.ignored(at, records);
        }
    }

    fn fail_attempt(
        &mut self,
        instance_id: String,
        locator: StableUsbLocator,
        attempt: u32,
        failure: FailureCode,
        at: Duration,
        records: &mut Vec<ServiceRecord>,
    ) {
        if failure == FailureCode::Cancelled {
            self.cancel_upload(at, attempt, records);
        } else if failure.recoverable() && attempt < self.config.max_attempts {
            let retry_at = at.saturating_add(self.backoff_for(attempt));
            self.change_state(
                ServiceState::RetryWaiting {
                    boot_instance_id: instance_id,
                    boot_locator: locator,
                    attempt,
                    retry_at,
                    failure,
                },
                at,
                RecordLevel::Warning,
                RecordKind::RetryScheduled,
                Some(attempt),
                Some(failure),
                records,
            );
        } else {
            self.change_state(
                ServiceState::FailedPermanent {
                    boot_instance_id: instance_id,
                    boot_locator: locator,
                    attempts: attempt,
                    failure,
                },
                at,
                RecordLevel::Error,
                RecordKind::PermanentFailure,
                Some(attempt),
                Some(failure),
                records,
            );
        }
    }

    fn cancel_upload(&mut self, at: Duration, attempt: u32, records: &mut Vec<ServiceRecord>) {
        self.change_state(
            ServiceState::Stopped,
            at,
            RecordLevel::Info,
            RecordKind::Cancelled,
            Some(attempt),
            Some(FailureCode::Cancelled),
            records,
        );
    }

    fn backoff_for(&self, failed_attempt: u32) -> Duration {
        let exponent = failed_attempt.saturating_sub(1).min(31);
        let multiplier = 1_u32 << exponent;
        self.config
            .initial_backoff
            .saturating_mul(multiplier)
            .min(self.config.maximum_backoff)
    }

    fn accept_locator(
        &mut self,
        locator: Option<StableUsbLocator>,
        at: Duration,
        records: &mut Vec<ServiceRecord>,
    ) -> Option<StableUsbLocator> {
        let failure = match locator.as_ref() {
            None => Some(FailureCode::TopologyUnavailable),
            Some(locator) if !locator.is_complete() => Some(FailureCode::TopologyUnavailable),
            Some(locator) if !locator.speed.supports_camera_upload() => {
                Some(FailureCode::UnsupportedLinkSpeed)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            self.record_without_transition(
                at,
                RecordLevel::Error,
                RecordKind::TopologyRejected,
                None,
                Some(failure),
                records,
            );
            None
        } else {
            locator
        }
    }

    fn ignored(&mut self, at: Duration, records: &mut Vec<ServiceRecord>) {
        self.record_without_transition(
            at,
            RecordLevel::Info,
            RecordKind::DuplicateIgnored,
            None,
            None,
            records,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn change_state(
        &mut self,
        state: ServiceState,
        at: Duration,
        level: RecordLevel,
        kind: RecordKind,
        attempt: Option<u32>,
        failure: Option<FailureCode>,
        records: &mut Vec<ServiceRecord>,
    ) {
        let from = self.state.name();
        let to = state.name();
        self.state = state;
        self.sequence += 1;
        records.push(ServiceRecord {
            sequence: self.sequence,
            at,
            level,
            kind,
            from,
            to,
            attempt,
            failure,
        });
    }

    fn record_without_transition(
        &mut self,
        at: Duration,
        level: RecordLevel,
        kind: RecordKind,
        attempt: Option<u32>,
        failure: Option<FailureCode>,
        records: &mut Vec<ServiceRecord>,
    ) {
        let state = self.state.name();
        self.sequence += 1;
        records.push(ServiceRecord {
            sequence: self.sequence,
            at,
            level,
            kind,
            from: state,
            to: state,
            attempt,
            failure,
        });
    }
}

fn validate_config(config: ServiceConfig) -> Result<(), ServiceConfigError> {
    if config.max_attempts == 0 {
        return Err(ServiceConfigError::ZeroAttempts);
    }
    if config.initial_backoff.is_zero()
        || config.maximum_backoff.is_zero()
        || config.reenumeration_timeout.is_zero()
    {
        return Err(ServiceConfigError::ZeroDuration);
    }
    if config.maximum_backoff < config.initial_backoff {
        return Err(ServiceConfigError::InvalidBackoffRange);
    }
    if config.upload_limits.maximum_image_bytes == 0
        || config.upload_limits.transfer_timeout.is_zero()
        || config.upload_limits.total_timeout.is_zero()
    {
        return Err(ServiceConfigError::InvalidUploadLimits);
    }
    if config.upload_limits.transfer_timeout > config.upload_limits.total_timeout {
        return Err(ServiceConfigError::TransferTimeoutExceedsTotal);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum HostReadiness {
    Ready,
    UnsupportedPlatform,
    FirmwareUnavailable,
}

pub const fn host_readiness() -> HostReadiness {
    if !cfg!(target_os = "windows") {
        HostReadiness::UnsupportedPlatform
    } else {
        HostReadiness::Ready
    }
}

/// Placeholder for the eventual Windows `RegisterDeviceNotification`/SCM
/// adapter. Construction fails explicitly instead of falling back to polling.
#[derive(Debug)]
pub struct WindowsDeviceEventSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum WindowsAdapterError {
    #[error("the Windows device-notification adapter is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("the Windows device-notification/SCM adapter has not been connected yet")]
    NotImplemented,
}

impl WindowsDeviceEventSource {
    pub fn open() -> Result<Self, WindowsAdapterError> {
        if cfg!(target_os = "windows") {
            Err(WindowsAdapterError::NotImplemented)
        } else {
            Err(WindowsAdapterError::UnsupportedPlatform)
        }
    }
}

#[derive(Debug, Error)]
pub enum ServiceHostError {
    #[error("device event source failed: {0}")]
    EventSource(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("structured event sink failed: {0}")]
    EventSink(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("device event source ended before service cancellation")]
    EventSourceEnded,
}

pub fn run_service<S, U, L>(
    engine: &mut ServiceEngine,
    source: &mut S,
    uploader: &mut U,
    sink: &mut L,
    cancellation: &dyn CancellationSignal,
) -> Result<(), ServiceHostError>
where
    S: DeviceEventSource,
    U: FirmwareUploader,
    L: StructuredEventSink,
{
    let mut observer = NoopUvcLifecycleObserver;
    run_service_with_uvc_observer(engine, source, uploader, sink, cancellation, &mut observer)
}

pub fn run_service_with_uvc_observer<S, U, L, O>(
    engine: &mut ServiceEngine,
    source: &mut S,
    uploader: &mut U,
    sink: &mut L,
    cancellation: &dyn CancellationSignal,
    observer: &mut O,
) -> Result<(), ServiceHostError>
where
    S: DeviceEventSource,
    U: FirmwareUploader,
    L: StructuredEventSink,
    O: UvcLifecycleObserver,
{
    loop {
        if cancellation.is_cancelled() {
            let records = engine.handle_with_uvc_observer(
                DeviceEvent::Cancel {
                    at: engine.last_event_time(),
                },
                uploader,
                cancellation,
                observer,
            );
            for record in records {
                sink.emit(&record).map_err(ServiceHostError::EventSink)?;
            }
            return Ok(());
        }

        let event = source
            .next_event(cancellation)
            .map_err(ServiceHostError::EventSource)?
            .ok_or(ServiceHostError::EventSourceEnded)?;
        let records = engine.handle_with_uvc_observer(event, uploader, cancellation, observer);
        for record in records {
            sink.emit(&record).map_err(ServiceHostError::EventSink)?;
        }
        if matches!(engine.state(), ServiceState::Stopped) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests;
