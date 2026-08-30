use clap::Parser;
use ps5cam_diagnostics::{
    collect, validate_probe_timeout_ms, TimeoutValidationError, DEFAULT_PROBE_TIMEOUT_MS,
};
use serde::Serialize;
use std::{io, process::ExitCode, time::Duration};

#[derive(Debug, Parser)]
#[command(about = "Read-only PS5 camera Windows readiness diagnostics")]
struct Arguments {
    #[arg(long, default_value_t = DEFAULT_PROBE_TIMEOUT_MS)]
    timeout_ms: u64,

    #[arg(long, default_value = "PS5CameraService")]
    service_name: String,

    #[arg(long)]
    compact: bool,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let timeout = match validated_timeout(&arguments) {
        Ok(timeout) => timeout,
        Err(error) => return write_input_error(&error),
    };
    let report = collect(timeout, &arguments.service_name);
    let result = if arguments.compact {
        serde_json::to_writer(io::stdout().lock(), &report)
    } else {
        serde_json::to_writer_pretty(io::stdout().lock(), &report)
    };
    match result {
        Ok(()) => {
            println!();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("ps5cam-diagnostics: failed to write JSON: {error}");
            ExitCode::FAILURE
        }
    }
}

fn validated_timeout(arguments: &Arguments) -> Result<Duration, TimeoutValidationError> {
    validate_probe_timeout_ms(arguments.timeout_ms)
}

#[derive(Debug, Serialize)]
struct InputErrorEnvelope<'a> {
    success: bool,
    operation: &'static str,
    error: &'a TimeoutValidationError,
}

fn write_input_error(error: &TimeoutValidationError) -> ExitCode {
    let envelope = InputErrorEnvelope {
        success: false,
        operation: "validate_arguments",
        error,
    };
    if serde_json::to_writer(io::stderr().lock(), &envelope).is_ok() {
        eprintln!();
    }
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps5cam_diagnostics::MAX_PROBE_TIMEOUT_MS;

    fn arguments(timeout_ms: u64) -> Arguments {
        Arguments {
            timeout_ms,
            service_name: "PS5CameraService".to_owned(),
            compact: false,
        }
    }

    #[test]
    fn cli_timeout_rejects_zero() {
        let error = validated_timeout(&arguments(0)).expect_err("zero is not operational");
        assert_eq!(error.code, "invalid_timeout_ms");
        assert_eq!(error.provided_ms, 0);
    }

    #[test]
    fn cli_timeout_accepts_the_inclusive_limit() {
        assert_eq!(
            validated_timeout(&arguments(MAX_PROBE_TIMEOUT_MS)),
            Ok(Duration::from_millis(MAX_PROBE_TIMEOUT_MS))
        );
    }

    #[test]
    fn cli_timeout_rejects_extreme_input() {
        let error = validated_timeout(&arguments(u64::MAX)).expect_err("extreme timeout must fail");
        assert_eq!(error.provided_ms, u64::MAX);
        assert_eq!(error.maximum_ms, MAX_PROBE_TIMEOUT_MS);
    }
}
