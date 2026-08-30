//! Capture contract shared by the Windows backend and deterministic tests.

use serde::Serialize;
use std::fmt;
use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Yuyv422,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: PixelFormat,
}

impl CaptureConfig {
    pub const fn mono_30() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            format: PixelFormat::Yuyv422,
        }
    }

    pub const fn stereo_30() -> Self {
        Self {
            width: 3840,
            height: 1080,
            fps: 30,
            format: PixelFormat::Yuyv422,
        }
    }
    pub fn frame_bytes(self) -> Result<usize, FrameError> {
        let pixels = (self.width as u64)
            .checked_mul(self.height as u64)
            .ok_or(FrameError::DimensionsOverflow)?;
        usize::try_from(
            pixels
                .checked_mul(2)
                .ok_or(FrameError::DimensionsOverflow)?,
        )
        .map_err(|_| FrameError::DimensionsOverflow)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum FrameError {
    DimensionsOverflow,
    InvalidLength { expected: usize, actual: usize },
    InvalidStereoWidth(u32),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionsOverflow => f.write_str("frame dimensions overflow"),
            Self::InvalidLength { expected, actual } => {
                write!(f, "invalid frame length: expected {expected}, got {actual}")
            }
            Self::InvalidStereoWidth(width) => write!(f, "stereo width must be even, got {width}"),
        }
    }
}
impl std::error::Error for FrameError {}

#[derive(Debug)]
pub struct UvcFrame {
    pub sequence: u64,
    pub timestamp_100ns: Option<i64>,
    pub config: CaptureConfig,
    pub data: Vec<u8>,
}

impl UvcFrame {
    pub fn new(
        sequence: u64,
        timestamp_100ns: Option<i64>,
        config: CaptureConfig,
        data: Vec<u8>,
    ) -> Result<Self, FrameError> {
        let expected = config.frame_bytes()?;
        if data.len() != expected {
            return Err(FrameError::InvalidLength {
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            sequence,
            timestamp_100ns,
            config,
            data,
        })
    }

    pub fn stats(&self) -> FrameStats {
        let mut min = u8::MAX;
        let mut max = 0_u8;
        let mut sum = 0_u64;
        let mut nonzero = 0_u64;
        let mut count = 0_u64;
        for chunk in self.data.chunks_exact(2) {
            let y = chunk[0];
            min = min.min(y);
            max = max.max(y);
            sum += u64::from(y);
            nonzero += u64::from(y != 0);
            count += 1;
        }
        FrameStats {
            mean_luma: if count == 0 {
                0.0
            } else {
                sum as f64 / count as f64
            },
            nonzero_fraction: if count == 0 {
                0.0
            } else {
                nonzero as f64 / count as f64
            },
            min_luma: if count == 0 { 0 } else { min },
            max_luma: if count == 0 { 0 } else { max },
        }
    }

    pub fn split_stereo(&self) -> Result<(Self, Self), FrameError> {
        if self.config.width % 2 != 0 {
            return Err(FrameError::InvalidStereoWidth(self.config.width));
        }
        let half_width = self.config.width / 2;
        let row_bytes = self.config.width as usize * 2;
        let half_row_bytes = half_width as usize * 2;
        let mut left = Vec::with_capacity(self.data.len() / 2);
        let mut right = Vec::with_capacity(self.data.len() / 2);
        for row in self.data.chunks_exact(row_bytes) {
            left.extend_from_slice(&row[..half_row_bytes]);
            right.extend_from_slice(&row[half_row_bytes..]);
        }
        let config = CaptureConfig {
            width: half_width,
            ..self.config
        };
        Ok((
            Self::new(self.sequence, self.timestamp_100ns, config, left)?,
            Self::new(self.sequence, self.timestamp_100ns, config, right)?,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FrameStats {
    pub mean_luma: f64,
    pub nonzero_fraction: f64,
    pub min_luma: u8,
    pub max_luma: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StreamReport {
    pub frames: u64,
    pub dropped_sequences: u64,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub timestamped_frames: u64,
    pub non_monotonic_timestamps: u64,
    pub largest_timestamp_gap_100ns: Option<i64>,
}

#[derive(Debug, Default)]
pub struct StreamTracker {
    report: StreamReport,
    last_timestamp_100ns: Option<i64>,
}

impl StreamTracker {
    pub fn observe(&mut self, frame: &UvcFrame) {
        if let Some(previous) = self.report.last_sequence {
            self.report.dropped_sequences +=
                frame.sequence.saturating_sub(previous.saturating_add(1));
        } else {
            self.report.first_sequence = Some(frame.sequence);
        }
        self.report.last_sequence = Some(frame.sequence);
        self.report.frames += 1;
        if let Some(timestamp) = frame.timestamp_100ns {
            self.report.timestamped_frames += 1;
            if let Some(previous) = self.last_timestamp_100ns {
                let gap = timestamp.saturating_sub(previous);
                if gap <= 0 {
                    self.report.non_monotonic_timestamps += 1;
                } else {
                    self.report.largest_timestamp_gap_100ns = Some(
                        self.report
                            .largest_timestamp_gap_100ns
                            .map_or(gap, |largest| largest.max(gap)),
                    );
                }
            }
            self.last_timestamp_100ns = Some(timestamp);
        }
    }

    pub const fn report(&self) -> StreamReport {
        self.report
    }
}

pub trait FrameSource {
    fn next_frame(&mut self) -> io::Result<UvcFrame>;
}

#[cfg(windows)]
mod media_foundation;

#[cfg(windows)]
pub mod platform {
    use super::{CaptureConfig, StreamReport, UvcFrame};
    use std::io::{self, BufReader, Read};
    use std::process::{Child, ChildStdout, Command, Stdio};

    pub const NAME: &str = "windows-directshow";
    pub use super::media_foundation::MediaFoundationCapture;

    pub struct DirectShowCapture {
        child: Child,
        output: BufReader<ChildStdout>,
        config: CaptureConfig,
        sequence: u64,
    }

    impl DirectShowCapture {
        pub fn start(
            ffmpeg: impl AsRef<std::ffi::OsStr>,
            device: &str,
            config: CaptureConfig,
        ) -> io::Result<Self> {
            let mut command = Command::new(ffmpeg);
            command
                .args([
                    "-nostdin",
                    "-thread_queue_size",
                    "512",
                    "-f",
                    "dshow",
                    "-rtbufsize",
                    "128M",
                    "-video_size",
                ])
                .arg(format!("{}x{}", config.width, config.height))
                .args(["-framerate"])
                .arg(config.fps.to_string())
                .arg("-i")
                .arg(format!("video={device}"))
                .args(["-pix_fmt", "yuyv422", "-f", "rawvideo", "pipe:1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = command.spawn()?;
            let output = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("ffmpeg stdout was not piped"))?;
            Ok(Self {
                child,
                output: BufReader::with_capacity(16 * 1024 * 1024, output),
                config,
                sequence: 0,
            })
        }

        pub fn next_frame(&mut self) -> io::Result<UvcFrame> {
            let mut data = vec![
                0_u8;
                self.config
                    .frame_bytes()
                    .map_err(|error| io::Error::other(error.to_string()))?
            ];
            self.output.read_exact(&mut data)?;
            let frame = UvcFrame::new(self.sequence, None, self.config, data)
                .map_err(|error| io::Error::other(error.to_string()))?;
            self.sequence = self.sequence.saturating_add(1);
            Ok(frame)
        }

        pub fn capture<F>(&mut self, frame_count: u64, mut sink: F) -> io::Result<StreamReport>
        where
            F: FnMut(&UvcFrame),
        {
            let mut tracker = super::StreamTracker::default();
            for _ in 0..frame_count {
                let frame = self.next_frame()?;
                tracker.observe(&frame);
                sink(&frame);
            }
            Ok(tracker.report())
        }

        pub fn stop(mut self) -> io::Result<()> {
            let _ = self.child.kill();
            self.child.wait().map(|_| ())
        }
    }

    impl super::FrameSource for DirectShowCapture {
        fn next_frame(&mut self) -> io::Result<UvcFrame> {
            DirectShowCapture::next_frame(self)
        }
    }
}
#[cfg(not(windows))]
pub mod platform {
    pub const NAME: &str = "unsupported-host";
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stereo_size_is_yuyv() {
        assert_eq!(CaptureConfig::stereo_30().frame_bytes().unwrap(), 8_294_400);
    }
    #[test]
    fn rejects_truncated_frame() {
        let c = CaptureConfig {
            width: 2,
            height: 1,
            fps: 30,
            format: PixelFormat::Yuyv422,
        };
        assert!(matches!(
            UvcFrame::new(1, None, c, vec![0; 3]),
            Err(FrameError::InvalidLength {
                expected: 4,
                actual: 3
            })
        ));
    }

    #[test]
    fn computes_stats_and_splits_stereo() {
        let config = CaptureConfig {
            width: 4,
            height: 1,
            fps: 30,
            format: PixelFormat::Yuyv422,
        };
        let frame = UvcFrame::new(
            7,
            Some(10),
            config,
            vec![10, 128, 20, 128, 30, 128, 40, 128],
        )
        .unwrap();
        assert_eq!(frame.stats().min_luma, 10);
        assert_eq!(frame.stats().max_luma, 40);
        assert_eq!(frame.stats().mean_luma, 25.0);
        let (left, right) = frame.split_stereo().unwrap();
        assert_eq!(left.data, vec![10, 128, 20, 128]);
        assert_eq!(right.data, vec![30, 128, 40, 128]);
    }

    #[test]
    fn tracks_sequence_gaps_without_guessing_timestamps() {
        let config = CaptureConfig {
            width: 2,
            height: 1,
            fps: 30,
            format: PixelFormat::Yuyv422,
        };
        let mut tracker = StreamTracker::default();
        for (sequence, timestamp) in [(10, 100), (11, 433_333), (14, 433_333)] {
            tracker.observe(
                &UvcFrame::new(sequence, Some(timestamp), config, vec![1, 128, 2, 128]).unwrap(),
            );
        }
        assert_eq!(
            tracker.report(),
            StreamReport {
                frames: 3,
                dropped_sequences: 2,
                first_sequence: Some(10),
                last_sequence: Some(14),
                timestamped_frames: 3,
                non_monotonic_timestamps: 1,
                largest_timestamp_gap_100ns: Some(433_233),
            }
        );
    }
}
