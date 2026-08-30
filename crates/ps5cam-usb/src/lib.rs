//! Low-level USB discovery and descriptor probing for the PS5 CFI-ZEY1 camera.
//!
//! This crate deliberately does not upload firmware. It establishes a stable,
//! testable description of the device states that higher-level tools can use.

use rusb::{
    Device, DeviceDescriptor, DeviceHandle, Direction, GlobalContext, Recipient, RequestType,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

pub const OV580_VENDOR_ID: u16 = 0x05a9;
pub const OV580_BOOT_PID: u16 = 0x0580;
pub const OV580_CAMERA_PID: u16 = 0x058c;
pub const REPORT_SCHEMA_VERSION: u32 = 1;
pub const DUMP_SCHEMA_VERSION: u16 = 1;
const DUMP_MAGIC: &[u8; 8] = b"PS5CAMD1";
const GET_DESCRIPTOR: u8 = 0x06;
const DEVICE_DESCRIPTOR_TYPE: u8 = 0x01;
const CONFIGURATION_DESCRIPTOR_TYPE: u8 = 0x02;
const BOS_DESCRIPTOR_TYPE: u8 = 0x0f;

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("failed to enumerate USB devices: {0}")]
    Enumeration(#[source] rusb::Error),
    #[error("system clock is before the Unix epoch")]
    InvalidSystemClock,
    #[error("system clock timestamp does not fit in the report schema")]
    TimestampOutOfRange,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DumpEncodeError {
    #[error("raw descriptor record {record_index} contains invalid hexadecimal data")]
    InvalidHex { record_index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceMode {
    Boot,
    Camera,
}

impl DeviceMode {
    pub fn from_ids(vendor_id: u16, product_id: u16) -> Option<Self> {
        if vendor_id != OV580_VENDOR_ID {
            return None;
        }

        match product_id {
            OV580_BOOT_PID => Some(Self::Boot),
            OV580_CAMERA_PID => Some(Self::Camera),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Absent,
    Boot,
    Camera,
    Mixed,
}

impl ProbeStatus {
    fn from_devices(devices: &[UsbDeviceReport]) -> Self {
        let boot = devices.iter().any(|device| device.mode == DeviceMode::Boot);
        let camera = devices
            .iter()
            .any(|device| device.mode == DeviceMode::Camera);

        match (boot, camera) {
            (false, false) => Self::Absent,
            (true, false) => Self::Boot,
            (false, true) => Self::Camera,
            (true, true) => Self::Mixed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub schema_version: u32,
    pub captured_at_unix_ms: u64,
    pub host: HostReport,
    pub status: ProbeStatus,
    pub devices: Vec<UsbDeviceReport>,
    pub issues: Vec<ProbeIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostReport {
    pub os: String,
    pub architecture: String,
    pub os_version: Option<String>,
    pub libusb_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbDeviceReport {
    pub mode: DeviceMode,
    /// Windows PnP metadata, when ConfigManager can correlate or recover a
    /// supported device that libusb cannot currently open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_pnp: Option<WindowsPnpReport>,
    pub locator: DeviceLocator,
    pub descriptor: DeviceDescriptorReport,
    pub strings: DeviceStrings,
    pub configurations: Vec<ConfigurationReport>,
    pub raw_descriptors: Vec<RawDescriptorReport>,
    pub descriptor_dump: Option<DescriptorDumpReport>,
    pub issues: Vec<ProbeIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsPnpReport {
    pub instance_id: String,
    pub status: Option<u32>,
    pub problem_code: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescriptorDumpReport {
    pub format: String,
    pub schema_version: u16,
    pub file_name: String,
    pub record_count: usize,
    pub length: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLocator {
    pub controller_id: String,
    pub bus_number: u8,
    pub device_address: u8,
    pub port_number: u8,
    pub port_path: Vec<u8>,
    pub speed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDescriptorReport {
    pub length: u8,
    pub descriptor_type: u8,
    pub usb_version: String,
    pub device_version: String,
    pub class_code: u8,
    pub sub_class_code: u8,
    pub protocol_code: u8,
    pub max_packet_size_0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer_string_index: Option<u8>,
    pub product_string_index: Option<u8>,
    pub serial_number_string_index: Option<u8>,
    pub num_configurations: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStrings {
    pub language_id: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationReport {
    pub index: u8,
    pub number: u8,
    pub total_length: u16,
    pub num_interfaces: u8,
    pub max_power_ma: u16,
    pub self_powered: bool,
    pub remote_wakeup: bool,
    pub description_string_index: Option<u8>,
    pub extra_hex: String,
    pub interfaces: Vec<InterfaceReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceReport {
    pub number: u8,
    pub alternate_settings: Vec<AlternateSettingReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlternateSettingReport {
    pub setting_number: u8,
    pub class_code: u8,
    pub sub_class_code: u8,
    pub protocol_code: u8,
    pub description_string_index: Option<u8>,
    pub extra_hex: String,
    pub endpoints: Vec<EndpointReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointReport {
    pub address: u8,
    pub number: u8,
    pub direction: String,
    pub transfer_type: String,
    pub sync_type: String,
    pub usage_type: String,
    pub max_packet_size: u16,
    pub interval: u8,
    pub extra_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum RawDescriptorKind {
    Device = DEVICE_DESCRIPTOR_TYPE,
    Configuration = CONFIGURATION_DESCRIPTOR_TYPE,
    Bos = BOS_DESCRIPTOR_TYPE,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawDescriptorReport {
    pub kind: RawDescriptorKind,
    pub index: u8,
    pub length: usize,
    pub sha256: String,
    pub bytes_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeIssue {
    pub severity: IssueSeverity,
    pub operation: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorDump {
    pub file_name: String,
    pub records: Vec<RawDescriptorReport>,
}

impl DescriptorDump {
    pub fn encode(&self) -> Result<Vec<u8>, DumpEncodeError> {
        encode_dump(&self.records)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSnapshot {
    pub report: ProbeReport,
    pub dumps: Vec<DescriptorDump>,
}

pub fn probe(timeout: Duration) -> Result<ProbeSnapshot, ProbeError> {
    let captured_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProbeError::InvalidSystemClock)?
        .as_millis();
    let captured_at_unix_ms =
        u64::try_from(captured_at_unix_ms).map_err(|_| ProbeError::TimestampOutOfRange)?;
    let devices = rusb::devices().map_err(ProbeError::Enumeration)?;
    let mut reports = Vec::new();
    let mut dumps = Vec::new();
    let mut top_level_issues = Vec::new();

    for device in devices.iter() {
        let descriptor = match device.device_descriptor() {
            Ok(descriptor) => descriptor,
            Err(error) => {
                top_level_issues.push(issue(
                    IssueSeverity::Warning,
                    "read_device_descriptor",
                    error,
                ));
                continue;
            }
        };

        let Some(mode) = DeviceMode::from_ids(descriptor.vendor_id(), descriptor.product_id())
        else {
            continue;
        };

        let (report, dump) = probe_device(&device, &descriptor, mode, timeout);
        reports.push(report);
        dumps.push(dump);
    }

    let pnp_scan = scan_windows_pnp();
    top_level_issues.extend(pnp_scan.issues);
    merge_pnp_devices(&mut reports, pnp_scan.devices);

    reports.sort_by_key(|report| {
        (
            report.mode as u8,
            report.locator.bus_number,
            report.locator.port_path.clone(),
            report.locator.device_address,
        )
    });
    dumps.sort_by(|left, right| left.file_name.cmp(&right.file_name));

    Ok(ProbeSnapshot {
        report: ProbeReport {
            schema_version: REPORT_SCHEMA_VERSION,
            captured_at_unix_ms,
            host: host_report(),
            status: ProbeStatus::from_devices(&reports),
            devices: reports,
            issues: top_level_issues,
        },
        dumps,
    })
}

fn probe_device(
    device: &Device<GlobalContext>,
    descriptor: &DeviceDescriptor,
    mode: DeviceMode,
    timeout: Duration,
) -> (UsbDeviceReport, DescriptorDump) {
    let bus_number = device.bus_number();
    let device_address = device.address();
    let mut issues = Vec::new();
    let port_path = match device.port_numbers() {
        Ok(port_path) => port_path,
        Err(error) => {
            issues.push(issue(IssueSeverity::Warning, "read_usb_port_path", error));
            Vec::new()
        }
    };
    let locator = DeviceLocator {
        controller_id: format!("libusb-bus-{bus_number}"),
        bus_number,
        device_address,
        port_number: device.port_number(),
        port_path,
        speed: enum_name(device.speed()),
    };
    let descriptor_report = map_device_descriptor(descriptor);
    let configurations = map_configurations(device, descriptor, &mut issues);
    let mut strings = DeviceStrings::default();
    let mut raw_descriptors = Vec::new();

    match device.open() {
        Ok(handle) => {
            strings = read_strings(&handle, descriptor, timeout, &mut issues);
            read_raw_descriptors(
                &handle,
                descriptor.num_configurations(),
                timeout,
                &mut raw_descriptors,
                &mut issues,
            );
        }
        Err(error) => issues.push(issue(IssueSeverity::Warning, "open_device", error)),
    }

    let file_name = dump_file_name(descriptor.vendor_id(), descriptor.product_id(), &locator);
    let dump = DescriptorDump {
        file_name,
        records: raw_descriptors.clone(),
    };
    let descriptor_dump = match dump.encode() {
        Ok(encoded_dump) => Some(DescriptorDumpReport {
            format: "PS5CAMD1".to_owned(),
            schema_version: DUMP_SCHEMA_VERSION,
            file_name: dump.file_name.clone(),
            record_count: dump.records.len(),
            length: encoded_dump.len(),
            sha256: sha256_hex(&encoded_dump),
        }),
        Err(error) => {
            issues.push(issue(IssueSeverity::Error, "encode_descriptor_dump", error));
            None
        }
    };

    (
        UsbDeviceReport {
            mode,
            windows_pnp: None,
            locator,
            descriptor: descriptor_report,
            strings,
            configurations,
            raw_descriptors,
            descriptor_dump,
            issues,
        },
        dump,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PnpCandidate {
    mode: DeviceMode,
    report: WindowsPnpReport,
    issues: Vec<ProbeIssue>,
}

#[derive(Debug, Default)]
struct PnpScan {
    devices: Vec<PnpCandidate>,
    issues: Vec<ProbeIssue>,
}

fn supported_mode_from_pnp_id(instance_id: &str) -> Option<DeviceMode> {
    let normalized = instance_id.to_ascii_uppercase();
    let hardware_id = normalized.strip_prefix("USB\\")?.split('\\').next()?;

    let mut vendor_id = None;
    let mut product_id = None;
    for token in hardware_id.split('&') {
        if let Some(value) = token.strip_prefix("VID_") {
            let value = parse_hex_u16(value)?;
            if vendor_id.replace(value).is_some() {
                return None;
            }
        } else if let Some(value) = token.strip_prefix("PID_") {
            let value = parse_hex_u16(value)?;
            if product_id.replace(value).is_some() {
                return None;
            }
        }
    }

    DeviceMode::from_ids(vendor_id?, product_id?)
}

fn parse_hex_u16(value: &str) -> Option<u16> {
    if value.len() != 4 {
        return None;
    }
    u16::from_str_radix(value, 16).ok()
}

fn decode_multi_sz(buffer: &[u16]) -> Vec<String> {
    let mut values = Vec::new();
    let mut start = 0;
    while start < buffer.len() {
        let Some(relative_end) = buffer[start..].iter().position(|value| *value == 0) else {
            break;
        };
        let end = start + relative_end;
        if end == start {
            break;
        }
        values.push(String::from_utf16_lossy(&buffer[start..end]));
        start = end + 1;
    }
    values
}

fn merge_pnp_devices(reports: &mut Vec<UsbDeviceReport>, candidates: Vec<PnpCandidate>) {
    for candidate in candidates {
        let matching_indices = reports
            .iter()
            .enumerate()
            .filter_map(|(index, report)| {
                (report.mode == candidate.mode && report.windows_pnp.is_none()).then_some(index)
            })
            .collect::<Vec<_>>();

        match matching_indices.as_slice() {
            [index] => {
                let existing = &mut reports[*index];
                existing.windows_pnp = Some(candidate.report);
                existing.issues.extend(candidate.issues);
            }
            [] => reports.push(pnp_fallback_report(candidate)),
            _ => {
                let message = format!(
                    "PnP instance {} matches {} libusb devices in the same mode; metadata was not associated without a unique topology key",
                    candidate.report.instance_id,
                    matching_indices.len()
                );
                for index in matching_indices {
                    reports[index].issues.push(ProbeIssue {
                        severity: IssueSeverity::Warning,
                        operation: "windows_pnp_libusb_ambiguous".to_owned(),
                        message: message.clone(),
                    });
                }
            }
        }
    }
}

fn pnp_fallback_report(mut candidate: PnpCandidate) -> UsbDeviceReport {
    candidate.issues.insert(
        0,
        ProbeIssue {
            severity: IssueSeverity::Warning,
            operation: "windows_pnp_libusb_fallback".to_owned(),
            message: "device is present in Windows PnP but unavailable through libusb; descriptors and USB topology were not read"
                .to_owned(),
        },
    );
    let product_id = match candidate.mode {
        DeviceMode::Boot => OV580_BOOT_PID,
        DeviceMode::Camera => OV580_CAMERA_PID,
    };

    UsbDeviceReport {
        mode: candidate.mode,
        windows_pnp: Some(candidate.report),
        locator: DeviceLocator {
            controller_id: "windows-pnp".to_owned(),
            bus_number: 0,
            device_address: 0,
            port_number: 0,
            port_path: Vec::new(),
            speed: "unknown".to_owned(),
        },
        descriptor: DeviceDescriptorReport {
            length: 0,
            descriptor_type: 0,
            usb_version: "unknown".to_owned(),
            device_version: "unknown".to_owned(),
            class_code: 0,
            sub_class_code: 0,
            protocol_code: 0,
            max_packet_size_0: 0,
            vendor_id: OV580_VENDOR_ID,
            product_id,
            manufacturer_string_index: None,
            product_string_index: None,
            serial_number_string_index: None,
            num_configurations: 0,
        },
        strings: DeviceStrings::default(),
        configurations: Vec::new(),
        raw_descriptors: Vec::new(),
        descriptor_dump: None,
        issues: candidate.issues,
    }
}

#[cfg(not(windows))]
fn scan_windows_pnp() -> PnpScan {
    PnpScan::default()
}

#[cfg(windows)]
fn scan_windows_pnp() -> PnpScan {
    windows_pnp::scan()
}

#[cfg(windows)]
mod windows_pnp {
    use super::*;
    use std::{iter, ptr};
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Status, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
        CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_PRESENT, CM_LOCATE_DEVNODE_NORMAL, CR_BUFFER_SMALL,
        CR_SUCCESS,
    };

    const LIST_RETRIES: usize = 3;

    pub(super) fn scan() -> PnpScan {
        match present_instance_ids() {
            Ok(instance_ids) => {
                let mut scan = PnpScan::default();
                for instance_id in instance_ids {
                    let Some(mode) = supported_mode_from_pnp_id(&instance_id) else {
                        continue;
                    };
                    scan.devices.push(read_candidate(mode, instance_id));
                }
                scan
            }
            Err((operation, code)) => PnpScan {
                devices: Vec::new(),
                issues: vec![config_manager_issue(operation, code)],
            },
        }
    }

    fn present_instance_ids() -> Result<Vec<String>, (&'static str, u32)> {
        for _ in 0..LIST_RETRIES {
            let mut length = 0_u32;
            // SAFETY: `length` is a valid out pointer. A null filter with the
            // PRESENT flag requests every locally present device instance.
            let result = unsafe {
                CM_Get_Device_ID_List_SizeW(&mut length, ptr::null(), CM_GETIDLIST_FILTER_PRESENT)
            };
            if result != CR_SUCCESS {
                return Err(("cm_get_device_id_list_size", result));
            }
            if length == 0 {
                return Ok(Vec::new());
            }

            let mut buffer = vec![0_u16; length as usize];
            // SAFETY: ConfigManager receives the capacity it returned above,
            // and `buffer` remains live and exclusively borrowed for the call.
            let result = unsafe {
                CM_Get_Device_ID_ListW(
                    ptr::null(),
                    buffer.as_mut_ptr(),
                    length,
                    CM_GETIDLIST_FILTER_PRESENT,
                )
            };
            if result == CR_SUCCESS {
                return Ok(decode_multi_sz(&buffer));
            }
            if result != CR_BUFFER_SMALL {
                return Err(("cm_get_device_id_list", result));
            }
        }
        Err(("cm_get_device_id_list", CR_BUFFER_SMALL))
    }

    fn read_candidate(mode: DeviceMode, instance_id: String) -> PnpCandidate {
        let mut candidate = PnpCandidate {
            mode,
            report: WindowsPnpReport {
                instance_id: instance_id.clone(),
                status: None,
                problem_code: None,
            },
            issues: Vec::new(),
        };
        let wide = instance_id
            .encode_utf16()
            .chain(iter::once(0))
            .collect::<Vec<_>>();
        let mut devinst = 0_u32;
        // SAFETY: `wide` is NUL terminated and valid for the duration of the
        // read-only ConfigManager lookup; `devinst` is a valid out pointer.
        let locate_result =
            unsafe { CM_Locate_DevNodeW(&mut devinst, wide.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
        if locate_result != CR_SUCCESS {
            candidate
                .issues
                .push(config_manager_issue("cm_locate_devnode", locate_result));
            return candidate;
        }

        let mut status = 0_u32;
        let mut problem_code = 0_u32;
        // SAFETY: both outputs are valid u32 pointers and `devinst` was
        // resolved by ConfigManager immediately above.
        let status_result =
            unsafe { CM_Get_DevNode_Status(&mut status, &mut problem_code, devinst, 0) };
        if status_result != CR_SUCCESS {
            candidate
                .issues
                .push(config_manager_issue("cm_get_devnode_status", status_result));
            return candidate;
        }

        candidate.report.status = Some(status);
        candidate.report.problem_code = Some(problem_code);
        if problem_code != 0 {
            candidate.issues.push(ProbeIssue {
                severity: IssueSeverity::Warning,
                operation: "windows_pnp_device_problem".to_owned(),
                message: format!(
                    "Windows reports problem code {problem_code} with devnode status 0x{status:08x}"
                ),
            });
        }
        candidate
    }

    fn config_manager_issue(operation: &'static str, code: u32) -> ProbeIssue {
        ProbeIssue {
            severity: IssueSeverity::Warning,
            operation: operation.to_owned(),
            message: format!("ConfigManager returned CONFIGRET 0x{code:08x}"),
        }
    }
}

fn map_device_descriptor(descriptor: &DeviceDescriptor) -> DeviceDescriptorReport {
    DeviceDescriptorReport {
        length: descriptor.length(),
        descriptor_type: descriptor.descriptor_type(),
        usb_version: descriptor.usb_version().to_string(),
        device_version: descriptor.device_version().to_string(),
        class_code: descriptor.class_code(),
        sub_class_code: descriptor.sub_class_code(),
        protocol_code: descriptor.protocol_code(),
        max_packet_size_0: descriptor.max_packet_size(),
        vendor_id: descriptor.vendor_id(),
        product_id: descriptor.product_id(),
        manufacturer_string_index: descriptor.manufacturer_string_index(),
        product_string_index: descriptor.product_string_index(),
        serial_number_string_index: descriptor.serial_number_string_index(),
        num_configurations: descriptor.num_configurations(),
    }
}

fn map_configurations(
    device: &Device<GlobalContext>,
    descriptor: &DeviceDescriptor,
    issues: &mut Vec<ProbeIssue>,
) -> Vec<ConfigurationReport> {
    let mut reports = Vec::new();

    for index in 0..descriptor.num_configurations() {
        let config = match device.config_descriptor(index) {
            Ok(config) => config,
            Err(error) => {
                issues.push(issue(
                    IssueSeverity::Warning,
                    format!("read_configuration_{index}"),
                    error,
                ));
                continue;
            }
        };

        let interfaces = config
            .interfaces()
            .map(|interface| InterfaceReport {
                number: interface.number(),
                alternate_settings: interface
                    .descriptors()
                    .map(|alternate| AlternateSettingReport {
                        setting_number: alternate.setting_number(),
                        class_code: alternate.class_code(),
                        sub_class_code: alternate.sub_class_code(),
                        protocol_code: alternate.protocol_code(),
                        description_string_index: alternate.description_string_index(),
                        extra_hex: bytes_to_hex(alternate.extra()),
                        endpoints: alternate
                            .endpoint_descriptors()
                            .map(|endpoint| EndpointReport {
                                address: endpoint.address(),
                                number: endpoint.number(),
                                direction: enum_name(endpoint.direction()),
                                transfer_type: enum_name(endpoint.transfer_type()),
                                sync_type: enum_name(endpoint.sync_type()),
                                usage_type: enum_name(endpoint.usage_type()),
                                max_packet_size: endpoint.max_packet_size(),
                                interval: endpoint.interval(),
                                extra_hex: bytes_to_hex(endpoint.extra().unwrap_or_default()),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();

        reports.push(ConfigurationReport {
            index,
            number: config.number(),
            total_length: config.total_length(),
            num_interfaces: config.num_interfaces(),
            max_power_ma: configuration_max_power_ma(
                descriptor.usb_version().major(),
                // rusb 0.9.4 always interprets bMaxPower in 2 mA units.
                // Dividing recovers the descriptor byte so USB 3.x can use
                // its specified 8 mA units instead.
                (config.max_power() / 2) as u8,
            ),
            self_powered: config.self_powered(),
            remote_wakeup: config.remote_wakeup(),
            description_string_index: config.description_string_index(),
            extra_hex: bytes_to_hex(config.extra()),
            interfaces,
        });
    }

    reports
}

/// Converts a configuration descriptor's raw `bMaxPower` field to milliamps.
///
/// USB 1.x and 2.x encode the field in 2 mA units. USB 3.x and newer encode it
/// in 8 mA units. Keeping the raw field as `u8` also makes the maximum result
/// explicit: 510 mA before USB 3 and 2,040 mA starting with USB 3.
fn configuration_max_power_ma(usb_major: u8, b_max_power: u8) -> u16 {
    let unit_ma = if usb_major >= 3 { 8 } else { 2 };
    u16::from(b_max_power) * unit_ma
}

fn read_strings(
    handle: &DeviceHandle<GlobalContext>,
    descriptor: &DeviceDescriptor,
    timeout: Duration,
    issues: &mut Vec<ProbeIssue>,
) -> DeviceStrings {
    let language = match handle.read_languages(timeout) {
        Ok(languages) => languages.into_iter().next(),
        Err(error) => {
            issues.push(issue(IssueSeverity::Warning, "read_languages", error));
            None
        }
    };
    let Some(language) = language else {
        return DeviceStrings::default();
    };

    let manufacturer = read_optional_string(
        descriptor.manufacturer_string_index(),
        "read_manufacturer_string",
        issues,
        || handle.read_manufacturer_string(language, descriptor, timeout),
    );
    let product = read_optional_string(
        descriptor.product_string_index(),
        "read_product_string",
        issues,
        || handle.read_product_string(language, descriptor, timeout),
    );
    let serial_number = read_optional_string(
        descriptor.serial_number_string_index(),
        "read_serial_number_string",
        issues,
        || handle.read_serial_number_string(language, descriptor, timeout),
    );

    DeviceStrings {
        language_id: Some(language.lang_id()),
        manufacturer,
        product,
        serial_number,
    }
}

fn read_optional_string(
    index: Option<u8>,
    operation: &str,
    issues: &mut Vec<ProbeIssue>,
    read: impl FnOnce() -> Result<String, rusb::Error>,
) -> Option<String> {
    index?;
    match read() {
        Ok(value) => Some(value),
        Err(error) => {
            issues.push(issue(IssueSeverity::Warning, operation, error));
            None
        }
    }
}

fn read_raw_descriptors(
    handle: &DeviceHandle<GlobalContext>,
    num_configurations: u8,
    timeout: Duration,
    records: &mut Vec<RawDescriptorReport>,
    issues: &mut Vec<ProbeIssue>,
) {
    match read_descriptor(handle, DEVICE_DESCRIPTOR_TYPE, 0, 18, 18, timeout) {
        Ok(bytes) => records.push(raw_descriptor(RawDescriptorKind::Device, 0, bytes)),
        Err(error) => issues.push(issue(
            IssueSeverity::Warning,
            "read_raw_device_descriptor",
            error,
        )),
    }

    for index in 0..num_configurations {
        match read_sized_descriptor(handle, CONFIGURATION_DESCRIPTOR_TYPE, index, 9, 2, timeout) {
            Ok(bytes) => records.push(raw_descriptor(
                RawDescriptorKind::Configuration,
                index,
                bytes,
            )),
            Err(error) => issues.push(issue(
                IssueSeverity::Warning,
                format!("read_raw_configuration_{index}"),
                error,
            )),
        }
    }

    match read_sized_descriptor(handle, BOS_DESCRIPTOR_TYPE, 0, 5, 2, timeout) {
        Ok(bytes) => records.push(raw_descriptor(RawDescriptorKind::Bos, 0, bytes)),
        Err(error) => issues.push(issue(
            IssueSeverity::Warning,
            "read_raw_bos_descriptor",
            error,
        )),
    }
}

fn read_sized_descriptor(
    handle: &DeviceHandle<GlobalContext>,
    descriptor_type: u8,
    index: u8,
    header_length: usize,
    total_length_offset: usize,
    timeout: Duration,
) -> Result<Vec<u8>, rusb::Error> {
    let header = read_descriptor(
        handle,
        descriptor_type,
        index,
        header_length,
        header_length,
        timeout,
    )?;
    let total_length =
        u16::from_le_bytes([header[total_length_offset], header[total_length_offset + 1]]) as usize;
    read_descriptor(
        handle,
        descriptor_type,
        index,
        total_length,
        header_length,
        timeout,
    )
}

fn read_descriptor(
    handle: &DeviceHandle<GlobalContext>,
    descriptor_type: u8,
    index: u8,
    requested_length: usize,
    minimum_length: usize,
    timeout: Duration,
) -> Result<Vec<u8>, rusb::Error> {
    let mut buffer = vec![0_u8; requested_length];
    let request_type = rusb::request_type(Direction::In, RequestType::Standard, Recipient::Device);
    let transferred = handle.read_control(
        request_type,
        GET_DESCRIPTOR,
        u16::from(descriptor_type) << 8 | u16::from(index),
        0,
        &mut buffer,
        timeout,
    )?;
    if transferred < minimum_length {
        return Err(rusb::Error::Other);
    }
    buffer.truncate(transferred);
    Ok(buffer)
}

fn raw_descriptor(kind: RawDescriptorKind, index: u8, bytes: Vec<u8>) -> RawDescriptorReport {
    let sha256 = sha256_hex(&bytes);
    RawDescriptorReport {
        kind,
        index,
        length: bytes.len(),
        sha256,
        bytes_hex: bytes_to_hex(&bytes),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn encode_dump(records: &[RawDescriptorReport]) -> Result<Vec<u8>, DumpEncodeError> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(DUMP_MAGIC);
    encoded.extend_from_slice(&DUMP_SCHEMA_VERSION.to_le_bytes());
    encoded.extend_from_slice(&(records.len() as u16).to_le_bytes());

    for (record_index, record) in records.iter().enumerate() {
        let bytes =
            hex_to_bytes(&record.bytes_hex).ok_or(DumpEncodeError::InvalidHex { record_index })?;
        encoded.push(record.kind as u8);
        encoded.push(record.index);
        encoded.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&bytes);
    }

    Ok(encoded)
}

pub fn dump_file_name(vendor_id: u16, product_id: u16, locator: &DeviceLocator) -> String {
    let port_path = if locator.port_path.is_empty() {
        "root".to_owned()
    } else {
        locator
            .port_path
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join("-")
    };
    format!(
        "{vendor_id:04x}-{product_id:04x}-bus{:03}-port-{port_path}.ps5cam-descriptors.bin",
        locator.bus_number
    )
}

fn host_report() -> HostReport {
    let version = rusb::version();
    HostReport {
        os: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        os_version: os_version(),
        libusb_version: format!(
            "{}.{}.{}.{}{}",
            version.major(),
            version.minor(),
            version.micro(),
            version.nano(),
            version.rc().unwrap_or_default()
        ),
    }
}

#[cfg(windows)]
fn os_version() -> Option<String> {
    let output = Command::new("cmd").args(["/C", "ver"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!version.is_empty()).then_some(version)
}

#[cfg(not(windows))]
fn os_version() -> Option<String> {
    let output = Command::new("uname").arg("-sr").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!version.is_empty()).then_some(version)
}

fn issue(
    severity: IssueSeverity,
    operation: impl Into<String>,
    error: impl fmt::Display,
) -> ProbeIssue {
    ProbeIssue {
        severity,
        operation: operation.into(),
        message: error.to_string(),
    }
}

fn enum_name(value: impl fmt::Debug) -> String {
    format!("{value:?}").to_ascii_lowercase()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn hex_to_bytes(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }

    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_supported_device_states() {
        assert_eq!(
            Some(DeviceMode::Boot),
            DeviceMode::from_ids(OV580_VENDOR_ID, OV580_BOOT_PID)
        );
        assert_eq!(
            Some(DeviceMode::Camera),
            DeviceMode::from_ids(OV580_VENDOR_ID, OV580_CAMERA_PID)
        );
        assert_eq!(None, DeviceMode::from_ids(0xffff, OV580_BOOT_PID));
        assert_eq!(None, DeviceMode::from_ids(OV580_VENDOR_ID, 0xffff));
    }

    #[test]
    fn converts_usb_2_b_max_power_in_two_milliamp_units() {
        assert_eq!(configuration_max_power_ma(2, 0x32), 100);
    }

    #[test]
    fn converts_usb_3_b_max_power_in_eight_milliamp_units() {
        assert_eq!(configuration_max_power_ma(3, 0x32), 400);
    }

    #[test]
    fn max_power_conversion_covers_version_and_field_bounds() {
        assert_eq!(configuration_max_power_ma(0, 0), 0);
        assert_eq!(configuration_max_power_ma(2, u8::MAX), 510);
        assert_eq!(configuration_max_power_ma(3, u8::MAX), 2_040);
        assert_eq!(configuration_max_power_ma(u8::MAX, u8::MAX), 2_040);
    }

    #[test]
    fn parses_supported_windows_instance_and_hardware_ids_without_localized_text() {
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9&PID_0580\\5&1de99128&0&3"),
            Some(DeviceMode::Boot)
        );
        assert_eq!(
            supported_mode_from_pnp_id("usb\\vid_05a9&pid_058c&rev_0100\\camera"),
            Some(DeviceMode::Camera)
        );
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9&PID_0580&REV_0100"),
            Some(DeviceMode::Boot)
        );
        assert_eq!(
            supported_mode_from_pnp_id("PCI\\VID_05A9&PID_0580\\not-usb"),
            None
        );
        assert_eq!(supported_mode_from_pnp_id("USB\\VID_05A&PID_0580"), None);
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9&PID_FFFF\\other"),
            None
        );
    }

    #[test]
    fn pnp_id_parser_rejects_conflicting_or_non_exact_hardware_id_tokens() {
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9&PID_0580&PID_058C\\conflict"),
            None
        );
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9&VID_05A9&PID_0580\\duplicate"),
            None
        );
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9FFFF&PID_0580\\suffix"),
            None
        );
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9&PID_0580EVIL\\suffix"),
            None
        );
        assert_eq!(
            supported_mode_from_pnp_id("USB\\VID_05A9&PID_0580\\VID_FFFF&PID_058C"),
            Some(DeviceMode::Boot)
        );
    }

    #[test]
    fn decodes_windows_multi_sz_until_double_nul() {
        let mut buffer = "USB\\VID_05A9&PID_0580\\one"
            .encode_utf16()
            .chain(std::iter::once(0))
            .chain("USB\\VID_05A9&PID_058C\\câmera".encode_utf16())
            .chain([0, 0, 99])
            .collect::<Vec<_>>();
        assert_eq!(
            decode_multi_sz(&buffer),
            [
                "USB\\VID_05A9&PID_0580\\one",
                "USB\\VID_05A9&PID_058C\\câmera"
            ]
        );

        buffer.pop();
        assert_eq!(decode_multi_sz(&buffer).len(), 2);
    }

    fn pnp_candidate(mode: DeviceMode, suffix: &str, problem_code: u32) -> PnpCandidate {
        let product_id = match mode {
            DeviceMode::Boot => OV580_BOOT_PID,
            DeviceMode::Camera => OV580_CAMERA_PID,
        };
        PnpCandidate {
            mode,
            report: WindowsPnpReport {
                instance_id: format!(
                    "USB\\VID_{OV580_VENDOR_ID:04X}&PID_{product_id:04X}\\{suffix}"
                ),
                status: Some(0x0080_2400),
                problem_code: Some(problem_code),
            },
            issues: if problem_code == 0 {
                Vec::new()
            } else {
                vec![ProbeIssue {
                    severity: IssueSeverity::Warning,
                    operation: "windows_pnp_device_problem".to_owned(),
                    message: format!("problem {problem_code}"),
                }]
            },
        }
    }

    #[test]
    fn merge_correlates_pnp_metadata_with_existing_libusb_report() {
        let mut existing = pnp_fallback_report(pnp_candidate(DeviceMode::Boot, "seed", 0));
        existing.windows_pnp = None;
        existing.locator.controller_id = "libusb-bus-3".to_owned();
        existing.issues.clear();
        let mut reports = vec![existing];

        merge_pnp_devices(
            &mut reports,
            vec![pnp_candidate(DeviceMode::Boot, "real-instance", 28)],
        );

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].locator.controller_id, "libusb-bus-3");
        assert_eq!(
            reports[0].windows_pnp.as_ref().unwrap().problem_code,
            Some(28)
        );
        assert!(reports[0]
            .issues
            .iter()
            .any(|issue| issue.operation == "windows_pnp_device_problem"));
        assert!(!reports[0]
            .issues
            .iter()
            .any(|issue| issue.operation == "windows_pnp_libusb_fallback"));
    }

    #[test]
    fn merge_does_not_guess_between_multiple_libusb_devices_in_the_same_mode() {
        let mut first = pnp_fallback_report(pnp_candidate(DeviceMode::Boot, "first", 0));
        first.windows_pnp = None;
        first.issues.clear();
        first.locator.device_address = 1;
        let mut second = first.clone();
        second.locator.device_address = 2;
        let mut reports = vec![first, second];

        merge_pnp_devices(
            &mut reports,
            vec![pnp_candidate(DeviceMode::Boot, "ambiguous", 0)],
        );

        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|report| report.windows_pnp.is_none()));
        assert!(reports
            .iter()
            .all(|report| report.issues.iter().any(|issue| {
                issue.operation == "windows_pnp_libusb_ambiguous"
                    && issue.message.contains("matches 2 libusb devices")
            })));
    }

    #[test]
    fn merge_creates_read_only_fallback_and_preserves_multiple_devices() {
        let mut reports = Vec::new();
        merge_pnp_devices(
            &mut reports,
            vec![
                pnp_candidate(DeviceMode::Boot, "first", 28),
                pnp_candidate(DeviceMode::Boot, "second", 0),
                pnp_candidate(DeviceMode::Camera, "camera", 0),
            ],
        );

        assert_eq!(reports.len(), 3);
        assert_eq!(ProbeStatus::from_devices(&reports), ProbeStatus::Mixed);
        assert!(reports.iter().all(|report| report.windows_pnp.is_some()));
        assert!(reports
            .iter()
            .all(|report| report.configurations.is_empty()));
        assert!(reports
            .iter()
            .all(|report| report.strings == DeviceStrings::default()));
        assert!(reports.iter().all(|report| report
            .issues
            .iter()
            .any(|issue| issue.operation == "windows_pnp_libusb_fallback")));
        assert_eq!(
            reports
                .iter()
                .filter(|report| report.mode == DeviceMode::Boot)
                .count(),
            2
        );
    }

    #[test]
    fn encodes_a_versioned_binary_descriptor_dump() {
        let bytes = vec![18, 1, 0, 3];
        let records = vec![raw_descriptor(RawDescriptorKind::Device, 0, bytes.clone())];
        let encoded = encode_dump(&records).expect("encode valid dump");

        assert_eq!(&encoded[..8], DUMP_MAGIC);
        assert_eq!(u16::from_le_bytes([encoded[8], encoded[9]]), 1);
        assert_eq!(u16::from_le_bytes([encoded[10], encoded[11]]), 1);
        assert_eq!(encoded[12], RawDescriptorKind::Device as u8);
        assert_eq!(encoded[13], 0);
        assert_eq!(
            u32::from_le_bytes([encoded[14], encoded[15], encoded[16], encoded[17]]),
            4
        );
        assert_eq!(&encoded[18..], bytes);
    }

    #[test]
    fn creates_stable_dump_file_names() {
        let locator = DeviceLocator {
            controller_id: "libusb-bus-2".to_owned(),
            bus_number: 2,
            device_address: 5,
            port_number: 4,
            port_path: vec![4, 4, 4],
            speed: "super".to_owned(),
        };

        assert_eq!(
            dump_file_name(OV580_VENDOR_ID, OV580_BOOT_PID, &locator),
            "05a9-0580-bus002-port-4-4-4.ps5cam-descriptors.bin"
        );
    }

    #[test]
    fn rejects_invalid_hex_in_a_binary_dump() {
        let records = vec![RawDescriptorReport {
            kind: RawDescriptorKind::Device,
            index: 0,
            length: 1,
            sha256: String::new(),
            bytes_hex: "xyz".to_owned(),
        }];

        assert_eq!(
            encode_dump(&records),
            Err(DumpEncodeError::InvalidHex { record_index: 0 })
        );
    }

    #[test]
    fn fixture_matches_the_public_report_schema() {
        let fixture = include_str!("../tests/fixtures/boot-report.json");
        let report: ProbeReport = serde_json::from_str(fixture).expect("valid report fixture");

        assert_eq!(report.schema_version, REPORT_SCHEMA_VERSION);
        assert_eq!(report.status, ProbeStatus::Boot);
        assert_eq!(report.devices[0].descriptor.vendor_id, OV580_VENDOR_ID);
        assert_eq!(report.devices[0].descriptor.product_id, OV580_BOOT_PID);
        assert_eq!(report.devices[0].windows_pnp, None);
        assert!(!serde_json::to_string(&report)
            .expect("serialize report")
            .contains("windows_pnp"));
        assert_eq!(
            serde_json::from_str::<ProbeReport>(
                &serde_json::to_string(&report).expect("serialize report")
            )
            .expect("deserialize serialized report"),
            report
        );
    }
}
