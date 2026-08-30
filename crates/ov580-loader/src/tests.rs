use super::*;
use std::{
    cell::Cell,
    collections::VecDeque,
    convert::Infallible,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Write {
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    data: Vec<u8>,
    timeout: Duration,
}

#[derive(Debug)]
struct FakeTransport {
    kernel_driver_result: Result<bool, rusb::Error>,
    detach_result: Result<(), rusb::Error>,
    claim_result: Result<(), rusb::Error>,
    release_result: Result<(), rusb::Error>,
    write_results: VecDeque<Result<usize, rusb::Error>>,
    writes: Vec<Write>,
    kernel_queries: usize,
    detaches: usize,
    claims: usize,
    releases: usize,
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self {
            kernel_driver_result: Ok(false),
            detach_result: Ok(()),
            claim_result: Ok(()),
            release_result: Ok(()),
            write_results: VecDeque::new(),
            writes: Vec::new(),
            kernel_queries: 0,
            detaches: 0,
            claims: 0,
            releases: 0,
        }
    }
}

impl UsbTransport for FakeTransport {
    fn kernel_driver_active(&mut self, _interface: u8) -> Result<bool, rusb::Error> {
        self.kernel_queries += 1;
        self.kernel_driver_result
    }

    fn detach_kernel_driver(&mut self, _interface: u8) -> Result<(), rusb::Error> {
        self.detaches += 1;
        self.detach_result
    }

    fn claim_interface(&mut self, _interface: u8) -> Result<(), rusb::Error> {
        self.claims += 1;
        self.claim_result
    }

    fn release_interface(&mut self, _interface: u8) -> Result<(), rusb::Error> {
        self.releases += 1;
        self.release_result
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
        self.writes.push(Write {
            request_type,
            request,
            value,
            index,
            data: data.to_vec(),
            timeout,
        });
        self.write_results.pop_front().unwrap_or(Ok(data.len()))
    }
}

fn image(length: usize) -> FirmwareImage {
    let bytes = (0..length).map(|index| index as u8).collect::<Vec<_>>();
    FirmwareImage::from_bytes(bytes).expect("valid fixture")
}

fn test_config() -> LoaderConfig {
    LoaderConfig {
        interface_policy: InterfacePolicy::ClaimOnly,
        ..LoaderConfig::default()
    }
}

#[test]
fn exact_boot_locator_rejects_every_toctou_identity_change() {
    let locator = BootDeviceLocator {
        controller_id: "libusb-bus-1".to_owned(),
        bus_number: 1,
        device_address: 7,
        port_path: vec![3, 2],
    };
    assert!(locator.matches_observation(0x05a9, 0x0580, "libusb-bus-1", 1, 7, &[3, 2]));
    assert!(!locator.matches_observation(0x05a9, 0x058c, "libusb-bus-1", 1, 7, &[3, 2]));
    assert!(!locator.matches_observation(0x05a9, 0x0580, "libusb-bus-2", 1, 7, &[3, 2]));
    assert!(!locator.matches_observation(0x05a9, 0x0580, "libusb-bus-1", 2, 7, &[3, 2]));
    assert!(!locator.matches_observation(0x05a9, 0x0580, "libusb-bus-1", 1, 8, &[3, 2]));
    assert!(!locator.matches_observation(0x05a9, 0x0580, "libusb-bus-1", 1, 7, &[3, 4]));
}

#[test]
fn image_rejects_65535_and_accepts_65536_bytes() {
    assert_eq!(
        FirmwareImage::from_bytes(vec![0; 65_535]),
        Err(FirmwareImageError::TooSmall {
            minimum: 65_536,
            actual: 65_535,
        })
    );
    assert_eq!(
        FirmwareImage::from_bytes(vec![0; 65_536])
            .expect("64 KiB image")
            .len(),
        65_536
    );
}

#[test]
fn image_length_is_bounded_by_documented_program_memory() {
    assert_eq!(MAX_FIRMWARE_SIZE, 96 * 1024);
    assert_eq!(validate_image_length(MAX_FIRMWARE_SIZE as usize), Ok(()));
    assert_eq!(
        validate_image_length((MAX_FIRMWARE_SIZE + 1) as usize),
        Err(FirmwareImageError::TooLarge {
            maximum: MAX_FIRMWARE_SIZE,
            actual: MAX_FIRMWARE_SIZE + 1,
        })
    );
}

#[test]
fn physical_image_limit_does_not_narrow_generic_protocol_addressing() {
    assert_eq!(
        protocol_address(MAX_FIRMWARE_SIZE - 1).unwrap(),
        ProtocolAddress {
            value: 0x7fff,
            index: 0x15,
        }
    );
    assert_eq!(
        protocol_address(MAX_FIRMWARE_SIZE).unwrap(),
        ProtocolAddress {
            value: 0x8000,
            index: 0x15,
        }
    );
}

#[test]
fn addresses_cross_65535_65536_boundary() {
    assert_eq!(
        protocol_address(65_535).unwrap(),
        ProtocolAddress {
            value: 0xffff,
            index: 0x14,
        }
    );
    assert_eq!(
        protocol_address(65_536).unwrap(),
        ProtocolAddress {
            value: 0,
            index: 0x15,
        }
    );
}

#[test]
fn addresses_multiple_banks_without_two_bank_assumption() {
    assert_eq!(protocol_address(0).unwrap().index, 0x14);
    assert_eq!(
        protocol_address(3 * 65_536 + 0x1234).unwrap(),
        ProtocolAddress {
            value: 0x1234,
            index: 0x17,
        }
    );
    assert_eq!(
        protocol_address(MAX_PROTOCOL_OFFSET).unwrap(),
        ProtocolAddress {
            value: 0xffff,
            index: 0xffff,
        }
    );
    assert_eq!(
        protocol_address(MAX_PROTOCOL_OFFSET + 1),
        Err(AddressOverflow {
            offset: MAX_PROTOCOL_OFFSET + 1,
        })
    );
}

#[test]
fn chunks_are_512_bytes_and_preserve_short_last_chunk() {
    let image = image(MIN_FIRMWARE_SIZE + 13);
    let chunks = image.chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 129);
    assert!(chunks[..128]
        .iter()
        .all(|chunk| chunk.bytes.len() == CHUNK_SIZE));
    let last = chunks.last().unwrap();
    assert_eq!(last.offset, 65_536);
    assert_eq!(last.bytes.len(), 13);
    assert_eq!(
        last.address,
        ProtocolAddress {
            value: 0,
            index: 0x15
        }
    );
}

#[test]
fn chunk_iterator_reports_exact_remaining_length() {
    let image = image(MIN_FIRMWARE_SIZE + 1);
    let mut chunks = image.chunks();
    assert_eq!(chunks.len(), 129);
    chunks.next();
    assert_eq!(chunks.len(), 128);
}

#[test]
fn upload_uses_vendor_requests_generic_addresses_and_timeout() {
    let mut device = Ov580BootDevice::from_transport(FakeTransport::default(), test_config())
        .expect("prepare fake");
    let report = device
        .upload(&image(MIN_FIRMWARE_SIZE + 1))
        .expect("upload");

    assert_eq!(report.bytes_uploaded, 65_537);
    assert_eq!(report.chunks_uploaded, 129);
    assert_eq!(device.transport.writes.len(), 129);
    assert_eq!(
        device.transport.writes[128],
        Write {
            request_type: 0x40,
            request: 0,
            value: 0,
            index: 0x15,
            data: vec![0],
            timeout: Duration::from_secs(1),
        }
    );
}

#[test]
fn upload_rejects_short_write_at_exact_offset() {
    let mut transport = FakeTransport::default();
    transport.write_results.push_back(Ok(CHUNK_SIZE));
    transport.write_results.push_back(Ok(CHUNK_SIZE - 1));
    let mut device = Ov580BootDevice::from_transport(transport, test_config()).unwrap();

    assert!(matches!(
        device.upload(&image(MIN_FIRMWARE_SIZE)),
        Err(LoaderError::ShortWrite {
            offset: 512,
            expected: 512,
            actual: 511,
        })
    ));
    assert_eq!(device.transport.writes.len(), 2);
}

struct CancelAfter {
    checks: AtomicUsize,
    allowed_checks: usize,
}

impl CancellationCheck for CancelAfter {
    fn is_cancelled(&self) -> bool {
        self.checks.fetch_add(1, Ordering::Relaxed) >= self.allowed_checks
    }
}

#[test]
fn upload_checks_cancellation_between_chunks() {
    let mut device =
        Ov580BootDevice::from_transport(FakeTransport::default(), test_config()).unwrap();
    let cancellation = CancelAfter {
        checks: AtomicUsize::new(0),
        allowed_checks: 2,
    };

    assert!(matches!(
        device.upload_with_cancellation(&image(MIN_FIRMWARE_SIZE), &cancellation),
        Err(LoaderError::Cancelled {
            bytes_uploaded: 1024
        })
    ));
    assert_eq!(device.transport.writes.len(), 2);
}

#[test]
fn execute_rechecks_guard_after_the_final_uploaded_chunk() {
    let mut device =
        Ov580BootDevice::from_transport(FakeTransport::default(), test_config()).unwrap();
    let cancellation = CancelAfter {
        checks: AtomicUsize::new(0),
        allowed_checks: 128,
    };
    let report = device
        .upload_with_cancellation(&image(MIN_FIRMWARE_SIZE), &cancellation)
        .expect("all firmware chunks are allowed");

    assert_eq!(report.bytes_uploaded, MIN_FIRMWARE_SIZE as u64);
    assert_eq!(device.transport.writes.len(), 128);
    assert!(matches!(
        device.execute_with_cancellation(&cancellation, report.bytes_uploaded),
        Err(LoaderError::Cancelled {
            bytes_uploaded: 65_536
        })
    ));
    assert_eq!(device.transport.writes.len(), 128);
}

#[test]
fn execute_accepts_no_device_as_expected_disconnect() {
    let mut transport = FakeTransport::default();
    transport
        .write_results
        .push_back(Err(rusb::Error::NoDevice));
    let mut device = Ov580BootDevice::from_transport(transport, test_config()).unwrap();

    assert_eq!(
        device.execute().unwrap(),
        ExecuteDisposition::DeviceDisconnected
    );
    assert_eq!(device.transport.writes[0].value, EXECUTE_VALUE);
    assert_eq!(device.transport.writes[0].index, EXECUTE_INDEX);
    assert_eq!(device.transport.writes[0].data, [EXECUTE_BYTE]);
    assert!(!device.claimed);
}

#[test]
fn execute_detects_short_write_and_propagates_other_usb_errors() {
    let mut short = FakeTransport::default();
    short.write_results.push_back(Ok(0));
    let mut device = Ov580BootDevice::from_transport(short, test_config()).unwrap();
    assert!(matches!(
        device.execute(),
        Err(LoaderError::ShortExecuteWrite { actual: 0 })
    ));

    let mut failed = FakeTransport::default();
    failed.write_results.push_back(Err(rusb::Error::Timeout));
    let mut device = Ov580BootDevice::from_transport(failed, test_config()).unwrap();
    assert!(matches!(
        device.execute(),
        Err(LoaderError::ExecuteTransfer(rusb::Error::Timeout))
    ));
}

#[test]
fn windows_default_ignores_not_supported_and_claims() {
    let mut transport = FakeTransport {
        kernel_driver_result: Err(rusb::Error::NotSupported),
        ..FakeTransport::default()
    };
    prepare_interface(
        &mut transport,
        0,
        InterfacePolicy::PlatformDefault,
        HostPlatform::Windows,
    )
    .unwrap();
    assert_eq!(transport.kernel_queries, 1);
    assert_eq!(transport.claims, 1);

    let mut detach_unsupported = FakeTransport {
        kernel_driver_result: Ok(true),
        detach_result: Err(rusb::Error::NotSupported),
        ..FakeTransport::default()
    };
    prepare_interface(
        &mut detach_unsupported,
        0,
        InterfacePolicy::PlatformDefault,
        HostPlatform::Windows,
    )
    .unwrap();
    assert_eq!(detach_unsupported.detaches, 1);
    assert_eq!(detach_unsupported.claims, 1);
}

#[test]
fn linux_default_detaches_active_driver_and_rejects_query_failure() {
    let mut active = FakeTransport {
        kernel_driver_result: Ok(true),
        ..FakeTransport::default()
    };
    prepare_interface(
        &mut active,
        0,
        InterfacePolicy::PlatformDefault,
        HostPlatform::Linux,
    )
    .unwrap();
    assert_eq!(active.detaches, 1);
    assert_eq!(active.claims, 1);

    let mut unsupported = FakeTransport {
        kernel_driver_result: Err(rusb::Error::NotSupported),
        ..FakeTransport::default()
    };
    assert!(matches!(
        prepare_interface(
            &mut unsupported,
            0,
            InterfacePolicy::PlatformDefault,
            HostPlatform::Linux,
        ),
        Err(LoaderError::KernelDriverStatus {
            source: rusb::Error::NotSupported,
            ..
        })
    ));
}

#[test]
fn zero_transfer_timeout_is_rejected_before_claim() {
    let config = LoaderConfig {
        transfer_timeout: Duration::ZERO,
        ..test_config()
    };
    assert!(matches!(
        Ov580BootDevice::from_transport(FakeTransport::default(), config),
        Err(LoaderError::ZeroTransferTimeout)
    ));
}

struct FakeReenumeration {
    states: VecDeque<ObservedDeviceState>,
    current: ObservedDeviceState,
    elapsed: Duration,
    waits: Cell<usize>,
}

impl FakeReenumeration {
    fn new(states: impl IntoIterator<Item = ObservedDeviceState>) -> Self {
        let states = states.into_iter().collect::<VecDeque<_>>();
        Self {
            states,
            current: ObservedDeviceState::Absent,
            elapsed: Duration::ZERO,
            waits: Cell::new(0),
        }
    }
}

impl ReenumerationBackend for FakeReenumeration {
    type Error = Infallible;

    fn observe(&mut self) -> Result<ObservedDeviceState, Self::Error> {
        if let Some(state) = self.states.pop_front() {
            self.current = state;
        }
        Ok(self.current)
    }

    fn elapsed(&self) -> Duration {
        self.elapsed
    }

    fn wait(&mut self, duration: Duration) {
        self.elapsed += duration;
        self.waits.set(self.waits.get() + 1);
    }
}

#[test]
fn reenumeration_separates_transition_from_already_camera() {
    let config = ReenumerationConfig {
        timeout: Duration::from_secs(1),
        poll_interval: Duration::from_millis(100),
    };
    let mut transition = FakeReenumeration::new([
        ObservedDeviceState::Boot,
        ObservedDeviceState::Absent,
        ObservedDeviceState::Camera,
    ]);
    assert_eq!(
        wait_for_camera_mode(&mut transition, config).unwrap(),
        ReenumerationOutcome::CameraReady {
            elapsed: Duration::from_millis(200)
        }
    );

    let mut camera = FakeReenumeration::new([ObservedDeviceState::Camera]);
    assert_eq!(
        wait_for_camera_mode(&mut camera, config).unwrap(),
        ReenumerationOutcome::AlreadyCamera {
            elapsed: Duration::ZERO
        }
    );
    assert_eq!(camera.waits.get(), 0);
}

#[test]
fn reenumeration_timeout_reports_last_state_without_real_sleep() {
    let mut backend = FakeReenumeration::new([ObservedDeviceState::Absent]);
    let result = wait_for_camera_mode(
        &mut backend,
        ReenumerationConfig {
            timeout: Duration::from_millis(250),
            poll_interval: Duration::from_millis(100),
        },
    );
    assert!(matches!(
        result,
        Err(ReenumerationError::Timeout {
            timeout,
            last_state: ObservedDeviceState::Absent,
        }) if timeout == Duration::from_millis(250)
    ));
    assert_eq!(backend.elapsed, Duration::from_millis(250));
}
