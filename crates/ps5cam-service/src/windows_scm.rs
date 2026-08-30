use crate::{
    bundled_reference_firmware_payload, discover_single_supported_device, host_readiness,
    reconcile_discovered_device, translate_windows_device_change, CancellationSignal, DeviceEvent,
    DiscoveredSupportedDevice, FailureCode, HostReadiness, RusbFirmwareUploader, ScmBlocker,
    ScmControl, ScmControlAction, ScmDispatchError, ScmLifecycle, ScmLogRecord, ScmPhase,
    ScmStatusSnapshot, ScmStopSignal, ServiceConfig, ServiceEngine, ServiceRecord,
    WindowsDeviceChange, WindowsDeviceEventRecord, WindowsEventLogSink, WINDOWS_SERVICE_NAME,
};
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering},
    mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    Mutex, MutexGuard, OnceLock,
};
use std::time::{Duration, Instant};
use std::{mem, ptr, slice};
use windows_sys::Win32::Devices::Usb::GUID_DEVINTERFACE_USB_DEVICE;
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_CALL_NOT_IMPLEMENTED, ERROR_SERVICE_SPECIFIC_ERROR, HANDLE, NO_ERROR,
};
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_CONTROL_DEVICEEVENT,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING,
    SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    RegisterDeviceNotificationW, UnregisterDeviceNotification, DBT_DEVICEARRIVAL,
    DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_DEVICEINTERFACE, DEVICE_NOTIFY_SERVICE_HANDLE,
    DEV_BROADCAST_DEVICEINTERFACE_W, DEV_BROADCAST_HDR,
};

const SERVICE_ERROR_NONE: u32 = 0;
const MAX_DEVICE_INTERFACE_EVENT_BYTES: usize = 64 * 1024;
const RUNTIME_EVENT_QUEUE_CAPACITY: usize = 256;
const ENGINE_TIMER_INTERVAL: Duration = Duration::from_millis(250);

static RUNTIME: OnceLock<ScmRuntime> = OnceLock::new();
static SERVICE_MAIN_ERROR: AtomicU32 = AtomicU32::new(SERVICE_ERROR_NONE);

#[derive(Debug)]
enum RuntimeEvent {
    Scm(ScmLogRecord),
    Device(WindowsDeviceEventRecord),
    Stop(ScmLogRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeQueueError {
    Full,
    Disconnected,
}

struct ScmRuntime {
    lifecycle: Mutex<ScmLifecycle>,
    stop: ScmStopSignal,
    status_handle: AtomicPtr<c_void>,
    notification_handle: AtomicPtr<c_void>,
    device_sequence: AtomicU64,
    started: Instant,
    event_sender: SyncSender<RuntimeEvent>,
    event_receiver: Mutex<Receiver<RuntimeEvent>>,
}

impl ScmRuntime {
    fn new() -> Self {
        let (event_sender, event_receiver) = sync_channel(RUNTIME_EVENT_QUEUE_CAPACITY);
        Self {
            lifecycle: Mutex::new(ScmLifecycle::new()),
            stop: ScmStopSignal::new(),
            status_handle: AtomicPtr::new(ptr::null_mut()),
            notification_handle: AtomicPtr::new(ptr::null_mut()),
            device_sequence: AtomicU64::new(0),
            started: Instant::now(),
            event_sender,
            event_receiver: Mutex::new(event_receiver),
        }
    }

    fn lifecycle(&self) -> MutexGuard<'_, ScmLifecycle> {
        match self.lifecycle.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn status_handle(&self) -> SERVICE_STATUS_HANDLE {
        self.status_handle.load(Ordering::Acquire)
    }

    fn try_enqueue(&self, event: RuntimeEvent) -> Result<(), RuntimeQueueError> {
        self.event_sender
            .try_send(event)
            .map_err(|error| match error {
                TrySendError::Full(_) => RuntimeQueueError::Full,
                TrySendError::Disconnected(_) => RuntimeQueueError::Disconnected,
            })
    }

    #[cfg(test)]
    fn receive(&self) -> Result<RuntimeEvent, std::sync::mpsc::RecvError> {
        match self.event_receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(poisoned) => poisoned.into_inner().recv(),
        }
    }

    fn receive_timeout(&self, timeout: Duration) -> Result<RuntimeEvent, RecvTimeoutError> {
        match self.event_receiver.lock() {
            Ok(receiver) => receiver.recv_timeout(timeout),
            Err(poisoned) => poisoned.into_inner().recv_timeout(timeout),
        }
    }
}

pub(super) fn run_dispatcher() -> Result<(), ScmDispatchError> {
    let mut service_name = wide_null(WINDOWS_SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: ptr::null_mut(),
            lpServiceProc: None,
        },
    ];

    // SAFETY: `table` is terminated by a null entry and remains valid until
    // the dispatcher returns after service main has exited.
    let result = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    if result == 0 {
        // SAFETY: GetLastError has no preconditions and is called immediately
        // after the failing Windows API.
        let code = unsafe { GetLastError() };
        return Err(ScmDispatchError::Dispatcher { code });
    }
    let code = SERVICE_MAIN_ERROR.load(Ordering::Acquire);
    if code != SERVICE_ERROR_NONE {
        Err(ScmDispatchError::ServiceMain { code })
    } else {
        Ok(())
    }
}

unsafe extern "system" fn service_main(_argument_count: u32, _arguments: *mut *mut u16) {
    let runtime = RUNTIME.get_or_init(ScmRuntime::new);
    let service_name = wide_null(WINDOWS_SERVICE_NAME);
    // SAFETY: the name is NUL terminated, handler has the required ABI, and
    // the static runtime outlives all possible control callbacks.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(service_name.as_ptr(), Some(control_handler), ptr::null())
    };
    if handle.is_null() {
        // SAFETY: called immediately after registration failed.
        SERVICE_MAIN_ERROR.store(unsafe { GetLastError() }, Ordering::Release);
        return;
    }
    runtime.status_handle.store(handle, Ordering::Release);

    if let Err(code) = publish(runtime, runtime.lifecycle().status()) {
        SERVICE_MAIN_ERROR.store(code, Ordering::Release);
        return;
    }
    let mut event_log = initialize_event_log();
    if let Err(code) = register_device_notifications(runtime) {
        stop_with_error(runtime, code, &mut event_log);
        return;
    }

    let (startup_records, status) = {
        let mut lifecycle = runtime.lifecycle();
        let mut records = Vec::with_capacity(2);
        if let Ok(record) = lifecycle.mark_running() {
            records.push(record);
        }
        match host_readiness() {
            HostReadiness::Ready => {}
            HostReadiness::FirmwareUnavailable => {
                records.push(lifecycle.note_readiness_blocked(ScmBlocker::FirmwareUnavailable))
            }
            HostReadiness::UnsupportedPlatform => {
                records.push(lifecycle.note_readiness_blocked(ScmBlocker::UnsupportedPlatform))
            }
        }
        (records, lifecycle.status())
    };
    for record in startup_records {
        emit(&mut event_log, &record);
    }
    if let Err(code) = publish(runtime, status) {
        SERVICE_MAIN_ERROR.store(code, Ordering::Release);
        return;
    }

    let mut engine = ServiceEngine::new(
        ServiceConfig::default(),
        bundled_reference_firmware_payload(),
    )
    .expect("the built-in Windows service configuration must be valid");
    let mut uploader = RusbFirmwareUploader::default();
    let mut observed = None;

    // The service may start after the camera was connected, so registration
    // alone is insufficient. Perform exactly one initial discovery, then
    // discover again only after matching Windows notifications.
    reconcile_usb_state(
        runtime,
        &mut engine,
        &mut uploader,
        &mut event_log,
        &mut observed,
    );

    // Only this service-main thread performs discovery, upload, Event Log and
    // stderr I/O. HandlerEx remains bounded to copying and queueing events.
    loop {
        match runtime.receive_timeout(ENGINE_TIMER_INTERVAL) {
            Ok(RuntimeEvent::Scm(record)) => emit(&mut event_log, &record),
            Ok(RuntimeEvent::Device(record)) => {
                emit_device(&mut event_log, &record);
                reconcile_usb_state(
                    runtime,
                    &mut engine,
                    &mut uploader,
                    &mut event_log,
                    &mut observed,
                );
            }
            Ok(RuntimeEvent::Stop(record)) => {
                emit(&mut event_log, &record);
                process_engine_event(
                    &mut engine,
                    &mut uploader,
                    &mut event_log,
                    &runtime.stop,
                    DeviceEvent::Cancel {
                        at: runtime.started.elapsed(),
                    },
                );
                break;
            }
            Err(RecvTimeoutError::Timeout) => process_engine_event(
                &mut engine,
                &mut uploader,
                &mut event_log,
                &runtime.stop,
                DeviceEvent::Timer {
                    at: runtime.started.elapsed(),
                },
            ),
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if runtime.stop.is_cancelled() {
            break;
        }
    }

    let unregister_error = unregister_device_notifications(runtime).err();
    let (record, status) = {
        let mut lifecycle = runtime.lifecycle();
        let record = lifecycle
            .mark_stopped(unregister_error.unwrap_or(NO_ERROR), 0)
            .ok();
        (record, lifecycle.status())
    };
    if let Some(record) = record {
        emit(&mut event_log, &record);
    }
    if let Err(code) = publish(runtime, status) {
        SERVICE_MAIN_ERROR.store(code, Ordering::Release);
    } else if let Some(code) = unregister_error {
        SERVICE_MAIN_ERROR.store(code, Ordering::Release);
    }
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    let Some(runtime) = RUNTIME.get() else {
        return ERROR_CALL_NOT_IMPLEMENTED;
    };
    if control == SERVICE_CONTROL_DEVICEEVENT {
        handle_device_event(runtime, _event_type, _event_data);
        return NO_ERROR;
    }
    let control = match control {
        SERVICE_CONTROL_STOP => ScmControl::Stop,
        SERVICE_CONTROL_SHUTDOWN => ScmControl::Shutdown,
        SERVICE_CONTROL_INTERROGATE => ScmControl::Interrogate,
        other => ScmControl::Other(other),
    };
    let (action, record, status) = {
        let mut lifecycle = runtime.lifecycle();
        let (action, record) = lifecycle.handle_control(control);
        (action, record, lifecycle.status())
    };

    match action {
        ScmControlAction::RequestStop => {
            let publish_result = publish(runtime, status);
            complete_stop_request(runtime, record, publish_result)
        }
        ScmControlAction::ReportStatus => {
            let result = publish(runtime, status).err().unwrap_or(NO_ERROR);
            let _ = runtime.try_enqueue(RuntimeEvent::Scm(record));
            result
        }
        ScmControlAction::Ignore => {
            let _ = runtime.try_enqueue(RuntimeEvent::Scm(record));
            ERROR_CALL_NOT_IMPLEMENTED
        }
    }
}

fn complete_stop_request(
    runtime: &ScmRuntime,
    record: ScmLogRecord,
    publish_result: Result<(), u32>,
) -> u32 {
    runtime.stop.request_stop();
    let _ = runtime.try_enqueue(RuntimeEvent::Stop(record));
    publish_result.err().unwrap_or(NO_ERROR)
}

fn register_device_notifications(runtime: &ScmRuntime) -> Result<(), u32> {
    let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
        dbcc_size: mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
        dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE,
        dbcc_reserved: 0,
        dbcc_classguid: GUID_DEVINTERFACE_USB_DEVICE,
        dbcc_name: [0],
    };
    // SAFETY: the recipient is the status handle returned by SCM, the filter
    // is initialized for the USB device-interface class, and Windows copies
    // the filter before this call returns.
    let handle = unsafe {
        RegisterDeviceNotificationW(
            runtime.status_handle() as HANDLE,
            (&filter as *const DEV_BROADCAST_DEVICEINTERFACE_W).cast(),
            DEVICE_NOTIFY_SERVICE_HANDLE,
        )
    };
    if handle.is_null() {
        // SAFETY: called immediately after registration failed.
        Err(unsafe { GetLastError() })
    } else {
        runtime.notification_handle.store(handle, Ordering::Release);
        Ok(())
    }
}

fn unregister_device_notifications(runtime: &ScmRuntime) -> Result<(), u32> {
    let handle = runtime
        .notification_handle
        .swap(ptr::null_mut(), Ordering::AcqRel);
    if handle.is_null() {
        return Ok(());
    }
    // SAFETY: the handle came from RegisterDeviceNotificationW and the atomic
    // swap guarantees this process unregisters it at most once.
    if unsafe { UnregisterDeviceNotification(handle) } == 0 {
        // SAFETY: called immediately after unregistration failed.
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}

fn stop_with_error(runtime: &ScmRuntime, code: u32, event_log: &mut Option<WindowsEventLogSink>) {
    let (record, status) = {
        let mut lifecycle = runtime.lifecycle();
        let record = lifecycle.mark_stopped(code, 0).ok();
        (record, lifecycle.status())
    };
    if let Some(record) = record {
        emit(event_log, &record);
    }
    let _ = publish(runtime, status);
    SERVICE_MAIN_ERROR.store(code, Ordering::Release);
}

fn reconcile_usb_state(
    runtime: &ScmRuntime,
    engine: &mut ServiceEngine,
    uploader: &mut RusbFirmwareUploader,
    event_log: &mut Option<WindowsEventLogSink>,
    observed: &mut Option<DiscoveredSupportedDevice>,
) {
    let current = match discover_single_supported_device() {
        Ok(current) => current,
        Err(failure) => {
            emit_discovery_failure(failure);
            return;
        }
    };
    let events = reconcile_discovered_device(
        observed.as_ref(),
        current.as_ref(),
        runtime.started.elapsed(),
    );
    for event in events {
        process_engine_event(engine, uploader, event_log, &runtime.stop, event);
    }
    *observed = current;
}

fn process_engine_event(
    engine: &mut ServiceEngine,
    uploader: &mut RusbFirmwareUploader,
    event_log: &mut Option<WindowsEventLogSink>,
    cancellation: &ScmStopSignal,
    event: DeviceEvent,
) {
    for record in engine.handle(event, uploader, cancellation) {
        emit_service(event_log, &record);
    }
}

fn emit_discovery_failure(failure: FailureCode) {
    eprintln!(
        "{}",
        serde_json::json!({
            "component": "ps5cam-service",
            "operation": "discover_supported_usb_device",
            "success": false,
            "failure": failure,
        })
    );
}

fn handle_device_event(runtime: &ScmRuntime, event_type: u32, event_data: *mut c_void) {
    let change = match event_type {
        DBT_DEVICEARRIVAL => WindowsDeviceChange::Arrival,
        DBT_DEVICEREMOVECOMPLETE => WindowsDeviceChange::RemovalComplete,
        _ => return,
    };
    // SAFETY: SCM guarantees event_data is valid for this callback. The helper
    // validates the broadcast type and byte length before copying its path.
    let Some(path) = (unsafe { copy_usb_interface_path(event_data) }) else {
        return;
    };
    let Some(event) = translate_windows_device_change(change, &path, runtime.started.elapsed())
    else {
        return;
    };
    let sequence = runtime.device_sequence.fetch_add(1, Ordering::AcqRel) + 1;
    let _ = runtime.try_enqueue(RuntimeEvent::Device(WindowsDeviceEventRecord::new(
        sequence, event,
    )));
}

unsafe fn copy_usb_interface_path(event_data: *mut c_void) -> Option<String> {
    if event_data.is_null() {
        return None;
    }
    // SAFETY: caller passes the SCM callback's event pointer. Every device
    // broadcast begins with this fixed-size header.
    let header = unsafe { ptr::read_unaligned(event_data.cast::<DEV_BROADCAST_HDR>()) };
    let size = header.dbch_size as usize;
    let name_offset = mem::offset_of!(DEV_BROADCAST_DEVICEINTERFACE_W, dbcc_name);
    if header.dbch_devicetype != DBT_DEVTYP_DEVICEINTERFACE
        || size < mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>()
        || size > MAX_DEVICE_INTERFACE_EVENT_BYTES
    {
        return None;
    }
    // SAFETY: size was checked to cover all fixed interface fields.
    let interface =
        unsafe { ptr::read_unaligned(event_data.cast::<DEV_BROADCAST_DEVICEINTERFACE_W>()) };
    if !guid_equal(interface.dbcc_classguid, GUID_DEVINTERFACE_USB_DEVICE) {
        return None;
    }
    let name_length = (size - name_offset) / mem::size_of::<u16>();
    // SAFETY: the validated byte size belongs to this broadcast allocation;
    // name_offset identifies its trailing UTF-16 array.
    let name = unsafe {
        slice::from_raw_parts(
            event_data.cast::<u8>().add(name_offset).cast::<u16>(),
            name_length,
        )
    };
    let end = name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(name.len());
    if end == 0 {
        None
    } else {
        Some(String::from_utf16_lossy(&name[..end]))
    }
}

fn guid_equal(left: windows_sys::core::GUID, right: windows_sys::core::GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4 == right.data4
}

fn publish(runtime: &ScmRuntime, snapshot: ScmStatusSnapshot) -> Result<(), u32> {
    let current_state = match snapshot.phase {
        ScmPhase::StartPending => SERVICE_START_PENDING,
        ScmPhase::Running => SERVICE_RUNNING,
        ScmPhase::StopPending => SERVICE_STOP_PENDING,
        ScmPhase::Stopped => SERVICE_STOPPED,
    };
    let mut controls = 0;
    if snapshot.accepts_stop {
        controls |= SERVICE_ACCEPT_STOP;
    }
    if snapshot.accepts_shutdown {
        controls |= SERVICE_ACCEPT_SHUTDOWN;
    }
    let wait_hint = u32::try_from(snapshot.wait_hint.as_millis()).unwrap_or(u32::MAX);
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: current_state,
        dwControlsAccepted: controls,
        dwWin32ExitCode: if snapshot.service_specific_exit_code == 0 {
            snapshot.win32_exit_code
        } else {
            ERROR_SERVICE_SPECIFIC_ERROR
        },
        dwServiceSpecificExitCode: snapshot.service_specific_exit_code,
        dwCheckPoint: snapshot.checkpoint,
        dwWaitHint: wait_hint,
    };
    // SAFETY: handle was returned by registration and status is initialized
    // for the duration of this synchronous call.
    if unsafe { SetServiceStatus(runtime.status_handle(), &status) } == 0 {
        // SAFETY: called immediately after SetServiceStatus failed.
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}

fn emit(event_log: &mut Option<WindowsEventLogSink>, record: &ScmLogRecord) {
    with_event_log(event_log, "write_scm", |sink| sink.write_scm(record));
    if let Ok(json) = serde_json::to_string(record) {
        eprintln!("{json}");
    }
}

fn emit_device(event_log: &mut Option<WindowsEventLogSink>, record: &WindowsDeviceEventRecord) {
    with_event_log(event_log, "write_device", |sink| sink.write_device(record));
    if let Ok(json) = serde_json::to_string(record) {
        eprintln!("{json}");
    }
}

fn emit_service(event_log: &mut Option<WindowsEventLogSink>, record: &ServiceRecord) {
    with_event_log(event_log, "write_service", |sink| {
        sink.write_service(record)
    });
    if let Ok(json) = serde_json::to_string(record) {
        eprintln!("{json}");
    }
}

fn initialize_event_log() -> Option<WindowsEventLogSink> {
    match WindowsEventLogSink::open() {
        Ok(sink) => Some(sink),
        Err(error) => {
            event_log_fallback("open", &error);
            None
        }
    }
}

fn with_event_log(
    event_log: &mut Option<WindowsEventLogSink>,
    operation: &'static str,
    write: impl FnOnce(&mut WindowsEventLogSink) -> Result<(), crate::EventLogError>,
) {
    if let Some(sink) = event_log.as_mut() {
        if let Err(error) = write(sink) {
            event_log_fallback(operation, &error);
        }
    }
}

fn event_log_fallback(operation: &'static str, error: &crate::EventLogError) {
    let diagnostic = serde_json::json!({
        "component": "ps5cam-service",
        "sink": "windows_event_log",
        "fallback": "stderr",
        "operation": operation,
        "error": error.to_string(),
    });
    eprintln!("{diagnostic}");
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT_PATH: &str =
        r"\\?\USB#VID_05A9&PID_0580#5&1de99128&0&3#{a5dcbf10-6530-11d2-901f-00c04fb951ed}";

    fn interface_broadcast(path: &str) -> Vec<u8> {
        let wide = path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let offset = mem::offset_of!(DEV_BROADCAST_DEVICEINTERFACE_W, dbcc_name);
        let size = offset + wide.len() * mem::size_of::<u16>();
        let mut bytes = vec![0_u8; size.max(mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>())];
        let interface = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE,
            dbcc_reserved: 0,
            dbcc_classguid: GUID_DEVINTERFACE_USB_DEVICE,
            dbcc_name: [0],
        };
        // SAFETY: the destination covers the fixed struct and write_unaligned
        // permits Vec<u8>'s alignment.
        unsafe {
            ptr::write_unaligned(
                bytes.as_mut_ptr().cast::<DEV_BROADCAST_DEVICEINTERFACE_W>(),
                interface,
            );
            ptr::copy_nonoverlapping(
                wide.as_ptr().cast::<u8>(),
                bytes.as_mut_ptr().add(offset),
                wide.len() * mem::size_of::<u16>(),
            );
        }
        bytes
    }

    #[test]
    fn copies_valid_scm_device_interface_payload() {
        let mut bytes = interface_broadcast(BOOT_PATH);
        // SAFETY: helper constructs a valid DEV_BROADCAST_DEVICEINTERFACE_W.
        let copied = unsafe { copy_usb_interface_path(bytes.as_mut_ptr().cast()) };
        assert_eq!(copied.as_deref(), Some(BOOT_PATH));
    }

    #[test]
    fn rejects_truncated_or_wrong_class_broadcasts() {
        let mut truncated = interface_broadcast(BOOT_PATH);
        // SAFETY: buffer covers the complete header; unaligned access supports
        // Vec<u8>'s alignment.
        unsafe {
            let header_pointer = truncated.as_mut_ptr().cast::<DEV_BROADCAST_HDR>();
            let mut header = ptr::read_unaligned(header_pointer);
            header.dbch_size = 4;
            ptr::write_unaligned(header_pointer, header);
        }
        // SAFETY: malformed buffer remains large enough to read its header.
        assert_eq!(
            unsafe { copy_usb_interface_path(truncated.as_mut_ptr().cast()) },
            None
        );

        let mut wrong_class = interface_broadcast(BOOT_PATH);
        // SAFETY: helper buffer covers the complete fixed struct; unaligned
        // access supports Vec<u8>'s alignment.
        unsafe {
            let interface_pointer = wrong_class
                .as_mut_ptr()
                .cast::<DEV_BROADCAST_DEVICEINTERFACE_W>();
            let mut interface = ptr::read_unaligned(interface_pointer);
            interface.dbcc_classguid = windows_sys::core::GUID::default();
            ptr::write_unaligned(interface_pointer, interface);
        }
        // SAFETY: helper buffer is otherwise structurally valid.
        assert_eq!(
            unsafe { copy_usb_interface_path(wrong_class.as_mut_ptr().cast()) },
            None
        );
    }

    #[test]
    fn runtime_queue_moves_owned_records_off_the_callback_path() {
        let runtime = ScmRuntime::new();
        let record = ScmLifecycle::new().note_readiness_blocked(ScmBlocker::FirmwareUnavailable);

        assert!(runtime.try_enqueue(RuntimeEvent::Scm(record)).is_ok());
        match runtime.receive().expect("queued event") {
            RuntimeEvent::Scm(received) => assert_eq!(received, record),
            other => panic!("unexpected runtime event: {other:?}"),
        }
    }

    #[test]
    fn runtime_queue_reports_full_without_waiting() {
        let runtime = ScmRuntime::new();
        let record = ScmLifecycle::new().note_readiness_blocked(ScmBlocker::FirmwareUnavailable);
        for _ in 0..RUNTIME_EVENT_QUEUE_CAPACITY {
            assert!(runtime.try_enqueue(RuntimeEvent::Scm(record)).is_ok());
        }
        assert!(matches!(
            runtime.try_enqueue(RuntimeEvent::Scm(record)),
            Err(RuntimeQueueError::Full)
        ));
    }

    #[test]
    fn failed_stop_pending_publish_still_cancels_and_enqueues_stop() {
        let runtime = ScmRuntime::new();
        let mut lifecycle = ScmLifecycle::new();
        lifecycle.mark_running().unwrap();
        let (action, record) = lifecycle.handle_control(ScmControl::Stop);
        assert_eq!(action, ScmControlAction::RequestStop);

        assert_eq!(complete_stop_request(&runtime, record, Err(5)), 5);
        assert!(runtime.stop.is_cancelled());
        match runtime.receive().expect("stop event") {
            RuntimeEvent::Stop(received) => assert_eq!(received, record),
            other => panic!("unexpected runtime event: {other:?}"),
        }
    }
}
