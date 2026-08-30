//! Deterministic, clean-room structural analysis of caller-supplied firmware images.
//!
//! This crate never bundles firmware and deliberately does not claim an instruction
//! set or entry point. Those conclusions require independent evidence from real,
//! lawfully obtained samples and hardware observations.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AnalyzerError {
    #[error("invalid analyzer configuration: {message}")]
    InvalidConfiguration { message: String },
    #[error("could not {operation} '{path}': {message}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },
}

impl AnalyzerError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration { .. } => "invalid_configuration",
            Self::Io {
                operation: "read", ..
            } => "input_read_failed",
            Self::Io { .. } => "io_failed",
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Io { path, .. } => Some(path),
            Self::InvalidConfiguration { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub entropy_window_size: usize,
    pub entropy_stride: usize,
    pub region_block_size: usize,
    pub minimum_string_length: usize,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            entropy_window_size: 256,
            entropy_stride: 256,
            region_block_size: 256,
            minimum_string_length: 4,
        }
    }
}

impl AnalysisConfig {
    pub fn validate(self) -> Result<Self, AnalyzerError> {
        if self.entropy_window_size == 0 {
            return Err(invalid_config(
                "entropy_window_size must be greater than zero",
            ));
        }
        if self.entropy_stride == 0 {
            return Err(invalid_config("entropy_stride must be greater than zero"));
        }
        if self.region_block_size == 0 {
            return Err(invalid_config(
                "region_block_size must be greater than zero",
            ));
        }
        if self.minimum_string_length < 2 {
            return Err(invalid_config("minimum_string_length must be at least two"));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirmwareReport {
    pub schema_version: u32,
    pub source: SourceIdentity,
    pub configuration: AnalysisConfig,
    pub entropy_windows: Vec<EntropyWindow>,
    pub strings: Vec<ExtractedString>,
    pub usb_descriptor_candidates: Vec<UsbDescriptorCandidate>,
    pub regions: Vec<Region>,
    pub architecture: ArchitectureAssessment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntropyWindow {
    pub offset: u64,
    pub length: u64,
    pub shannon_bits_per_byte: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StringEncoding {
    Ascii,
    Utf16Le,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedString {
    pub offset: u64,
    pub byte_length: u64,
    pub encoding: StringEncoding,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbDescriptorCandidate {
    pub offset: u64,
    pub declared_length: u8,
    pub descriptor_type: u8,
    pub descriptor_name: String,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    ZeroFill,
    RepeatedByte,
    Sparse,
    TextLike,
    LowEntropy,
    HighEntropy,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub offset: u64,
    pub length: u64,
    pub kind: RegionKind,
    pub shannon_bits_per_byte: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchitectureAssessment {
    pub instruction_set: EvidenceStatus,
    pub entry_point: EvidenceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceStatus {
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffConfig {
    pub block_size: usize,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self { block_size: 256 }
    }
}

impl DiffConfig {
    pub fn validate(self) -> Result<Self, AnalyzerError> {
        if self.block_size == 0 {
            return Err(invalid_config("block_size must be greater than zero"));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralDiffReport {
    pub schema_version: u32,
    pub left: SourceIdentity,
    pub right: SourceIdentity,
    pub configuration: DiffConfig,
    pub summary: DiffSummary,
    pub changed_blocks: Vec<ChangedBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub identical: bool,
    pub shared_bytes: u64,
    pub modified_bytes: u64,
    pub added_bytes: u64,
    pub removed_bytes: u64,
    pub first_difference: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Modified,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedBlock {
    pub offset: u64,
    pub left_length: u64,
    pub right_length: u64,
    pub kind: ChangeKind,
    pub modified_bytes: u64,
    pub left_sha256: Option<String>,
    pub right_sha256: Option<String>,
}

pub fn analyze_file(
    path: impl AsRef<Path>,
    config: AnalysisConfig,
) -> Result<FirmwareReport, AnalyzerError> {
    let path = path.as_ref();
    let bytes = read_input(path)?;
    analyze_bytes(path.to_string_lossy(), &bytes, config)
}

pub fn analyze_bytes(
    source_name: impl Into<String>,
    bytes: &[u8],
    config: AnalysisConfig,
) -> Result<FirmwareReport, AnalyzerError> {
    let config = config.validate()?;
    let source = source_identity(source_name.into(), bytes);
    let entropy_windows = entropy_windows(bytes, config.entropy_window_size, config.entropy_stride);
    let mut strings = extract_ascii_strings(bytes, config.minimum_string_length);
    strings.extend(extract_utf16le_strings(bytes, config.minimum_string_length));
    strings.sort_by(|a, b| {
        a.offset
            .cmp(&b.offset)
            .then_with(|| encoding_rank(&a.encoding).cmp(&encoding_rank(&b.encoding)))
            .then_with(|| a.byte_length.cmp(&b.byte_length))
    });

    Ok(FirmwareReport {
        schema_version: SCHEMA_VERSION,
        source,
        configuration: config,
        entropy_windows,
        strings,
        usb_descriptor_candidates: find_usb_descriptor_candidates(bytes),
        regions: classify_regions(bytes, config.region_block_size),
        architecture: ArchitectureAssessment {
            instruction_set: EvidenceStatus {
                status: "indeterminate".into(),
                reason: "no ISA is asserted by structural analysis alone; independent opcode and hardware evidence is required".into(),
            },
            entry_point: EvidenceStatus {
                status: "indeterminate".into(),
                reason: "no entry point is asserted without a validated image format, ISA, and runtime trace".into(),
            },
        },
    })
}

pub fn diff_files(
    left: impl AsRef<Path>,
    right: impl AsRef<Path>,
    config: DiffConfig,
) -> Result<StructuralDiffReport, AnalyzerError> {
    let left = left.as_ref();
    let right = right.as_ref();
    let left_bytes = read_input(left)?;
    let right_bytes = read_input(right)?;
    diff_bytes(
        left.to_string_lossy(),
        &left_bytes,
        right.to_string_lossy(),
        &right_bytes,
        config,
    )
}

pub fn diff_bytes(
    left_name: impl Into<String>,
    left: &[u8],
    right_name: impl Into<String>,
    right: &[u8],
    config: DiffConfig,
) -> Result<StructuralDiffReport, AnalyzerError> {
    let config = config.validate()?;
    let shared = left.len().min(right.len());
    let modified_bytes = left[..shared]
        .iter()
        .zip(&right[..shared])
        .filter(|(a, b)| a != b)
        .count();
    let added_bytes = right.len().saturating_sub(left.len());
    let removed_bytes = left.len().saturating_sub(right.len());
    let first_difference = (0..shared)
        .find(|&index| left[index] != right[index])
        .or_else(|| (left.len() != right.len()).then_some(shared));

    let total = left.len().max(right.len());
    let mut changed_blocks = Vec::new();
    let mut offset = 0usize;
    while offset < total {
        let left_end = left.len().min(offset.saturating_add(config.block_size));
        let right_end = right.len().min(offset.saturating_add(config.block_size));
        let left_block = if offset < left.len() {
            &left[offset..left_end]
        } else {
            &[]
        };
        let right_block = if offset < right.len() {
            &right[offset..right_end]
        } else {
            &[]
        };
        if left_block != right_block {
            let shared_block = left_block.len().min(right_block.len());
            let block_modified = left_block[..shared_block]
                .iter()
                .zip(&right_block[..shared_block])
                .filter(|(a, b)| a != b)
                .count();
            let kind = if left_block.is_empty() {
                ChangeKind::Added
            } else if right_block.is_empty() {
                ChangeKind::Removed
            } else {
                ChangeKind::Modified
            };
            changed_blocks.push(ChangedBlock {
                offset: offset as u64,
                left_length: left_block.len() as u64,
                right_length: right_block.len() as u64,
                kind,
                modified_bytes: block_modified as u64,
                left_sha256: (!left_block.is_empty()).then(|| sha256(left_block)),
                right_sha256: (!right_block.is_empty()).then(|| sha256(right_block)),
            });
        }
        offset = offset.saturating_add(config.block_size);
    }

    Ok(StructuralDiffReport {
        schema_version: SCHEMA_VERSION,
        left: source_identity(left_name.into(), left),
        right: source_identity(right_name.into(), right),
        configuration: config,
        summary: DiffSummary {
            identical: first_difference.is_none(),
            shared_bytes: shared as u64,
            modified_bytes: modified_bytes as u64,
            added_bytes: added_bytes as u64,
            removed_bytes: removed_bytes as u64,
            first_difference: first_difference.map(|value| value as u64),
        },
        changed_blocks,
    })
}

fn invalid_config(message: impl Into<String>) -> AnalyzerError {
    AnalyzerError::InvalidConfiguration {
        message: message.into(),
    }
}

fn read_input(path: &Path) -> Result<Vec<u8>, AnalyzerError> {
    fs::read(path).map_err(|error| AnalyzerError::Io {
        operation: "read",
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn source_identity(name: String, bytes: &[u8]) -> SourceIdentity {
    SourceIdentity {
        name,
        size: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn entropy_windows(bytes: &[u8], window_size: usize, stride: usize) -> Vec<EntropyWindow> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = bytes.len().min(offset.saturating_add(window_size));
        result.push(EntropyWindow {
            offset: offset as u64,
            length: (end - offset) as u64,
            shannon_bits_per_byte: rounded_entropy(&bytes[offset..end]),
        });
        offset = offset.saturating_add(stride);
    }
    result
}

fn rounded_entropy(bytes: &[u8]) -> f64 {
    let entropy = shannon_entropy(bytes);
    (entropy * 1_000_000.0).round() / 1_000_000.0
}

fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &byte in bytes {
        counts[byte as usize] += 1;
    }
    let length = bytes.len() as f64;
    counts
        .into_iter()
        .filter(|&count| count != 0)
        .map(|count| {
            let probability = count as f64 / length;
            -probability * probability.log2()
        })
        .sum()
}

fn extract_ascii_strings(bytes: &[u8], minimum: usize) -> Vec<ExtractedString> {
    let mut strings = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if !is_ascii_text(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_ascii_text(bytes[index]) {
            index += 1;
        }
        if index - start >= minimum {
            strings.push(ExtractedString {
                offset: start as u64,
                byte_length: (index - start) as u64,
                encoding: StringEncoding::Ascii,
                value: String::from_utf8_lossy(&bytes[start..index]).into_owned(),
            });
        }
    }
    strings
}

fn extract_utf16le_strings(bytes: &[u8], minimum: usize) -> Vec<ExtractedString> {
    let mut candidates = Vec::new();
    for alignment in 0..2usize {
        let mut index = alignment;
        while index + 1 < bytes.len() {
            let start = index;
            let mut units = Vec::new();
            while index + 1 < bytes.len() {
                let unit = u16::from_le_bytes([bytes[index], bytes[index + 1]]);
                // Restrict generic firmware scanning to the Latin-1 subset. This
                // keeps USB descriptor headers and machine code from being folded
                // into otherwise valid UTF-16LE strings.
                if bytes[index + 1] != 0 || !is_utf16_text_unit(unit) {
                    break;
                }
                units.push(unit);
                index += 2;
            }
            if units.len() >= minimum {
                let value: String = char::decode_utf16(units.iter().copied())
                    .filter_map(Result::ok)
                    .collect();
                if value.chars().count() >= minimum {
                    candidates.push(ExtractedString {
                        offset: start as u64,
                        byte_length: (index - start) as u64,
                        encoding: StringEncoding::Utf16Le,
                        value,
                    });
                }
            }
            if index == start {
                index += 2;
            }
        }
    }
    candidates
}

fn is_ascii_text(byte: u8) -> bool {
    matches!(byte, 0x20..=0x7e | b'\t')
}

fn is_utf16_text_unit(unit: u16) -> bool {
    if unit == 0 || (unit < 0x20 && unit != b'\t' as u16) {
        return false;
    }
    if (0xd800..=0xdfff).contains(&unit) {
        return false;
    }
    char::from_u32(unit as u32).is_some_and(|character| !character.is_control())
}

fn encoding_rank(encoding: &StringEncoding) -> u8 {
    match encoding {
        StringEncoding::Ascii => 0,
        StringEncoding::Utf16Le => 1,
    }
}

fn find_usb_descriptor_candidates(bytes: &[u8]) -> Vec<UsbDescriptorCandidate> {
    let mut candidates = Vec::new();
    for offset in 0..bytes.len().saturating_sub(1) {
        let length = bytes[offset] as usize;
        let descriptor_type = bytes[offset + 1];
        if length < 2 || offset.saturating_add(length) > bytes.len() {
            continue;
        }
        let descriptor = &bytes[offset..offset + length];
        if let Some(candidate) = validate_usb_descriptor(bytes, offset, descriptor_type, descriptor)
        {
            candidates.push(candidate);
        }
    }
    candidates
}

fn validate_usb_descriptor(
    image: &[u8],
    offset: usize,
    descriptor_type: u8,
    descriptor: &[u8],
) -> Option<UsbDescriptorCandidate> {
    let length = descriptor.len();
    let (name, confidence, mut evidence) = match descriptor_type {
        0x01 if length == 18 => {
            let bcd_usb = u16::from_le_bytes([descriptor[2], descriptor[3]]);
            let max_packet_size_0 = descriptor[7];
            let vendor_id = u16::from_le_bytes([descriptor[8], descriptor[9]]);
            let product_id = u16::from_le_bytes([descriptor[10], descriptor[11]]);
            let configurations = descriptor[17];
            if !is_usb_bcd(bcd_usb)
                || !matches!(max_packet_size_0, 8 | 16 | 32 | 64 | 9)
                || vendor_id == 0
                || product_id == 0
                || configurations == 0
                || configurations > 8
            {
                return None;
            }
            (
                "device",
                Confidence::High,
                vec![
                    "device descriptor has the required 18-byte length".into(),
                    "USB version, EP0 size, VID/PID and configuration count are plausible".into(),
                ],
            )
        }
        0x02 if length == 9 => {
            let total = u16::from_le_bytes([descriptor[2], descriptor[3]]) as usize;
            let interfaces = descriptor[4];
            let configuration_value = descriptor[5];
            let attributes = descriptor[7];
            if !(9..=4096).contains(&total)
                || !descriptor_chain_is_well_formed(image, offset, total)
                || interfaces == 0
                || interfaces > 32
                || configuration_value == 0
                || attributes & 0x80 == 0
            {
                return None;
            }
            (
                "configuration",
                Confidence::High,
                vec![format!("wTotalLength is {total} bytes")],
            )
        }
        0x03 if is_plausible_usb_string_descriptor(descriptor) => (
            "string",
            Confidence::Medium,
            vec!["descriptor length is valid for UTF-16LE code units".into()],
        ),
        0x04 if length == 9 && descriptor[4] <= 32 => (
            "interface",
            Confidence::High,
            vec!["interface descriptor meets the 9-byte minimum".into()],
        ),
        0x05 if length == 7
            && descriptor[2] & 0x70 == 0
            && u16::from_le_bytes([descriptor[4], descriptor[5]]) != 0 =>
        {
            (
                "endpoint",
                Confidence::High,
                vec!["endpoint descriptor meets the 7-byte minimum".into()],
            )
        }
        0x0b if length == 8 && descriptor[3] != 0 => (
            "interface_association",
            Confidence::High,
            vec!["interface association descriptor meets the 8-byte minimum".into()],
        ),
        0x0f if length == 5 => {
            let total = u16::from_le_bytes([descriptor[2], descriptor[3]]) as usize;
            if !(5..=1024).contains(&total)
                || descriptor[4] == 0
                || !descriptor_chain_is_well_formed(image, offset, total)
            {
                return None;
            }
            (
                "bos",
                Confidence::High,
                vec!["BOS length and capability count are plausible".into()],
            )
        }
        _ => return None,
    };
    evidence.insert(0, "bLength fits entirely within the image".into());
    Some(UsbDescriptorCandidate {
        offset: offset as u64,
        declared_length: length as u8,
        descriptor_type,
        descriptor_name: name.into(),
        confidence,
        evidence,
    })
}

fn is_usb_bcd(value: u16) -> bool {
    let digits_are_bcd = (0..4).all(|shift| ((value >> (shift * 4)) & 0x0f) <= 9);
    let major = (value >> 8) as u8;
    digits_are_bcd && matches!(major, 1..=3)
}

fn is_plausible_usb_string_descriptor(descriptor: &[u8]) -> bool {
    if descriptor.len() < 4 || descriptor.len() % 2 != 0 {
        return false;
    }
    if descriptor.len() == 4 {
        return u16::from_le_bytes([descriptor[2], descriptor[3]]) != 0;
    }
    let units = descriptor[2..].chunks_exact(2).collect::<Vec<_>>();
    let zero_high_bytes = units.iter().filter(|pair| pair[1] == 0).count();
    zero_high_bytes.saturating_mul(4) >= units.len().saturating_mul(3)
        && units
            .iter()
            .all(|pair| is_utf16_text_unit(u16::from_le_bytes([pair[0], pair[1]])))
}

fn descriptor_chain_is_well_formed(image: &[u8], offset: usize, total: usize) -> bool {
    let Some(end) = offset.checked_add(total) else {
        return false;
    };
    if end > image.len() {
        return false;
    }
    let mut cursor = offset;
    while cursor < end {
        if cursor + 2 > end {
            return false;
        }
        let length = image[cursor] as usize;
        if length < 2 || cursor.saturating_add(length) > end {
            return false;
        }
        cursor += length;
    }
    cursor == end
}

fn classify_regions(bytes: &[u8], block_size: usize) -> Vec<Region> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut raw = Vec::new();
    for (index, block) in bytes.chunks(block_size).enumerate() {
        raw.push((index * block_size, block.len(), classify_block(block)));
    }

    let mut merged: Vec<(usize, usize, RegionKind)> = Vec::new();
    for (offset, length, kind) in raw {
        if let Some((_, previous_length, previous_kind)) = merged.last_mut() {
            if *previous_kind == kind {
                *previous_length += length;
                continue;
            }
        }
        merged.push((offset, length, kind));
    }

    merged
        .into_iter()
        .map(|(offset, length, kind)| Region {
            offset: offset as u64,
            length: length as u64,
            kind,
            shannon_bits_per_byte: rounded_entropy(&bytes[offset..offset + length]),
        })
        .collect()
}

fn classify_block(bytes: &[u8]) -> RegionKind {
    if bytes.iter().all(|&byte| byte == 0) {
        return RegionKind::ZeroFill;
    }
    if bytes.windows(2).all(|window| window[0] == window[1]) {
        return RegionKind::RepeatedByte;
    }
    let nonzero_ratio = bytes.iter().filter(|&&byte| byte != 0).count() as f64 / bytes.len() as f64;
    if nonzero_ratio <= 0.10 {
        return RegionKind::Sparse;
    }
    let text_ratio =
        bytes.iter().filter(|&&byte| is_ascii_text(byte)).count() as f64 / bytes.len() as f64;
    if text_ratio >= 0.75 {
        return RegionKind::TextLike;
    }
    match shannon_entropy(bytes) {
        entropy if entropy <= 2.0 => RegionKind::LowEntropy,
        entropy if entropy >= 7.25 => RegionKind::HighEntropy,
        _ => RegionKind::Mixed,
    }
}

/// Counts byte values without exposing image contents. Useful for reproducible
/// research assertions while keeping caller-provided blobs outside reports.
pub fn byte_histogram(bytes: &[u8]) -> BTreeMap<u8, u64> {
    let mut histogram = BTreeMap::new();
    for &byte in bytes {
        *histogram.entry(byte).or_insert(0) += 1;
    }
    histogram
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_image_is_well_defined() {
        let report = analyze_bytes("empty", &[], AnalysisConfig::default()).unwrap();
        assert_eq!(report.source.size, 0);
        assert!(report.entropy_windows.is_empty());
        assert!(report.regions.is_empty());
        assert_eq!(report.architecture.instruction_set.status, "indeterminate");
    }

    #[test]
    fn entropy_is_rounded_and_deterministic() {
        assert_eq!(rounded_entropy(&[0; 16]), 0.0);
        assert_eq!(rounded_entropy(&[0, 1, 0, 1]), 1.0);
    }

    #[test]
    fn invalid_configuration_is_structured() {
        let error = AnalysisConfig {
            entropy_window_size: 0,
            ..AnalysisConfig::default()
        }
        .validate()
        .unwrap_err();
        assert_eq!(error.code(), "invalid_configuration");
        assert!(error.path().is_none());
    }

    #[test]
    fn utf16_extraction_rejects_machine_code_with_one_zero_high_byte() {
        let code_like = [0x00, 0x99, 0x20, 0x99, 0x40, 0x99, b'A', 0x00];
        assert!(extract_utf16le_strings(&code_like, 4).is_empty());

        let text = [b'C', 0, b'A', 0, b'M', 0, b'E', 0, b'R', 0, b'A', 0];
        let strings = extract_utf16le_strings(&text, 4);
        assert_eq!(strings.len(), 1);
        assert_eq!(strings[0].value, "CAMERA");
    }

    #[test]
    fn usb_candidates_require_exact_lengths_and_well_formed_chains() {
        let mut bytes = vec![56, 4];
        bytes.extend([0; 54]);
        bytes.extend([9, 2, 18, 0, 1, 1, 0, 0x80, 50]);
        bytes.extend([0; 9]);
        assert!(find_usb_descriptor_candidates(&bytes).is_empty());

        let valid = [
            18, 1, 0x20, 0x03, 0xef, 2, 1, 9, 0xa9, 5, 0x8a, 5, 0, 1, 1, 2, 0, 1, 9, 2, 18, 0, 1,
            1, 0, 0x80, 50, 9, 4, 0, 0, 0, 0x0e, 1, 0, 0,
        ];
        let candidates = find_usb_descriptor_candidates(&valid);
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.descriptor_name == "device")
                .count(),
            1
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.descriptor_name == "configuration")
                .count(),
            1
        );
    }
}
