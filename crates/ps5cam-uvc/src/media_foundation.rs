use crate::{CaptureConfig, FrameSource, UvcFrame};
use std::{ffi::c_void, io, ptr};
use windows::{
    core::PWSTR,
    Win32::{
        Media::MediaFoundation::{
            IMFActivate, IMFMediaSource, IMFSourceReader, MFCreateAttributes, MFCreateMediaType,
            MFCreateSourceReaderFromMediaSource, MFEnumDeviceSources, MFMediaType_Video,
            MFShutdown, MFStartup, MFVideoFormat_YUY2, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
            MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
            MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
        },
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
    },
};

pub struct MediaFoundationCapture {
    reader: IMFSourceReader,
    source: IMFMediaSource,
    config: CaptureConfig,
    sequence: u64,
}

impl MediaFoundationCapture {
    pub fn start(device_name: &str, config: CaptureConfig) -> io::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(error)?;
            if let Err(value) = MFStartup(MF_VERSION, 0) {
                CoUninitialize();
                return Err(error(value));
            }
            let result = Self::open(device_name, config);
            if result.is_err() {
                let _ = MFShutdown();
                CoUninitialize();
            }
            result
        }
    }

    unsafe fn open(device_name: &str, config: CaptureConfig) -> io::Result<Self> {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1).map_err(error)?;
        let attributes = attributes
            .ok_or_else(|| io::Error::other("Media Foundation returned null attributes"))?;
        attributes
            .SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )
            .map_err(error)?;

        let mut raw_devices: *mut Option<IMFActivate> = ptr::null_mut();
        let mut count = 0_u32;
        MFEnumDeviceSources(&attributes, &mut raw_devices, &mut count).map_err(error)?;
        let devices = std::slice::from_raw_parts_mut(raw_devices, count as usize);
        let mut selected = None;
        for activate in devices.iter().flatten() {
            if allocated_string(activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)? == device_name {
                selected = Some(activate.clone());
                break;
            }
        }
        CoTaskMemFree(Some(raw_devices.cast::<c_void>()));
        let activate = selected.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("UVC device not found: {device_name}"),
            )
        })?;
        let source = activate.ActivateObject::<IMFMediaSource>().map_err(error)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, None).map_err(error)?;

        let media_type = MFCreateMediaType().map_err(error)?;
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(error)?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_YUY2)
            .map_err(error)?;
        media_type
            .SetUINT64(
                &MF_MT_FRAME_SIZE,
                pack_u32_pair(config.width, config.height),
            )
            .map_err(error)?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack_u32_pair(config.fps, 1))
            .map_err(error)?;
        reader
            .SetCurrentMediaType(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                None,
                &media_type,
            )
            .map_err(error)?;
        Ok(Self {
            reader,
            source,
            config,
            sequence: 0,
        })
    }

    pub fn next_frame(&mut self) -> io::Result<UvcFrame> {
        unsafe {
            for _ in 0..128 {
                let mut stream_index = 0_u32;
                let mut flags = 0_u32;
                let mut timestamp = 0_i64;
                let mut sample = None;
                self.reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        Some(&mut stream_index),
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(error)?;
                let Some(sample) = sample else {
                    continue;
                };
                let buffer = sample.ConvertToContiguousBuffer().map_err(error)?;
                let mut data = ptr::null_mut();
                let mut length = 0_u32;
                buffer
                    .Lock(&mut data, None, Some(&mut length))
                    .map_err(error)?;
                let bytes = std::slice::from_raw_parts(data, length as usize).to_vec();
                buffer.Unlock().map_err(error)?;
                let frame = UvcFrame::new(self.sequence, Some(timestamp), self.config, bytes)
                    .map_err(|value| io::Error::other(value.to_string()))?;
                self.sequence = self.sequence.saturating_add(1);
                return Ok(frame);
            }
            Err(io::Error::other(
                "Media Foundation did not produce a video sample",
            ))
        }
    }
}

impl FrameSource for MediaFoundationCapture {
    fn next_frame(&mut self) -> io::Result<UvcFrame> {
        MediaFoundationCapture::next_frame(self)
    }
}

impl Drop for MediaFoundationCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.source.Shutdown();
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

unsafe fn allocated_string(
    activate: &IMFActivate,
    key: &windows::core::GUID,
) -> io::Result<String> {
    let mut value = PWSTR::null();
    let mut length = 0_u32;
    activate
        .GetAllocatedString(key, &mut value, &mut length)
        .map_err(error)?;
    let result = value
        .to_string()
        .map_err(|value| io::Error::other(value.to_string()));
    CoTaskMemFree(Some(value.0.cast::<c_void>()));
    result
}

const fn pack_u32_pair(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

fn error(value: windows::core::Error) -> io::Error {
    io::Error::other(value.to_string())
}
