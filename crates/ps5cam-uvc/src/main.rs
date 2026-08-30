use clap::{Parser, ValueEnum};
use ps5cam_uvc::{CaptureConfig, FrameSource, FrameStats, StreamReport, StreamTracker};
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(name = "ps5cam-uvc-capture")]
#[command(about = "Capture and summarize UVC frames without writing to the camera")]
struct Cli {
    #[arg(long, default_value = "USB Camera-OV580")]
    device: String,
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: String,
    #[arg(long, value_enum, default_value_t = Backend::MediaFoundation)]
    backend: Backend,
    #[arg(long, value_enum, default_value_t = Mode::Mono)]
    mode: Mode,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    frames: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Mode {
    Mono,
    Stereo,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Backend {
    MediaFoundation,
    Directshow,
}

#[derive(Debug, Serialize)]
struct SideSummary {
    mean_luma_min: f64,
    mean_luma_max: f64,
    nonzero_fraction_min: f64,
}

#[derive(Debug, Serialize)]
struct Summary {
    device: String,
    backend: String,
    mode: String,
    requested_frames: u64,
    elapsed_millis: u128,
    effective_fps: f64,
    report: StreamReport,
    whole_frame: SideSummary,
    left: Option<SideSummary>,
    right: Option<SideSummary>,
}

#[derive(Debug, Clone, Copy)]
struct Accumulator {
    mean_min: f64,
    mean_max: f64,
    nonzero_min: f64,
}

impl Default for Accumulator {
    fn default() -> Self {
        Self {
            mean_min: f64::INFINITY,
            mean_max: f64::NEG_INFINITY,
            nonzero_min: f64::INFINITY,
        }
    }
}

impl Accumulator {
    fn observe(&mut self, stats: FrameStats) {
        self.mean_min = self.mean_min.min(stats.mean_luma);
        self.mean_max = self.mean_max.max(stats.mean_luma);
        self.nonzero_min = self.nonzero_min.min(stats.nonzero_fraction);
    }

    fn finish(self) -> SideSummary {
        SideSummary {
            mean_luma_min: self.mean_min,
            mean_luma_max: self.mean_max,
            nonzero_fraction_min: self.nonzero_min,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    run(cli)
}

fn summarize<C: FrameSource>(
    capture: &mut C,
    frame_count: u64,
    mode: Mode,
) -> std::io::Result<(
    StreamReport,
    Accumulator,
    Accumulator,
    Accumulator,
    std::time::Duration,
)> {
    let started = Instant::now();
    let mut tracker = StreamTracker::default();
    let mut whole = Accumulator::default();
    let mut left = Accumulator::default();
    let mut right = Accumulator::default();
    for _ in 0..frame_count {
        let frame = capture.next_frame()?;
        tracker.observe(&frame);
        whole.observe(frame.stats());
        if matches!(mode, Mode::Stereo) {
            let (l, r) = frame
                .split_stereo()
                .expect("configured stereo width is even");
            left.observe(l.stats());
            right.observe(r.stats());
        }
    }
    Ok((tracker.report(), whole, left, right, started.elapsed()))
}

#[cfg(windows)]
fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    use ps5cam_uvc::platform::{DirectShowCapture, MediaFoundationCapture};

    let config = match cli.mode {
        Mode::Mono => CaptureConfig::mono_30(),
        Mode::Stereo => CaptureConfig::stereo_30(),
    };
    let (report, whole, left, right, elapsed) = match cli.backend {
        Backend::MediaFoundation => {
            let mut capture = MediaFoundationCapture::start(&cli.device, config)?;
            summarize(&mut capture, cli.frames, cli.mode)?
        }
        Backend::Directshow => {
            let mut capture = DirectShowCapture::start(&cli.ffmpeg, &cli.device, config)?;
            let summary = summarize(&mut capture, cli.frames, cli.mode)?;
            capture.stop()?;
            summary
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&Summary {
            device: cli.device,
            backend: match cli.backend {
                Backend::MediaFoundation => "media_foundation",
                Backend::Directshow => "directshow",
            }
            .into(),
            mode: match cli.mode {
                Mode::Mono => "mono",
                Mode::Stereo => "stereo",
            }
            .into(),
            requested_frames: cli.frames,
            elapsed_millis: elapsed.as_millis(),
            effective_fps: report.frames as f64 / elapsed.as_secs_f64(),
            report,
            whole_frame: whole.finish(),
            left: matches!(cli.mode, Mode::Stereo).then(|| left.finish()),
            right: matches!(cli.mode, Mode::Stereo).then(|| right.finish()),
        })?
    );
    Ok(())
}

#[cfg(not(windows))]
fn run(_: Cli) -> Result<(), Box<dyn std::error::Error>> {
    Err("ps5cam-uvc-capture requires Windows DirectShow".into())
}
