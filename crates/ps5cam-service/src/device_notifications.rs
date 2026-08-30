use crate::DeviceEvent;
use ps5cam_usb::DeviceMode;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsDeviceChange {
    Arrival,
    RemovalComplete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedUsbInterface {
    pub mode: DeviceMode,
    pub device_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowsDeviceEventRecord {
    pub sequence: u64,
    pub source: &'static str,
    pub event: DeviceEvent,
}

impl WindowsDeviceEventRecord {
    pub fn new(sequence: u64, event: DeviceEvent) -> Self {
        Self {
            sequence,
            source: "windows_device_interface",
            event,
        }
    }
}

/// Parses a Windows USB device-interface path such as
/// `\\?\USB#VID_05A9&PID_0580#...`. No display name or localized PnP text is
/// used for classification.
pub fn parse_supported_usb_interface(path: &str) -> Option<SupportedUsbInterface> {
    let normalized = path.to_ascii_uppercase();
    if !(normalized.starts_with("\\\\?\\USB#") || normalized.starts_with("\\\\.\\USB#")) {
        return None;
    }

    let hardware_id = normalized.split('#').nth(1)?;
    let mut vendor_id = None;
    let mut product_id = None;
    for component in hardware_id.split('&') {
        if let Some(value) = component.strip_prefix("VID_") {
            if vendor_id.is_some() {
                return None;
            }
            vendor_id = Some(parse_id(value)?);
        } else if let Some(value) = component.strip_prefix("PID_") {
            if product_id.is_some() {
                return None;
            }
            product_id = Some(parse_id(value)?);
        }
    }
    let mode = DeviceMode::from_ids(vendor_id?, product_id?)?;
    Some(SupportedUsbInterface {
        mode,
        device_path: path.to_owned(),
    })
}

pub fn translate_windows_device_change(
    change: WindowsDeviceChange,
    path: &str,
    at: Duration,
) -> Option<DeviceEvent> {
    let interface = parse_supported_usb_interface(path)?;
    Some(match change {
        WindowsDeviceChange::Arrival => DeviceEvent::Arrived {
            mode: interface.mode,
            instance_id: interface.device_path,
            // A device-interface notification does not carry controller/port
            // topology. The service consequently fails closed until a trusted
            // discovery adapter supplies the stable locator.
            locator: None,
            at,
        },
        WindowsDeviceChange::RemovalComplete => DeviceEvent::Removed {
            instance_id: interface.device_path,
            at,
        },
    })
}

fn parse_id(value: &str) -> Option<u16> {
    if value.len() != 4 {
        return None;
    }
    u16::from_str_radix(value, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT_PATH: &str =
        r"\\?\USB#VID_05A9&PID_0580#5&1de99128&0&3#{a5dcbf10-6530-11d2-901f-00c04fb951ed}";
    const CAMERA_PATH: &str =
        r"\\?\usb#vid_05a9&pid_058c&rev_0100#camera#{a5dcbf10-6530-11d2-901f-00c04fb951ed}";

    #[test]
    fn parses_only_supported_usb_interface_paths() {
        assert_eq!(
            parse_supported_usb_interface(BOOT_PATH),
            Some(SupportedUsbInterface {
                mode: DeviceMode::Boot,
                device_path: BOOT_PATH.to_owned(),
            })
        );
        assert_eq!(
            parse_supported_usb_interface(CAMERA_PATH)
                .expect("camera interface")
                .mode,
            DeviceMode::Camera
        );
        assert_eq!(
            parse_supported_usb_interface(
                r"\\?\USB#VID_FFFF&PID_0580#other#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"
            ),
            None
        );
        assert_eq!(
            parse_supported_usb_interface(r"USB\VID_05A9&PID_0580\instance"),
            None
        );
        assert_eq!(
            parse_supported_usb_interface(r"\\?\USB#VID_05A#PID_0580"),
            None
        );
    }

    #[test]
    fn classification_uses_only_the_first_hardware_id_segment() {
        let unsupported_first =
            r"\\?\USB#VID_FFFF&PID_FFFF#VID_05A9&PID_0580#{a5dcbf10-6530-11d2-901f-00c04fb951ed}";
        assert_eq!(parse_supported_usb_interface(unsupported_first), None);

        let conflicting_instance =
            r"\\?\USB#VID_05A9&PID_0580#VID_FFFF&PID_058C#{a5dcbf10-6530-11d2-901f-00c04fb951ed}";
        assert_eq!(
            parse_supported_usb_interface(conflicting_instance)
                .expect("the first hardware ID is authoritative")
                .mode,
            DeviceMode::Boot
        );
    }

    #[test]
    fn duplicate_or_malformed_ids_in_first_segment_are_rejected() {
        assert_eq!(
            parse_supported_usb_interface(r"\\?\USB#VID_05A9&VID_05A9&PID_0580#instance"),
            None
        );
        assert_eq!(
            parse_supported_usb_interface(r"\\?\USB#VID_05A90&PID_0580#instance"),
            None
        );
        assert_eq!(
            parse_supported_usb_interface(r"\\?\USB#VID_05A9#PID_0580#instance"),
            None
        );
    }

    #[test]
    fn translates_arrival_and_removal_to_core_events() {
        let at = Duration::from_millis(42);
        assert_eq!(
            translate_windows_device_change(WindowsDeviceChange::Arrival, BOOT_PATH, at),
            Some(DeviceEvent::Arrived {
                mode: DeviceMode::Boot,
                instance_id: BOOT_PATH.to_owned(),
                locator: None,
                at,
            })
        );
        assert_eq!(
            translate_windows_device_change(WindowsDeviceChange::RemovalComplete, CAMERA_PATH, at),
            Some(DeviceEvent::Removed {
                instance_id: CAMERA_PATH.to_owned(),
                at,
            })
        );
    }

    #[test]
    fn typed_notification_record_is_structured_json() {
        let event = translate_windows_device_change(
            WindowsDeviceChange::Arrival,
            CAMERA_PATH,
            Duration::from_secs(1),
        )
        .unwrap();
        let json = serde_json::to_string(&WindowsDeviceEventRecord::new(7, event)).unwrap();
        assert!(json.contains("windows_device_interface"));
        assert!(json.contains("camera"));
        assert!(json.contains("arrived"));
    }
}
