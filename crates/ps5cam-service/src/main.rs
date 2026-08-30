use ps5cam_service::{
    host_readiness, parse_service_command, perform_event_log_self_test,
    run_windows_service_dispatcher, EventLogSelfTestRecord, ServiceCommand, WindowsEventLogSink,
};
use std::{
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1);
    match parse_service_command(arguments) {
        Ok(ServiceCommand::RunScm) => run_scm(),
        Ok(ServiceCommand::EventLogSelfTest) => run_event_log_self_test(),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "operation": "parse_command_line",
                    "success": false,
                    "error": error,
                })
            );
            ExitCode::from(64)
        }
    }
}

fn run_scm() -> ExitCode {
    match run_windows_service_dispatcher() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let diagnostic = serde_json::json!({
                "readiness": host_readiness(),
                "scm_error": error,
            });
            eprintln!("{diagnostic}");
            ExitCode::from(2)
        }
    }
}

fn run_event_log_self_test() -> ExitCode {
    let unix_time_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let record = EventLogSelfTestRecord::new(std::process::id(), unix_time_ms);
    let report = perform_event_log_self_test(&record, |record| {
        let mut sink = WindowsEventLogSink::open()?;
        sink.write_self_test(record)?;
        Ok(sink.write_receipt())
    });
    let exit_code = report.exit_code();
    match serde_json::to_string(&report) {
        Ok(json) => {
            println!("{json}");
            ExitCode::from(exit_code)
        }
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "schema_version": 1,
                    "operation": "event_log_self_test_report",
                    "success": false,
                    "error": error.to_string(),
                })
            );
            ExitCode::from(70)
        }
    }
}
