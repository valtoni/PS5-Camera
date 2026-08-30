use super::*;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
};

const BOOT_ID: &str = "USB\\VID_05A9&PID_0580\\boot-1";
const CAMERA_ID: &str = "USB\\VID_05A9&PID_058C\\camera-1";

struct FakeUploader {
    results: VecDeque<Result<UploadOutcome, UploadFailure>>,
    calls: usize,
    observed_lengths: Vec<usize>,
    observed_locators: Vec<StableUsbLocator>,
    observed_limits: Vec<UploadLimits>,
}

#[derive(Default)]
struct FakeUvcObserver {
    ready: Vec<(String, StableUsbLocator)>,
    removed: Vec<(String, StableUsbLocator)>,
}

impl UvcLifecycleObserver for FakeUvcObserver {
    fn camera_ready(&mut self, instance_id: &str, locator: &StableUsbLocator) {
        self.ready.push((instance_id.to_owned(), locator.clone()));
    }

    fn camera_removed(&mut self, instance_id: &str, locator: &StableUsbLocator) {
        self.removed.push((instance_id.to_owned(), locator.clone()));
    }
}

impl FakeUploader {
    fn new(results: impl IntoIterator<Item = Result<UploadOutcome, UploadFailure>>) -> Self {
        Self {
            results: results.into_iter().collect(),
            calls: 0,
            observed_lengths: Vec::new(),
            observed_locators: Vec::new(),
            observed_limits: Vec::new(),
        }
    }
}

impl FirmwareUploader for FakeUploader {
    fn upload_and_execute(
        &mut self,
        image: &FirmwareImage,
        boot_locator: &StableUsbLocator,
        limits: UploadLimits,
        cancellation: &dyn CancellationSignal,
    ) -> Result<UploadOutcome, UploadFailure> {
        self.calls += 1;
        self.observed_lengths.push(image.len());
        self.observed_locators.push(boot_locator.clone());
        self.observed_limits.push(limits);
        if cancellation.is_cancelled() {
            return Err(UploadFailure {
                code: FailureCode::Cancelled,
            });
        }
        self.results
            .pop_front()
            .unwrap_or(Ok(UploadOutcome::Reenumerating))
    }
}

fn payload() -> FirmwarePayload {
    let bytes = vec![0x5a; ov580_loader::MIN_FIRMWARE_SIZE];
    let digest = Sha256Digest::calculate(&bytes);
    FirmwarePayload::new("test-cleanroom", Arc::<[u8]>::from(bytes), digest)
}

#[test]
fn bundled_reference_firmware_is_pinned_and_valid() {
    let payload = bundled_reference_firmware_payload();
    assert_eq!(payload.version(), BUNDLED_REFERENCE_FIRMWARE_VERSION);
    assert_eq!(
        Sha256Digest::calculate(BUNDLED_REFERENCE_FIRMWARE),
        Sha256Digest::parse_hex(BUNDLED_REFERENCE_FIRMWARE_SHA256).unwrap()
    );
    assert!(payload.verify().is_ok());
}

fn engine() -> ServiceEngine {
    ServiceEngine::new(
        ServiceConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(100),
            maximum_backoff: Duration::from_millis(250),
            reenumeration_timeout: Duration::from_millis(500),
            upload_limits: UploadLimits::default(),
        },
        payload(),
    )
    .unwrap()
}

fn arrived(mode: DeviceMode, id: &str, milliseconds: u64) -> DeviceEvent {
    DeviceEvent::Arrived {
        mode,
        instance_id: id.to_owned(),
        locator: Some(locator()),
        at: Duration::from_millis(milliseconds),
    }
}

fn locator() -> StableUsbLocator {
    StableUsbLocator::new("controller-1", vec![3], UsbLinkSpeed::Super)
}

fn timer(milliseconds: u64) -> DeviceEvent {
    DeviceEvent::Timer {
        at: Duration::from_millis(milliseconds),
    }
}

fn failure(code: FailureCode) -> Result<UploadOutcome, UploadFailure> {
    Err(UploadFailure { code })
}

#[test]
fn digest_parser_and_payload_verification_are_deterministic() {
    let bytes = vec![0x11; ov580_loader::MIN_FIRMWARE_SIZE];
    let digest = Sha256Digest::calculate(&bytes);
    assert_eq!(
        Sha256Digest::parse_hex(&digest.to_string()).unwrap(),
        digest
    );
    assert!(matches!(
        Sha256Digest::parse_hex("not-a-digest"),
        Err(DigestParseError::WrongLength { .. })
    ));
    assert!(matches!(
        Sha256Digest::parse_hex(&"z".repeat(64)),
        Err(DigestParseError::InvalidHex { index: 0 })
    ));
    assert!(matches!(
        Sha256Digest::parse_hex(&"é".repeat(32)),
        Err(DigestParseError::InvalidHex { index: 0 })
    ));

    let tampered = FirmwarePayload::new(
        "tampered",
        Arc::<[u8]>::from(vec![0x22; ov580_loader::MIN_FIRMWARE_SIZE]),
        digest,
    );
    assert!(matches!(
        tampered.verify(),
        Err(PayloadError::DigestMismatch { .. })
    ));
}

#[test]
fn boot_upload_reenumeration_and_camera_follow_expected_states() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([Ok(UploadOutcome::Reenumerating)]);
    let records = engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );

    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [
            RecordKind::BootObserved,
            RecordKind::UploadStarted,
            RecordKind::UploadCompleted,
            RecordKind::ReenumerationStarted,
        ]
    );
    assert!(matches!(
        engine.state(),
        ServiceState::Reenlisting { attempt: 1, .. }
    ));
    assert_eq!(uploader.calls, 1);
    assert_eq!(uploader.observed_lengths, [ov580_loader::MIN_FIRMWARE_SIZE]);
    assert_eq!(uploader.observed_locators, [locator()]);
    assert_eq!(uploader.observed_limits, [UploadLimits::default()]);

    let removal = engine.handle(
        DeviceEvent::Removed {
            instance_id: BOOT_ID.to_owned(),
            at: Duration::from_millis(20),
        },
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(removal[0].kind, RecordKind::DeviceRemoved);
    assert!(matches!(engine.state(), ServiceState::Reenlisting { .. }));

    engine.handle(
        arrived(DeviceMode::Camera, CAMERA_ID, 100),
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(
        engine.state(),
        &ServiceState::Ready {
            camera_instance_id: CAMERA_ID.to_owned(),
            camera_locator: locator(),
        }
    );
}

#[test]
fn uvc_lifecycle_observer_only_sees_camera_ready_and_removal() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([]);
    let mut observer = FakeUvcObserver::default();

    engine.handle_with_uvc_observer(
        arrived(DeviceMode::Camera, CAMERA_ID, 0),
        &mut uploader,
        &NeverCancelled,
        &mut observer,
    );
    assert_eq!(observer.ready, vec![(CAMERA_ID.to_owned(), locator())]);
    assert!(observer.removed.is_empty());

    engine.handle_with_uvc_observer(
        DeviceEvent::Removed {
            instance_id: CAMERA_ID.to_owned(),
            at: Duration::from_millis(1),
        },
        &mut uploader,
        &NeverCancelled,
        &mut observer,
    );
    assert_eq!(observer.removed, vec![(CAMERA_ID.to_owned(), locator())]);
}

#[test]
fn already_camera_never_invokes_uploader() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([]);
    let records = engine.handle(
        arrived(DeviceMode::Camera, CAMERA_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, RecordKind::CameraReady);
    assert_eq!(uploader.calls, 0);
    assert!(matches!(engine.state(), ServiceState::Ready { .. }));
}

#[test]
fn disconnect_during_upload_schedules_bounded_retry_and_ignores_duplicate_boot() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([
        failure(FailureCode::DeviceDisconnected),
        Ok(UploadOutcome::Reenumerating),
    ]);
    engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(uploader.calls, 1);
    assert!(matches!(
        engine.state(),
        ServiceState::RetryWaiting {
            attempt: 1,
            retry_at,
            failure: FailureCode::DeviceDisconnected,
            ..
        } if *retry_at == Duration::from_millis(100)
    ));

    let duplicate = engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 10),
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(duplicate[0].kind, RecordKind::DuplicateIgnored);
    engine.handle(timer(99), &mut uploader, &NeverCancelled);
    assert_eq!(uploader.calls, 1);
    engine.handle(timer(100), &mut uploader, &NeverCancelled);
    assert_eq!(uploader.calls, 2);
    assert!(matches!(
        engine.state(),
        ServiceState::Reenlisting { attempt: 2, .. }
    ));
}

#[test]
fn reenumeration_timeout_enters_backoff_then_retries() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([
        Ok(UploadOutcome::Reenumerating),
        Ok(UploadOutcome::Reenumerating),
    ]);
    engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );
    engine.handle(timer(499), &mut uploader, &NeverCancelled);
    assert_eq!(uploader.calls, 1);

    let timeout_records = engine.handle(timer(500), &mut uploader, &NeverCancelled);
    assert_eq!(timeout_records[0].kind, RecordKind::ReenumerationTimedOut);
    assert_eq!(timeout_records[1].kind, RecordKind::RetryScheduled);
    assert!(matches!(
        engine.state(),
        ServiceState::RetryWaiting {
            retry_at,
            ..
        } if *retry_at == Duration::from_millis(600)
    ));

    engine.handle(timer(600), &mut uploader, &NeverCancelled);
    assert_eq!(uploader.calls, 2);
    assert!(matches!(
        engine.state(),
        ServiceState::Reenlisting { attempt: 2, .. }
    ));
}

#[test]
fn retry_limit_prevents_an_upload_loop_until_removal() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([
        failure(FailureCode::TransferTimeout),
        failure(FailureCode::TransferTimeout),
        failure(FailureCode::TransferTimeout),
        Ok(UploadOutcome::Reenumerating),
    ]);

    engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );
    engine.handle(timer(100), &mut uploader, &NeverCancelled);
    engine.handle(timer(300), &mut uploader, &NeverCancelled);
    assert_eq!(uploader.calls, 3);
    assert!(matches!(
        engine.state(),
        ServiceState::FailedPermanent { attempts: 3, .. }
    ));

    engine.handle(timer(10_000), &mut uploader, &NeverCancelled);
    engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 10_001),
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(uploader.calls, 3);

    engine.handle(
        DeviceEvent::Removed {
            instance_id: BOOT_ID.to_owned(),
            at: Duration::from_millis(10_002),
        },
        &mut uploader,
        &NeverCancelled,
    );
    engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 10_003),
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(uploader.calls, 4);
}

#[test]
fn digest_mismatch_is_permanent_and_never_reaches_uploader() {
    let bytes = vec![0x44; ov580_loader::MIN_FIRMWARE_SIZE];
    let wrong_digest = Sha256Digest::calculate(&vec![0x55; ov580_loader::MIN_FIRMWARE_SIZE]);
    let bad_payload = FirmwarePayload::new("bad", Arc::<[u8]>::from(bytes), wrong_digest);
    let mut engine = ServiceEngine::new(ServiceConfig::default(), bad_payload).unwrap();
    let mut uploader = FakeUploader::new([]);
    let records = engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );
    assert_eq!(records.last().unwrap().kind, RecordKind::PermanentFailure);
    assert_eq!(
        records.last().unwrap().failure,
        Some(FailureCode::PayloadDigestMismatch)
    );
    assert_eq!(uploader.calls, 0);
}

struct FlagCancellation(AtomicBool);

impl CancellationSignal for FlagCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[test]
fn cancellation_stops_without_retry() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([]);
    let cancelled = FlagCancellation(AtomicBool::new(true));
    let records = engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &cancelled,
    );
    assert_eq!(records.last().unwrap().kind, RecordKind::Cancelled);
    assert_eq!(uploader.calls, 0);
    assert_eq!(engine.state(), &ServiceState::Stopped);
}

#[test]
fn loader_already_camera_result_moves_directly_to_ready() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([Ok(UploadOutcome::AlreadyCamera)]);
    engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );
    assert!(matches!(engine.state(), ServiceState::Ready { .. }));
    assert_eq!(uploader.calls, 1);
}

#[test]
fn config_rejects_unbounded_or_busy_loop_settings() {
    let config = ServiceConfig {
        max_attempts: 0,
        ..ServiceConfig::default()
    };
    assert!(matches!(
        ServiceEngine::new(config, payload()),
        Err(ServiceConfigError::ZeroAttempts)
    ));

    let config = ServiceConfig {
        initial_backoff: Duration::ZERO,
        ..ServiceConfig::default()
    };
    assert!(matches!(
        ServiceEngine::new(config, payload()),
        Err(ServiceConfigError::ZeroDuration)
    ));

    let config = ServiceConfig {
        upload_limits: UploadLimits {
            maximum_image_bytes: 0,
            ..UploadLimits::default()
        },
        ..ServiceConfig::default()
    };
    assert!(matches!(
        ServiceEngine::new(config, payload()),
        Err(ServiceConfigError::InvalidUploadLimits)
    ));

    let config = ServiceConfig {
        upload_limits: UploadLimits {
            transfer_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(1),
            ..UploadLimits::default()
        },
        ..ServiceConfig::default()
    };
    assert!(matches!(
        ServiceEngine::new(config, payload()),
        Err(ServiceConfigError::TransferTimeoutExceedsTotal)
    ));
}

#[test]
fn missing_or_high_speed_topology_fails_closed_before_upload() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([]);
    let missing = DeviceEvent::Arrived {
        mode: DeviceMode::Boot,
        instance_id: BOOT_ID.to_owned(),
        locator: None,
        at: Duration::ZERO,
    };
    let records = engine.handle(missing, &mut uploader, &NeverCancelled);
    assert_eq!(records[0].kind, RecordKind::TopologyRejected);
    assert_eq!(records[0].failure, Some(FailureCode::TopologyUnavailable));

    let high_speed = DeviceEvent::Arrived {
        mode: DeviceMode::Boot,
        instance_id: BOOT_ID.to_owned(),
        locator: Some(StableUsbLocator::new(
            "controller-1",
            vec![3],
            UsbLinkSpeed::High,
        )),
        at: Duration::from_millis(1),
    };
    let records = engine.handle(high_speed, &mut uploader, &NeverCancelled);
    assert_eq!(records[0].failure, Some(FailureCode::UnsupportedLinkSpeed));
    assert_eq!(uploader.calls, 0);
    assert_eq!(engine.state(), &ServiceState::Absent);
}

#[test]
fn camera_reenumeration_must_match_boot_controller_and_port() {
    let mut engine = engine();
    let mut uploader = FakeUploader::new([Ok(UploadOutcome::Reenumerating)]);
    engine.handle(
        arrived(DeviceMode::Boot, BOOT_ID, 0),
        &mut uploader,
        &NeverCancelled,
    );
    let event = DeviceEvent::Arrived {
        mode: DeviceMode::Camera,
        instance_id: CAMERA_ID.to_owned(),
        locator: Some(StableUsbLocator::new(
            "controller-1",
            vec![4],
            UsbLinkSpeed::Super,
        )),
        at: Duration::from_millis(10),
    };
    let records = engine.handle(event, &mut uploader, &NeverCancelled);
    assert_eq!(records[0].kind, RecordKind::TopologyRejected);
    assert_eq!(records[0].failure, Some(FailureCode::TopologyMismatch));
    assert!(matches!(engine.state(), ServiceState::Reenlisting { .. }));
}

#[test]
fn exact_device_selection_rejects_multiple_and_high_speed() {
    let expected = locator();
    let boot = SupportedUsbObservation {
        mode: DeviceMode::Boot,
        locator: expected.clone(),
    };
    assert_eq!(
        validate_single_supported_device(std::slice::from_ref(&boot), &expected),
        Ok(DeviceMode::Boot)
    );
    assert_eq!(
        validate_single_supported_device(&[boot.clone(), boot.clone()], &expected),
        Err(FailureCode::MultipleSupportedDevices)
    );
    let high = SupportedUsbObservation {
        mode: DeviceMode::Boot,
        locator: StableUsbLocator::new("controller-1", vec![3], UsbLinkSpeed::High),
    };
    assert_eq!(
        validate_single_supported_device(&[high], &expected),
        Err(FailureCode::UnsupportedLinkSpeed)
    );
}

#[test]
fn camera_selection_correlates_by_controller_and_port() {
    let expected = locator();
    let camera = SupportedUsbObservation {
        mode: DeviceMode::Camera,
        locator: StableUsbLocator::new("controller-1", vec![3], UsbLinkSpeed::SuperPlus),
    };
    assert_eq!(
        validate_single_supported_device(&[camera], &expected),
        Ok(DeviceMode::Camera)
    );
    let wrong_port = SupportedUsbObservation {
        mode: DeviceMode::Camera,
        locator: StableUsbLocator::new("controller-1", vec![9], UsbLinkSpeed::Super),
    };
    assert_eq!(
        validate_single_supported_device(&[wrong_port], &expected),
        Err(FailureCode::TopologyMismatch)
    );
}

#[test]
fn image_and_transfer_bounds_are_enforced_before_or_during_upload() {
    assert_eq!(
        bounded_transfer_timeout(Duration::from_secs(1), Duration::from_millis(25)),
        Some(Duration::from_millis(25))
    );
    assert_eq!(
        bounded_transfer_timeout(Duration::from_secs(1), Duration::ZERO),
        None
    );
    assert!(matches!(
        payload().verify_bounded(ov580_loader::MIN_FIRMWARE_SIZE - 1),
        Err(PayloadError::ImageTooLarge { .. })
    ));
}

#[test]
fn stop_in_last_chunk_execute_window_prevents_execute_deterministically() {
    let stop = ScmStopSignal::new();
    assert!(check_pre_execute_gate(&stop, false).is_ok());

    stop.request_stop();
    assert_eq!(
        check_pre_execute_gate(&stop, false),
        Err(UploadFailure {
            code: FailureCode::Cancelled
        })
    );
}

#[test]
fn deadline_in_last_chunk_execute_window_prevents_execute_deterministically() {
    assert_eq!(
        check_pre_execute_gate(&NeverCancelled, true),
        Err(UploadFailure {
            code: FailureCode::UploadDeadlineExceeded
        })
    );
}

struct VecSource(VecDeque<DeviceEvent>);

impl DeviceEventSource for VecSource {
    fn next_event(
        &mut self,
        _cancellation: &dyn CancellationSignal,
    ) -> Result<Option<DeviceEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.0.pop_front())
    }
}

#[derive(Default)]
struct VecSink(Vec<ServiceRecord>);

impl StructuredEventSink for VecSink {
    fn emit(
        &mut self,
        record: &ServiceRecord,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.0.push(record.clone());
        Ok(())
    }
}

#[test]
fn injected_event_source_and_sink_drive_host_without_polling() {
    let mut source = VecSource(VecDeque::from([
        arrived(DeviceMode::Camera, CAMERA_ID, 1),
        DeviceEvent::Cancel {
            at: Duration::from_millis(2),
        },
    ]));
    let mut sink = VecSink::default();
    let mut uploader = FakeUploader::new([]);
    let mut engine = engine();
    run_service(
        &mut engine,
        &mut source,
        &mut uploader,
        &mut sink,
        &NeverCancelled,
    )
    .unwrap();
    assert_eq!(sink.0.len(), 2);
    assert_eq!(sink.0[0].kind, RecordKind::CameraReady);
    assert_eq!(sink.0[1].kind, RecordKind::Cancelled);
}

#[test]
fn service_loop_forwards_only_uvc_lifecycle_events_to_observer() {
    let mut source = VecSource(VecDeque::from([
        arrived(DeviceMode::Camera, CAMERA_ID, 1),
        DeviceEvent::Removed {
            instance_id: CAMERA_ID.to_owned(),
            at: Duration::from_millis(2),
        },
        DeviceEvent::Cancel {
            at: Duration::from_millis(3),
        },
    ]));
    let mut sink = VecSink::default();
    let mut uploader = FakeUploader::new([]);
    let mut observer = FakeUvcObserver::default();
    let mut engine = engine();
    run_service_with_uvc_observer(
        &mut engine,
        &mut source,
        &mut uploader,
        &mut sink,
        &NeverCancelled,
        &mut observer,
    )
    .unwrap();
    assert_eq!(observer.ready, vec![(CAMERA_ID.to_owned(), locator())]);
    assert_eq!(observer.removed, vec![(CAMERA_ID.to_owned(), locator())]);
}

#[test]
fn structured_records_serialize_without_payload_bytes() {
    let record = ServiceRecord {
        sequence: 7,
        at: Duration::from_secs(1),
        level: RecordLevel::Warning,
        kind: RecordKind::RetryScheduled,
        from: StateName::Uploading,
        to: StateName::RetryWaiting,
        attempt: Some(1),
        failure: Some(FailureCode::TransferTimeout),
    };
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("retry_scheduled"));
    assert!(json.contains("transfer_timeout"));
    assert!(!json.contains("firmware"));
}

#[test]
fn startup_discovery_emits_one_located_arrival() {
    let current = DiscoveredSupportedDevice {
        mode: DeviceMode::Boot,
        instance_id: "libusb-bus-1:13".to_owned(),
        locator: locator(),
    };
    assert_eq!(
        reconcile_discovered_device(None, Some(&current), Duration::from_millis(5)),
        vec![DeviceEvent::Arrived {
            mode: DeviceMode::Boot,
            instance_id: current.instance_id.clone(),
            locator: Some(current.locator.clone()),
            at: Duration::from_millis(5),
        }]
    );
}

#[test]
fn reenlist_discovery_emits_removal_then_camera_arrival_on_same_port() {
    let boot = DiscoveredSupportedDevice {
        mode: DeviceMode::Boot,
        instance_id: "libusb-bus-1:13".to_owned(),
        locator: locator(),
    };
    let camera = DiscoveredSupportedDevice {
        mode: DeviceMode::Camera,
        ..boot.clone()
    };
    assert_eq!(
        reconcile_discovered_device(Some(&boot), Some(&camera), Duration::from_millis(9)),
        vec![
            DeviceEvent::Removed {
                instance_id: boot.instance_id,
                at: Duration::from_millis(9),
            },
            DeviceEvent::Arrived {
                mode: DeviceMode::Camera,
                instance_id: camera.instance_id,
                locator: Some(camera.locator),
                at: Duration::from_millis(9),
            },
        ]
    );
}

#[test]
fn unchanged_discovery_does_not_duplicate_upload_events() {
    let device = DiscoveredSupportedDevice {
        mode: DeviceMode::Boot,
        instance_id: "libusb-bus-1:13".to_owned(),
        locator: locator(),
    };
    assert!(
        reconcile_discovered_device(Some(&device), Some(&device), Duration::from_secs(1))
            .is_empty()
    );
}
