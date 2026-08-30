use clap::Parser;
use ps5cam_usb::{probe, ProbeStatus};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(
    name = "ps5cam-probe",
    version,
    about = "Collect a reproducible USB descriptor report for the PS5 CFI-ZEY1 camera"
)]
struct Arguments {
    /// JSON report path. Omit it to write the report to stdout.
    output: Option<PathBuf>,

    /// Directory for versioned binary descriptor dumps.
    #[arg(long, value_name = "DIRECTORY")]
    dump_dir: Option<PathBuf>,

    /// Timeout for each USB control transfer.
    #[arg(long, default_value_t = 1000, value_name = "MILLISECONDS")]
    timeout_ms: u64,

    /// Emit compact JSON instead of human-readable indented JSON.
    #[arg(long)]
    compact: bool,
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Probe(#[from] ps5cam_usb::ProbeError),
    #[error(transparent)]
    EncodeDump(#[from] ps5cam_usb::DumpEncodeError),
    #[error("failed to serialize the JSON report: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("failed to create directory {path}: {source}")]
    CreateDirectory { path: PathBuf, source: io::Error },
    #[error("failed to write {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
    #[error("failed to write the report to stdout: {0}")]
    WriteStdout(io::Error),
}

fn main() {
    if let Err(error) = run(Arguments::parse()) {
        eprintln!("ps5cam-probe: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: Arguments) -> Result<(), CliError> {
    let snapshot = probe(Duration::from_millis(arguments.timeout_ms))?;
    let json = if arguments.compact {
        serde_json::to_vec(&snapshot.report)?
    } else {
        serde_json::to_vec_pretty(&snapshot.report)?
    };

    match &arguments.output {
        Some(path) => write_file(path, &json)?,
        None => {
            let mut stdout = io::stdout().lock();
            stdout.write_all(&json).map_err(CliError::WriteStdout)?;
            stdout.write_all(b"\n").map_err(CliError::WriteStdout)?;
        }
    }

    if let Some(dump_dir) = &arguments.dump_dir {
        fs::create_dir_all(dump_dir).map_err(|source| CliError::CreateDirectory {
            path: dump_dir.clone(),
            source,
        })?;

        for dump in &snapshot.dumps {
            write_file(&dump_dir.join(&dump.file_name), &dump.encode()?)?;
        }
    }

    emit_summary(snapshot.report.status, snapshot.report.devices.len());
    Ok(())
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| CliError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    fs::write(path, contents).map_err(|source| CliError::WriteFile {
        path: path.to_path_buf(),
        source,
    })
}

fn emit_summary(status: ProbeStatus, device_count: usize) {
    let message = match status {
        ProbeStatus::Absent => "PS5 camera not found",
        ProbeStatus::Boot => "PS5 camera found in OV580 boot mode",
        ProbeStatus::Camera => "PS5 camera found in UVC camera mode",
        ProbeStatus::Mixed => "PS5 cameras found in both boot and UVC camera modes",
    };
    eprintln!("{message} ({device_count} matching device(s))");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_output_form() {
        let arguments =
            Arguments::try_parse_from(["ps5cam-probe", "report.json", "--dump-dir", "descriptors"])
                .expect("valid arguments");

        assert_eq!(arguments.output, Some(PathBuf::from("report.json")));
        assert_eq!(arguments.dump_dir, Some(PathBuf::from("descriptors")));
        assert_eq!(arguments.timeout_ms, 1000);
        assert!(!arguments.compact);
    }
}
