use clap::{Parser, Subcommand};
use ov580_fw_analyzer::{analyze_file, diff_files, AnalysisConfig, AnalyzerError, DiffConfig};
use serde::Serialize;
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "ov580-fw-analyzer")]
#[command(about = "Clean-room structural analysis of caller-supplied firmware images")]
struct Cli {
    #[arg(long, global = true, help = "Emit compact rather than pretty JSON")]
    compact: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Analyze {
        input: PathBuf,
        #[arg(long, default_value_t = 256)]
        entropy_window_size: usize,
        #[arg(long, default_value_t = 256)]
        entropy_stride: usize,
        #[arg(long, default_value_t = 256)]
        region_block_size: usize,
        #[arg(long, default_value_t = 4)]
        minimum_string_length: usize,
    },
    Diff {
        left: PathBuf,
        right: PathBuf,
        #[arg(long, default_value_t = 256)]
        block_size: usize,
    },
}

#[derive(Debug, Serialize)]
struct ErrorDocument {
    schema_version: u32,
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: String,
    message: String,
    path: Option<String>,
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        let document = ErrorDocument {
            schema_version: ov580_fw_analyzer::SCHEMA_VERSION,
            error: ErrorBody {
                code: error.code().into(),
                message: error.to_string(),
                path: error.path().map(|path| path.to_string_lossy().into_owned()),
            },
        };
        let _ = serde_json::to_writer_pretty(io::stderr().lock(), &document);
        let _ = writeln!(io::stderr().lock());
        std::process::exit(2);
    }
}

fn run(cli: Cli) -> Result<(), AnalyzerError> {
    match cli.command {
        Command::Analyze {
            input,
            entropy_window_size,
            entropy_stride,
            region_block_size,
            minimum_string_length,
        } => {
            let report = analyze_file(
                input,
                AnalysisConfig {
                    entropy_window_size,
                    entropy_stride,
                    region_block_size,
                    minimum_string_length,
                },
            )?;
            write_json(&report, cli.compact)
        }
        Command::Diff {
            left,
            right,
            block_size,
        } => {
            let report = diff_files(left, right, DiffConfig { block_size })?;
            write_json(&report, cli.compact)
        }
    }
}

fn write_json(value: &impl Serialize, compact: bool) -> Result<(), AnalyzerError> {
    let bytes = if compact {
        serde_json::to_vec(value)
    } else {
        serde_json::to_vec_pretty(value)
    }
    .map_err(|error| AnalyzerError::Io {
        operation: "serialize",
        path: PathBuf::from("<stdout>"),
        message: error.to_string(),
    })?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&bytes)
        .map_err(|error| AnalyzerError::Io {
            operation: "write",
            path: PathBuf::from("<stdout>"),
            message: error.to_string(),
        })?;
    stdout.write_all(b"\n").map_err(|error| AnalyzerError::Io {
        operation: "write",
        path: PathBuf::from("<stdout>"),
        message: error.to_string(),
    })
}
