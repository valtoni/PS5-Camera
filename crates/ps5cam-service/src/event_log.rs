use crate::{
    RecordLevel, ScmEventKind, ScmLogRecord, ServiceRecord, StructuredEventSink,
    WindowsDeviceEventRecord,
};
use serde::Serialize;
use std::fmt;
use thiserror::Error;

pub const EVENT_LOG_SOURCE_NAME: &str = "PS5CameraService";
pub const EVENT_LOG_APPLICATION_SOURCE_NAME: &str = "Application";
pub const EVENT_LOG_SCHEMA_VERSION: u32 = 1;
pub const MAX_EVENT_LOG_PAYLOAD_BYTES: usize = 60 * 1024;
pub const MAX_EVENT_LOG_INSERTION_UTF16_UNITS: usize = 31_839;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLogSourceMode {
    /// Classic Event Log API writing to the Application log. This deliberately
    /// makes no claim about whether the selected source is Registry-backed.
    ClassicApplicationLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLogStream {
    Service,
    Scm,
    DeviceNotification,
    SelfTest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLogLevel {
    Information,
    Warning,
    Error,
}

impl From<RecordLevel> for EventLogLevel {
    fn from(level: RecordLevel) -> Self {
        match level {
            RecordLevel::Info => Self::Information,
            RecordLevel::Warning => Self::Warning,
            RecordLevel::Error => Self::Error,
        }
    }
}

impl EventLogStream {
    pub const fn event_id(self) -> u32 {
        match self {
            Self::Service => 0x1000,
            Self::Scm => 0x1001,
            Self::DeviceNotification => 0x1002,
            Self::SelfTest => 0x10ff,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventLogSelfTestRecord {
    pub schema_version: u32,
    pub kind: &'static str,
    pub identifier: String,
    pub process_id: u32,
    pub unix_time_ms: u64,
}

impl EventLogSelfTestRecord {
    pub fn new(process_id: u32, unix_time_ms: u64) -> Self {
        Self {
            schema_version: EVENT_LOG_SCHEMA_VERSION,
            kind: "event_log_self_test",
            identifier: format!("ps5cam-event-log-self-test-{process_id}-{unix_time_ms}"),
            process_id,
            unix_time_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventLogSelfTestReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub success: bool,
    pub identifier: String,
    pub source_name: &'static str,
    pub source_mode: EventLogSourceMode,
    pub event_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventLogWriteReceipt {
    pub source_name: &'static str,
    pub source_mode: EventLogSourceMode,
}

impl EventLogSelfTestReport {
    pub const fn exit_code(&self) -> u8 {
        if self.success {
            0
        } else {
            3
        }
    }
}

pub fn perform_event_log_self_test<F>(
    record: &EventLogSelfTestRecord,
    write_once: F,
) -> EventLogSelfTestReport
where
    F: FnOnce(&EventLogSelfTestRecord) -> Result<EventLogWriteReceipt, EventLogError>,
{
    let result = write_once(record);
    let (source_name, source_mode, error) = match result {
        Ok(receipt) => (receipt.source_name, receipt.source_mode, None),
        Err(error) => (
            EVENT_LOG_SOURCE_NAME,
            EventLogSourceMode::ClassicApplicationLog,
            Some(error.to_string()),
        ),
    };
    EventLogSelfTestReport {
        schema_version: EVENT_LOG_SCHEMA_VERSION,
        operation: "event_log_self_test",
        success: error.is_none(),
        identifier: record.identifier.clone(),
        source_name,
        source_mode,
        event_id: EventLogStream::SelfTest.event_id(),
        error,
    }
}

#[derive(Debug, Serialize)]
struct EventLogEnvelope<'a, T: Serialize> {
    schema_version: u32,
    component: &'static str,
    source_mode: EventLogSourceMode,
    stream: EventLogStream,
    level: EventLogLevel,
    record: &'a T,
}

pub fn serialize_event_log_record<T: Serialize>(
    stream: EventLogStream,
    level: EventLogLevel,
    record: &T,
) -> Result<String, EventLogError> {
    serialize_event_log_record_with_mode(
        stream,
        level,
        EventLogSourceMode::ClassicApplicationLog,
        record,
    )
}

fn serialize_event_log_record_with_mode<T: Serialize>(
    stream: EventLogStream,
    level: EventLogLevel,
    source_mode: EventLogSourceMode,
    record: &T,
) -> Result<String, EventLogError> {
    let json = serde_json::to_string(&EventLogEnvelope {
        schema_version: EVENT_LOG_SCHEMA_VERSION,
        component: "ps5cam-service",
        source_mode,
        stream,
        level,
        record,
    })
    .map_err(EventLogError::Serialize)?;
    validate_event_log_payload(&json)?;
    Ok(json)
}

pub fn validate_event_log_payload(json: &str) -> Result<(), EventLogError> {
    if json.len() > MAX_EVENT_LOG_PAYLOAD_BYTES {
        return Err(EventLogError::PayloadTooLarge {
            actual: json.len(),
            maximum: MAX_EVENT_LOG_PAYLOAD_BYTES,
        });
    }
    let utf16_units = json.encode_utf16().count();
    if utf16_units > MAX_EVENT_LOG_INSERTION_UTF16_UNITS {
        return Err(EventLogError::InsertionStringTooLong {
            actual: utf16_units,
            maximum: MAX_EVENT_LOG_INSERTION_UTF16_UNITS,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum EventLogError {
    #[error("Windows Event Log is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("failed to serialize structured Event Log record: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("Event Log payload is too large: maximum {maximum} bytes, got {actual}")]
    PayloadTooLarge { actual: usize, maximum: usize },
    #[error(
        "Event Log insertion string is too long: maximum {maximum} UTF-16 units, got {actual}"
    )]
    InsertionStringTooLong { actual: usize, maximum: usize },
    #[error("RegisterEventSourceW failed with Win32 error {code}")]
    Open { code: u32 },
    #[error(
        "RegisterEventSourceW failed for both the preferred source (Win32 error {preferred_code}) and Application fallback (Win32 error {fallback_code})"
    )]
    OpenFallback {
        preferred_code: u32,
        fallback_code: u32,
    },
    #[error("ReportEventW failed with Win32 error {code}")]
    Write { code: u32 },
}

#[cfg(windows)]
struct EventSourceHandle(windows_sys::Win32::Foundation::HANDLE);

// ReportEvent operations are atomic, and callers serialize access through a
// mutable sink or an external Mutex. The owned handle is closed exactly once.
#[cfg(windows)]
unsafe impl Send for EventSourceHandle {}

#[cfg(windows)]
impl Drop for EventSourceHandle {
    fn drop(&mut self) {
        use windows_sys::Win32::System::EventLog::DeregisterEventSource;
        // SAFETY: the handle is owned and came from RegisterEventSourceW.
        let _ = unsafe { DeregisterEventSource(self.0) };
    }
}

pub struct WindowsEventLogSink {
    #[cfg(windows)]
    handle: EventSourceHandle,
    source_name: &'static str,
    source_mode: EventLogSourceMode,
}

impl fmt::Debug for WindowsEventLogSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsEventLogSink")
            .field("source_mode", &self.source_mode)
            .field("source_name", &self.source_name)
            .finish_non_exhaustive()
    }
}

impl WindowsEventLogSink {
    #[cfg(windows)]
    pub fn open() -> Result<Self, EventLogError> {
        use std::ptr;
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::System::EventLog::RegisterEventSourceW;

        let open_source = |source_name| {
            let source = wide_null(source_name);
            // SAFETY: source is NUL terminated; null server selects the local host.
            let handle = unsafe { RegisterEventSourceW(ptr::null(), source.as_ptr()) };
            if handle.is_null() {
                // SAFETY: called immediately after RegisterEventSourceW failed.
                Err(unsafe { GetLastError() })
            } else {
                Ok(EventSourceHandle(handle))
            }
        };

        match open_source(EVENT_LOG_SOURCE_NAME) {
            Ok(handle) => Ok(Self {
                handle,
                source_name: EVENT_LOG_SOURCE_NAME,
                source_mode: EventLogSourceMode::ClassicApplicationLog,
            }),
            Err(preferred_code) => match open_source(EVENT_LOG_APPLICATION_SOURCE_NAME) {
                Ok(handle) => Ok(Self {
                    handle,
                    source_name: EVENT_LOG_APPLICATION_SOURCE_NAME,
                    source_mode: EventLogSourceMode::ClassicApplicationLog,
                }),
                Err(fallback_code) => Err(EventLogError::OpenFallback {
                    preferred_code,
                    fallback_code,
                }),
            },
        }
    }

    #[cfg(not(windows))]
    pub fn open() -> Result<Self, EventLogError> {
        Err(EventLogError::UnsupportedPlatform)
    }

    pub const fn source_mode(&self) -> EventLogSourceMode {
        self.source_mode
    }

    pub const fn source_name(&self) -> &'static str {
        self.source_name
    }

    pub const fn write_receipt(&self) -> EventLogWriteReceipt {
        EventLogWriteReceipt {
            source_name: self.source_name,
            source_mode: self.source_mode,
        }
    }

    pub fn write_service(&mut self, record: &ServiceRecord) -> Result<(), EventLogError> {
        self.write_serialized(EventLogStream::Service, record.level.into(), record)
    }

    pub fn write_scm(&mut self, record: &ScmLogRecord) -> Result<(), EventLogError> {
        let level = match record.event {
            ScmEventKind::ReadinessBlocked | ScmEventKind::ControlIgnored => EventLogLevel::Warning,
            ScmEventKind::Stopped
                if record.win32_exit_code != 0 || record.service_specific_exit_code != 0 =>
            {
                EventLogLevel::Error
            }
            _ => EventLogLevel::Information,
        };
        self.write_serialized(EventLogStream::Scm, level, record)
    }

    pub fn write_device(&mut self, record: &WindowsDeviceEventRecord) -> Result<(), EventLogError> {
        self.write_serialized(
            EventLogStream::DeviceNotification,
            EventLogLevel::Information,
            record,
        )
    }

    pub fn write_self_test(
        &mut self,
        record: &EventLogSelfTestRecord,
    ) -> Result<(), EventLogError> {
        self.write_serialized(EventLogStream::SelfTest, EventLogLevel::Information, record)
    }

    fn write_serialized<T: Serialize>(
        &mut self,
        stream: EventLogStream,
        level: EventLogLevel,
        record: &T,
    ) -> Result<(), EventLogError> {
        let json = serialize_event_log_record_with_mode(stream, level, self.source_mode, record)?;
        self.write_json(stream, level, &json)
    }

    #[cfg(windows)]
    fn write_json(
        &mut self,
        stream: EventLogStream,
        level: EventLogLevel,
        json: &str,
    ) -> Result<(), EventLogError> {
        use std::{ffi::c_void, ptr};
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::System::EventLog::{
            ReportEventW, EVENTLOG_ERROR_TYPE, EVENTLOG_INFORMATION_TYPE, EVENTLOG_WARNING_TYPE,
        };

        let event_type = match level {
            EventLogLevel::Information => EVENTLOG_INFORMATION_TYPE,
            EventLogLevel::Warning => EVENTLOG_WARNING_TYPE,
            EventLogLevel::Error => EVENTLOG_ERROR_TYPE,
        };
        let insertion = wide_null(json);
        let insertion_pointers = [insertion.as_ptr()];
        let data_size = u32::try_from(json.len()).map_err(|_| EventLogError::PayloadTooLarge {
            actual: json.len(),
            maximum: MAX_EVENT_LOG_PAYLOAD_BYTES,
        })?;
        // SAFETY: the owned handle is valid, insertion/data buffers remain
        // live for the synchronous call, and their exact sizes are supplied.
        let result = unsafe {
            ReportEventW(
                self.handle.0,
                event_type,
                0,
                stream.event_id(),
                ptr::null_mut(),
                1,
                data_size,
                insertion_pointers.as_ptr(),
                json.as_ptr().cast::<c_void>(),
            )
        };
        if result == 0 {
            // SAFETY: called immediately after ReportEventW failed.
            Err(EventLogError::Write {
                code: unsafe { GetLastError() },
            })
        } else {
            Ok(())
        }
    }

    #[cfg(not(windows))]
    fn write_json(
        &mut self,
        _stream: EventLogStream,
        _level: EventLogLevel,
        _json: &str,
    ) -> Result<(), EventLogError> {
        Err(EventLogError::UnsupportedPlatform)
    }
}

impl StructuredEventSink for WindowsEventLogSink {
    fn emit(
        &mut self,
        record: &ServiceRecord,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.write_service(record)
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FailureCode, RecordKind, StateName};
    use serde::Serializer;
    use std::time::Duration;

    fn service_record(level: RecordLevel) -> ServiceRecord {
        ServiceRecord {
            sequence: 3,
            at: Duration::from_millis(40),
            level,
            kind: RecordKind::RetryScheduled,
            from: StateName::Uploading,
            to: StateName::RetryWaiting,
            attempt: Some(1),
            failure: Some(FailureCode::TransferTimeout),
        }
    }

    #[test]
    fn envelope_explicitly_marks_registry_neutral_fallback() {
        let json = serialize_event_log_record(
            EventLogStream::Service,
            EventLogLevel::Warning,
            &service_record(RecordLevel::Warning),
        )
        .unwrap();
        assert!(json.contains("classic_application_log"));
        assert!(json.contains("ps5cam-service"));
        assert!(json.contains("retry_scheduled"));
        assert!(json.contains("transfer_timeout"));
    }

    #[test]
    fn stream_ids_are_stable_and_distinct() {
        assert_eq!(EventLogStream::Service.event_id(), 0x1000);
        assert_eq!(EventLogStream::Scm.event_id(), 0x1001);
        assert_eq!(EventLogStream::DeviceNotification.event_id(), 0x1002);
        assert_eq!(EventLogStream::SelfTest.event_id(), 0x10ff);
    }

    struct Oversized;

    impl Serialize for Oversized {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&"x".repeat(MAX_EVENT_LOG_PAYLOAD_BYTES + 1))
        }
    }

    #[test]
    fn oversized_payload_is_rejected_before_windows_api_call() {
        assert!(matches!(
            serialize_event_log_record(
                EventLogStream::Service,
                EventLogLevel::Information,
                &Oversized
            ),
            Err(EventLogError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn insertion_string_limit_counts_utf16_units() {
        assert!(
            validate_event_log_payload(&"x".repeat(MAX_EVENT_LOG_INSERTION_UTF16_UNITS)).is_ok()
        );
        assert!(matches!(
            validate_event_log_payload(&"x".repeat(MAX_EVENT_LOG_INSERTION_UTF16_UNITS + 1)),
            Err(EventLogError::InsertionStringTooLong { actual, maximum })
                if actual == MAX_EVENT_LOG_INSERTION_UTF16_UNITS + 1
                    && maximum == MAX_EVENT_LOG_INSERTION_UTF16_UNITS
        ));
    }

    #[test]
    fn self_test_success_is_auditable_without_calling_windows() {
        let record = EventLogSelfTestRecord::new(42, 1_725_000_000_123);
        let mut writes = 0;
        let report = perform_event_log_self_test(&record, |written| {
            writes += 1;
            assert_eq!(written, &record);
            Ok(EventLogWriteReceipt {
                source_name: EVENT_LOG_SOURCE_NAME,
                source_mode: EventLogSourceMode::ClassicApplicationLog,
            })
        });

        assert_eq!(writes, 1);
        assert!(report.success);
        assert_eq!(report.exit_code(), 0);
        assert_eq!(report.event_id, 0x10ff);
        assert_eq!(report.identifier, record.identifier);
        assert_eq!(report.error, None);
    }

    #[test]
    fn self_test_failure_has_nonzero_exit_and_error() {
        let record = EventLogSelfTestRecord::new(7, 99);
        let report =
            perform_event_log_self_test(&record, |_| Err(EventLogError::Write { code: 5 }));

        assert!(!report.success);
        assert_eq!(report.exit_code(), 3);
        assert_eq!(
            report.error.as_deref(),
            Some("ReportEventW failed with Win32 error 5")
        );
    }

    #[test]
    fn self_test_payload_contains_its_searchable_identifier() {
        let record = EventLogSelfTestRecord::new(100, 200);
        let json = serialize_event_log_record(
            EventLogStream::SelfTest,
            EventLogLevel::Information,
            &record,
        )
        .unwrap();

        assert!(json.contains(&record.identifier));
        assert!(json.contains("event_log_self_test"));
    }
}
