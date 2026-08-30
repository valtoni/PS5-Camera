//! Safe, composable implementation of the OV580 bootloader upload protocol.
//!
//! This crate intentionally contains no CLI, file-system access, or process
//! termination. Callers decide where firmware comes from and how errors are
//! presented. USB access is behind [`UsbTransport`], while re-enumeration is
//! behind [`ReenumerationBackend`], so both state machines can be tested
//! without a camera.

use ps5cam_usb::{DeviceMode, OV580_BOOT_PID, OV580_CAMERA_PID, OV580_VENDOR_ID};
use rusb::{Device, DeviceHandle, GlobalContext};
use std::{
    fmt,
    time::{Duration, Instant},
};
use thiserror::Error;

pub const CHUNK_SIZE: usize = 512;
pub const MIN_FIRMWARE_SIZE: usize = 64 * 1024;
pub const REQUEST_TYPE_VENDOR_DEVICE_OUT: u8 = 0x40;
pub const UPLOAD_REQUEST: u8 = 0x00;
pub const FIRST_MEMORY_BANK: u16 = 0x14;
pub const EXECUTE_VALUE: u16 = 0x2200;
pub const EXECUTE_INDEX: u16 = 0x8018;
pub const EXECUTE_BYTE: u8 = 0x5b;

/// Documented OV580 program-memory capacity (96 KiB).
///
/// This is intentionally distinct from [`MAX_PROTOCOL_OFFSET`]: the vendor
/// request fields can address a much larger range than the processor's known
/// program memory.
pub const OV580_PROGRAM_MEMORY_SIZE: u64 = 96 * 1024;

/// Largest absolute byte offset representable by the OV580 request fields.
pub const MAX_PROTOCOL_OFFSET: u64 =
    ((u16::MAX - FIRST_MEMORY_BANK) as u64) * 65_536 + u16::MAX as u64;
/// Largest complete firmware image accepted for the OV580 program memory.
pub const MAX_FIRMWARE_SIZE: u64 = OV580_PROGRAM_MEMORY_SIZE;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FirmwareImageError {
    #[error("firmware is too small: expected at least {minimum} bytes, got {actual}")]
    TooSmall { minimum: usize, actual: usize },
    #[error(
        "firmware exceeds the documented OV580 program memory: maximum {maximum} bytes, got {actual}"
    )]
    TooLarge { maximum: u64, actual: u64 },
}

/// An owned firmware image whose complete byte range can be addressed.
#[derive(Clone, PartialEq, Eq)]
pub struct FirmwareImage {
    bytes: Box<[u8]>,
}

impl fmt::Debug for FirmwareImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FirmwareImage")
            .field("length", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

impl FirmwareImage {
    /// Validates the historical full-image lower bound of exactly 64 KiB.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, FirmwareImageError> {
        let bytes = bytes.into();
        validate_image_length(bytes.len())?;
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn chunks(&self) -> FirmwareChunks<'_> {
        FirmwareChunks {
            bytes: &self.bytes,
            offset: 0,
        }
    }
}

fn validate_image_length(length: usize) -> Result<(), FirmwareImageError> {
    if length < MIN_FIRMWARE_SIZE {
        return Err(FirmwareImageError::TooSmall {
            minimum: MIN_FIRMWARE_SIZE,
            actual: length,
        });
    }

    let length = length as u64;
    if length > MAX_FIRMWARE_SIZE {
        return Err(FirmwareImageError::TooLarge {
            maximum: MAX_FIRMWARE_SIZE,
            actual: length,
        });
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolAddress {
    pub value: u16,
    pub index: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("offset {offset} exceeds the last OV580 memory bank")]
pub struct AddressOverflow {
    pub offset: u64,
}

/// Maps an absolute image offset to the vendor request's `wValue`/`wIndex`.
pub fn protocol_address(offset: u64) -> Result<ProtocolAddress, AddressOverflow> {
    let bank = offset >> 16;
    let bank = u16::try_from(bank).map_err(|_| AddressOverflow { offset })?;
    let index = FIRST_MEMORY_BANK
        .checked_add(bank)
        .ok_or(AddressOverflow { offset })?;

    Ok(ProtocolAddress {
        value: offset as u16,
        index,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareChunk<'a> {
    pub offset: u64,
    pub address: ProtocolAddress,
    pub bytes: &'a [u8],
}

pub struct FirmwareChunks<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for FirmwareChunks<'a> {
    type Item = FirmwareChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset == self.bytes.len() {
            return None;
        }

        let start = self.offset;
        let end = start.saturating_add(CHUNK_SIZE).min(self.bytes.len());
        self.offset = end;
        let offset = start as u64;

        // FirmwareImage validation proves that every yielded offset is valid.
        let address = protocol_address(offset).expect("validated firmware offset");
        Some(FirmwareChunk {
            offset,
            address,
            bytes: &self.bytes[start..end],
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.bytes.len() - self.offset;
        let chunks = remaining.div_ceil(CHUNK_SIZE);
        (chunks, Some(chunks))
    }
}

impl ExactSizeIterator for FirmwareChunks<'_> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Windows,
    Linux,
    Other,
}

impl HostPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfacePolicy {
    /// Windows tolerates `NotSupported`; Linux requires kernel-driver calls to
    /// succeed. Other targets claim the interface directly.
    PlatformDefault,
    ClaimOnly,
    DetachKernelDriverIfActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoaderConfig {
    pub interface: u8,
    pub transfer_timeout: Duration,
    pub interface_policy: InterfacePolicy,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            interface: 0,
            transfer_timeout: Duration::from_secs(1),
            interface_policy: InterfacePolicy::PlatformDefault,
        }
    }
}

pub trait UsbTransport {
    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, rusb::Error>;
    fn detach_kernel_driver(&mut self, interface: u8) -> Result<(), rusb::Error>;
    fn claim_interface(&mut self, interface: u8) -> Result<(), rusb::Error>;
    fn release_interface(&mut self, interface: u8) -> Result<(), rusb::Error>;
    fn write_control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
        timeout: Duration,
    ) -> Result<usize, rusb::Error>;
}

impl UsbTransport for DeviceHandle<GlobalContext> {
    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, rusb::Error> {
        DeviceHandle::kernel_driver_active(self, interface)
    }

    fn detach_kernel_driver(&mut self, interface: u8) -> Result<(), rusb::Error> {
        DeviceHandle::detach_kernel_driver(self, interface)
    }

    fn claim_interface(&mut self, interface: u8) -> Result<(), rusb::Error> {
        DeviceHandle::claim_interface(self, interface)
    }

    fn release_interface(&mut self, interface: u8) -> Result<(), rusb::Error> {
        DeviceHandle::release_interface(self, interface)
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
        DeviceHandle::write_control(self, request_type, request, value, index, data, timeout)
    }
}

pub trait CancellationCheck {
    fn is_cancelled(&self) -> bool;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NeverCancel;

impl CancellationCheck for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadReport {
    pub bytes_uploaded: u64,
    pub chunks_uploaded: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecuteDisposition {
    CommandAccepted,
    DeviceDisconnected,
}

/// Stable libusb coordinates captured during the read-only preflight.
///
/// The device address is deliberately included: reconnecting the camera can
/// preserve the physical port while creating a different USB device.  Callers
/// must repeat preflight when any coordinate changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDeviceLocator {
    pub controller_id: String,
    pub bus_number: u8,
    pub device_address: u8,
    pub port_path: Vec<u8>,
}

impl BootDeviceLocator {
    pub fn matches_observation(
        &self,
        vendor_id: u16,
        product_id: u16,
        controller_id: &str,
        bus_number: u8,
        device_address: u8,
        port_path: &[u8],
    ) -> bool {
        vendor_id == OV580_VENDOR_ID
            && product_id == OV580_BOOT_PID
            && controller_id == self.controller_id
            && bus_number == self.bus_number
            && device_address == self.device_address
            && port_path == self.port_path
    }
}

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("transfer timeout must be non-zero")]
    ZeroTransferTimeout,
    #[error("failed to enumerate USB devices: {0}")]
    Enumeration(#[source] rusb::Error),
    #[error("failed to read USB device descriptor: {0}")]
    DeviceDescriptor(#[source] rusb::Error),
    #[error("failed to open OV580 boot device: {0}")]
    Open(#[source] rusb::Error),
    #[error("camera is already in camera mode")]
    AlreadyCameraMode,
    #[error("OV580 boot or camera device was not found")]
    DeviceNotFound,
    #[error(
        "preflight boot device is no longer present at controller {controller_id} bus {bus_number} address {device_address} port {port_path:?}"
    )]
    TargetNotFound {
        controller_id: String,
        bus_number: u8,
        device_address: u8,
        port_path: Vec<u8>,
    },
    #[error("failed to read USB port path while revalidating the preflight target: {0}")]
    TargetPortPath(#[source] rusb::Error),
    #[error(
        "device identity changed after preflight: expected 05a9:0580 at controller {expected_controller} bus {expected_bus} address {expected_address} port {expected_port:?}, observed {observed_vid:04x}:{observed_pid:04x} at controller {observed_controller} bus {observed_bus} address {observed_address} port {observed_port:?}"
    )]
    TargetIdentityChanged {
        expected_controller: String,
        expected_bus: u8,
        expected_address: u8,
        expected_port: Vec<u8>,
        observed_vid: u16,
        observed_pid: u16,
        observed_controller: String,
        observed_bus: u8,
        observed_address: u8,
        observed_port: Vec<u8>,
    },
    #[error("failed to query kernel driver on interface {interface}: {source}")]
    KernelDriverStatus {
        interface: u8,
        #[source]
        source: rusb::Error,
    },
    #[error("failed to detach kernel driver from interface {interface}: {source}")]
    DetachKernelDriver {
        interface: u8,
        #[source]
        source: rusb::Error,
    },
    #[error("failed to claim interface {interface}: {source}")]
    ClaimInterface {
        interface: u8,
        #[source]
        source: rusb::Error,
    },
    #[error("failed to release interface {interface}: {source}")]
    ReleaseInterface {
        interface: u8,
        #[source]
        source: rusb::Error,
    },
    #[error("upload was cancelled after {bytes_uploaded} bytes")]
    Cancelled { bytes_uploaded: u64 },
    #[error("USB upload failed at offset {offset}: {source}")]
    UploadTransfer {
        offset: u64,
        #[source]
        source: rusb::Error,
    },
    #[error("short USB write at offset {offset}: expected {expected} bytes, wrote {actual}")]
    ShortWrite {
        offset: u64,
        expected: usize,
        actual: usize,
    },
    #[error("execute command failed: {0}")]
    ExecuteTransfer(#[source] rusb::Error),
    #[error("short execute write: expected 1 byte, wrote {actual}")]
    ShortExecuteWrite { actual: usize },
}

pub struct Ov580BootDevice<T: UsbTransport> {
    transport: T,
    config: LoaderConfig,
    claimed: bool,
}

impl Ov580BootDevice<DeviceHandle<GlobalContext>> {
    pub fn open(config: LoaderConfig) -> Result<Self, LoaderError> {
        let devices = rusb::devices().map_err(LoaderError::Enumeration)?;
        let mut camera_present = false;

        for device in devices.iter() {
            let descriptor = device
                .device_descriptor()
                .map_err(LoaderError::DeviceDescriptor)?;
            let Some(mode) = DeviceMode::from_ids(descriptor.vendor_id(), descriptor.product_id())
            else {
                continue;
            };

            match mode {
                DeviceMode::Boot => {
                    let handle = device.open().map_err(LoaderError::Open)?;
                    return Self::from_transport(handle, config);
                }
                DeviceMode::Camera => camera_present = true,
            }
        }

        if camera_present {
            Err(LoaderError::AlreadyCameraMode)
        } else {
            Err(LoaderError::DeviceNotFound)
        }
    }

    /// Opens only the exact boot device captured by preflight.
    pub fn open_at(locator: &BootDeviceLocator, config: LoaderConfig) -> Result<Self, LoaderError> {
        let devices = rusb::devices().map_err(LoaderError::Enumeration)?;

        for device in devices.iter() {
            if !device_coordinates_match(&device, locator)? {
                continue;
            }
            validate_device_identity(&device, locator)?;
            let handle = device.open().map_err(LoaderError::Open)?;
            let opened = Self::from_transport(handle, config)?;
            opened.revalidate_target(locator)?;
            return Ok(opened);
        }

        Err(LoaderError::TargetNotFound {
            controller_id: locator.controller_id.clone(),
            bus_number: locator.bus_number,
            device_address: locator.device_address,
            port_path: locator.port_path.clone(),
        })
    }

    /// Revalidates VID/PID and every libusb coordinate on the opened handle.
    pub fn revalidate_target(&self, locator: &BootDeviceLocator) -> Result<(), LoaderError> {
        validate_device_identity(&self.transport.device(), locator)
    }

    /// Performs the final identity check immediately before upload iteration.
    pub fn upload_at_with_cancellation<C: CancellationCheck>(
        &mut self,
        locator: &BootDeviceLocator,
        image: &FirmwareImage,
        cancellation: &C,
    ) -> Result<UploadReport, LoaderError> {
        self.revalidate_target(locator)?;
        self.upload_with_cancellation(image, cancellation)
    }
}

fn device_coordinates_match(
    device: &Device<GlobalContext>,
    locator: &BootDeviceLocator,
) -> Result<bool, LoaderError> {
    if format!("libusb-bus-{}", device.bus_number()) != locator.controller_id
        || device.bus_number() != locator.bus_number
        || device.address() != locator.device_address
    {
        return Ok(false);
    }
    let port_path = device.port_numbers().map_err(LoaderError::TargetPortPath)?;
    Ok(port_path == locator.port_path)
}

fn validate_device_identity(
    device: &Device<GlobalContext>,
    locator: &BootDeviceLocator,
) -> Result<(), LoaderError> {
    let descriptor = device
        .device_descriptor()
        .map_err(LoaderError::DeviceDescriptor)?;
    let observed_port = device.port_numbers().map_err(LoaderError::TargetPortPath)?;
    let matches = locator.matches_observation(
        descriptor.vendor_id(),
        descriptor.product_id(),
        &format!("libusb-bus-{}", device.bus_number()),
        device.bus_number(),
        device.address(),
        &observed_port,
    );
    if matches {
        return Ok(());
    }
    Err(LoaderError::TargetIdentityChanged {
        expected_controller: locator.controller_id.clone(),
        expected_bus: locator.bus_number,
        expected_address: locator.device_address,
        expected_port: locator.port_path.clone(),
        observed_vid: descriptor.vendor_id(),
        observed_pid: descriptor.product_id(),
        observed_controller: format!("libusb-bus-{}", device.bus_number()),
        observed_bus: device.bus_number(),
        observed_address: device.address(),
        observed_port,
    })
}

impl<T: UsbTransport> Ov580BootDevice<T> {
    pub fn from_transport(mut transport: T, config: LoaderConfig) -> Result<Self, LoaderError> {
        if config.transfer_timeout.is_zero() {
            return Err(LoaderError::ZeroTransferTimeout);
        }

        prepare_interface(
            &mut transport,
            config.interface,
            config.interface_policy,
            HostPlatform::current(),
        )?;

        Ok(Self {
            transport,
            config,
            claimed: true,
        })
    }

    pub fn upload(&mut self, image: &FirmwareImage) -> Result<UploadReport, LoaderError> {
        self.upload_with_cancellation(image, &NeverCancel)
    }

    pub fn upload_with_cancellation<C: CancellationCheck>(
        &mut self,
        image: &FirmwareImage,
        cancellation: &C,
    ) -> Result<UploadReport, LoaderError> {
        let mut report = UploadReport {
            bytes_uploaded: 0,
            chunks_uploaded: 0,
        };

        for chunk in image.chunks() {
            if cancellation.is_cancelled() {
                return Err(LoaderError::Cancelled {
                    bytes_uploaded: report.bytes_uploaded,
                });
            }

            let actual = self
                .transport
                .write_control(
                    REQUEST_TYPE_VENDOR_DEVICE_OUT,
                    UPLOAD_REQUEST,
                    chunk.address.value,
                    chunk.address.index,
                    chunk.bytes,
                    self.config.transfer_timeout,
                )
                .map_err(|source| LoaderError::UploadTransfer {
                    offset: chunk.offset,
                    source,
                })?;

            if actual != chunk.bytes.len() {
                return Err(LoaderError::ShortWrite {
                    offset: chunk.offset,
                    expected: chunk.bytes.len(),
                    actual,
                });
            }

            report.bytes_uploaded += actual as u64;
            report.chunks_uploaded += 1;
        }

        Ok(report)
    }

    pub fn execute(&mut self) -> Result<ExecuteDisposition, LoaderError> {
        self.execute_with_cancellation(&NeverCancel, 0)
    }

    /// Checks the same operational guard used during upload immediately before
    /// issuing the execute control transfer. `bytes_uploaded` is retained in a
    /// cancellation error so callers can distinguish the post-final-chunk
    /// window from a partial upload.
    pub fn execute_with_cancellation<C: CancellationCheck>(
        &mut self,
        cancellation: &C,
        bytes_uploaded: u64,
    ) -> Result<ExecuteDisposition, LoaderError> {
        if cancellation.is_cancelled() {
            return Err(LoaderError::Cancelled { bytes_uploaded });
        }
        let result = self.transport.write_control(
            REQUEST_TYPE_VENDOR_DEVICE_OUT,
            UPLOAD_REQUEST,
            EXECUTE_VALUE,
            EXECUTE_INDEX,
            &[EXECUTE_BYTE],
            self.config.transfer_timeout,
        );

        match result {
            Ok(1) => Ok(ExecuteDisposition::CommandAccepted),
            Ok(actual) => Err(LoaderError::ShortExecuteWrite { actual }),
            Err(rusb::Error::NoDevice) => {
                self.claimed = false;
                Ok(ExecuteDisposition::DeviceDisconnected)
            }
            Err(error) => Err(LoaderError::ExecuteTransfer(error)),
        }
    }

    pub fn release(mut self) -> Result<(), LoaderError> {
        if self.claimed {
            self.transport
                .release_interface(self.config.interface)
                .map_err(|source| LoaderError::ReleaseInterface {
                    interface: self.config.interface,
                    source,
                })?;
            self.claimed = false;
        }
        Ok(())
    }
}

impl<T: UsbTransport> Drop for Ov580BootDevice<T> {
    fn drop(&mut self) {
        if self.claimed {
            let _ = self.transport.release_interface(self.config.interface);
            self.claimed = false;
        }
    }
}

fn prepare_interface<T: UsbTransport>(
    transport: &mut T,
    interface: u8,
    policy: InterfacePolicy,
    platform: HostPlatform,
) -> Result<(), LoaderError> {
    let (query_kernel_driver, ignore_not_supported) = match policy {
        InterfacePolicy::ClaimOnly => (false, false),
        InterfacePolicy::DetachKernelDriverIfActive => (true, false),
        InterfacePolicy::PlatformDefault => match platform {
            HostPlatform::Windows => (true, true),
            HostPlatform::Linux => (true, false),
            HostPlatform::Other => (false, false),
        },
    };

    if query_kernel_driver {
        let active = match transport.kernel_driver_active(interface) {
            Ok(active) => active,
            Err(rusb::Error::NotSupported) if ignore_not_supported => false,
            Err(source) => {
                return Err(LoaderError::KernelDriverStatus { interface, source });
            }
        };

        if active {
            match transport.detach_kernel_driver(interface) {
                Ok(()) => {}
                Err(rusb::Error::NotSupported) if ignore_not_supported => {}
                Err(source) => {
                    return Err(LoaderError::DetachKernelDriver { interface, source });
                }
            }
        }
    }

    transport
        .claim_interface(interface)
        .map_err(|source| LoaderError::ClaimInterface { interface, source })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedDeviceState {
    Absent,
    Boot,
    Camera,
    BootAndCamera,
}

impl ObservedDeviceState {
    fn has_camera(self) -> bool {
        matches!(self, Self::Camera | Self::BootAndCamera)
    }
}

pub trait ReenumerationBackend {
    type Error: std::error::Error + Send + Sync + 'static;

    fn observe(&mut self) -> Result<ObservedDeviceState, Self::Error>;
    fn elapsed(&self) -> Duration;
    fn wait(&mut self, duration: Duration);
}

#[derive(Debug)]
pub struct SystemReenumerationBackend {
    started: Instant,
}

impl Default for SystemReenumerationBackend {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl ReenumerationBackend for SystemReenumerationBackend {
    type Error = rusb::Error;

    fn observe(&mut self) -> Result<ObservedDeviceState, Self::Error> {
        let devices = rusb::devices()?;
        let mut boot = false;
        let mut camera = false;

        for device in devices.iter() {
            let descriptor = device.device_descriptor()?;
            if descriptor.vendor_id() != OV580_VENDOR_ID {
                continue;
            }
            match descriptor.product_id() {
                OV580_BOOT_PID => boot = true,
                OV580_CAMERA_PID => camera = true,
                _ => {}
            }
        }

        Ok(match (boot, camera) {
            (false, false) => ObservedDeviceState::Absent,
            (true, false) => ObservedDeviceState::Boot,
            (false, true) => ObservedDeviceState::Camera,
            (true, true) => ObservedDeviceState::BootAndCamera,
        })
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReenumerationConfig {
    pub timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for ReenumerationConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            poll_interval: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReenumerationOutcome {
    CameraReady { elapsed: Duration },
    AlreadyCamera { elapsed: Duration },
}

#[derive(Debug, Error)]
pub enum ReenumerationError<E: std::error::Error + 'static> {
    #[error("re-enumeration timeout and poll interval must both be non-zero")]
    InvalidTiming,
    #[error("failed to observe USB re-enumeration: {0}")]
    Observe(#[source] E),
    #[error("camera did not re-enumerate within {timeout:?}; last state was {last_state:?}")]
    Timeout {
        timeout: Duration,
        last_state: ObservedDeviceState,
    },
}

pub fn wait_for_camera_mode<B: ReenumerationBackend>(
    backend: &mut B,
    config: ReenumerationConfig,
) -> Result<ReenumerationOutcome, ReenumerationError<B::Error>> {
    if config.timeout.is_zero() || config.poll_interval.is_zero() {
        return Err(ReenumerationError::InvalidTiming);
    }

    let initial = backend.observe().map_err(ReenumerationError::Observe)?;
    if initial.has_camera() {
        return Ok(ReenumerationOutcome::AlreadyCamera {
            elapsed: backend.elapsed(),
        });
    }

    let mut last_state = initial;
    loop {
        let elapsed = backend.elapsed();
        if elapsed >= config.timeout {
            return Err(ReenumerationError::Timeout {
                timeout: config.timeout,
                last_state,
            });
        }

        backend.wait(config.poll_interval.min(config.timeout - elapsed));
        last_state = backend.observe().map_err(ReenumerationError::Observe)?;
        if last_state.has_camera() {
            return Ok(ReenumerationOutcome::CameraReady {
                elapsed: backend.elapsed(),
            });
        }
    }
}

#[cfg(test)]
mod tests;
