use ov580_fw_analyzer::{
    analyze_bytes, diff_bytes, AnalysisConfig, ChangeKind, DiffConfig, FirmwareReport, RegionKind,
    StringEncoding,
};
use std::path::PathBuf;
use std::process::Command;

fn decode_fixture(text: &str) -> Vec<u8> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(str::split_whitespace)
        .map(|value| u8::from_str_radix(value, 16).expect("fixture contains valid hex"))
        .collect()
}

fn fixture_a() -> Vec<u8> {
    decode_fixture(include_str!("fixtures/synthetic_a.hex"))
}

fn fixture_b() -> Vec<u8> {
    decode_fixture(include_str!("fixtures/synthetic_b.hex"))
}

#[test]
fn extracts_strings_and_usb_candidates_from_synthetic_fixture() {
    let bytes = fixture_a();
    let report = analyze_bytes(
        "synthetic-a",
        &bytes,
        AnalysisConfig {
            entropy_window_size: 16,
            entropy_stride: 16,
            region_block_size: 8,
            minimum_string_length: 4,
        },
    )
    .unwrap();

    assert_eq!(report.source.size, bytes.len() as u64);
    assert_eq!(report.source.sha256.len(), 64);
    assert!(report.strings.iter().any(|candidate| {
        candidate.encoding == StringEncoding::Ascii && candidate.value == "OV580 TEST"
    }));
    assert!(report.strings.iter().any(|candidate| {
        candidate.encoding == StringEncoding::Utf16Le && candidate.value == "CAMERA"
    }));
    let device = report
        .usb_descriptor_candidates
        .iter()
        .find(|candidate| candidate.descriptor_name == "device")
        .expect("synthetic USB device descriptor");
    assert_eq!(device.declared_length, 18);
    assert!(report
        .regions
        .iter()
        .any(|region| region.kind == RegionKind::ZeroFill));
    assert!(report
        .regions
        .iter()
        .any(|region| region.kind == RegionKind::RepeatedByte));
}

#[test]
fn structural_diff_reports_modified_and_added_data_without_embedding_bytes() {
    let left = fixture_a();
    let right = fixture_b();
    let report = diff_bytes(
        "synthetic-a",
        &left,
        "synthetic-b",
        &right,
        DiffConfig { block_size: 16 },
    )
    .unwrap();

    assert!(!report.summary.identical);
    assert_eq!(report.summary.modified_bytes, 2);
    assert_eq!(report.summary.added_bytes, 4);
    assert_eq!(report.summary.removed_bytes, 0);
    assert_eq!(report.summary.first_difference, Some(17));
    assert!(report
        .changed_blocks
        .iter()
        .any(|block| block.kind == ChangeKind::Modified));
    assert!(report.changed_blocks.iter().any(|block| {
        block.right_length > block.left_length || block.kind == ChangeKind::Added
    }));

    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("deadbeef"));
    assert_eq!(json, serde_json::to_string(&report).unwrap());
}

#[test]
fn identical_diff_has_no_changed_blocks() {
    let bytes = fixture_a();
    let report = diff_bytes(
        "same-left",
        &bytes,
        "same-right",
        &bytes,
        DiffConfig { block_size: 7 },
    )
    .unwrap();
    assert!(report.summary.identical);
    assert_eq!(report.summary.first_difference, None);
    assert!(report.changed_blocks.is_empty());
}

#[test]
fn cli_emits_machine_readable_report() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("synthetic_a.hex");
    let output = Command::new(env!("CARGO_BIN_EXE_ov580-fw-analyzer"))
        .arg("--compact")
        .arg("analyze")
        .arg(fixture)
        .output()
        .expect("analyzer CLI runs");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: FirmwareReport = serde_json::from_slice(&output.stdout).expect("valid report JSON");
    assert_eq!(report.schema_version, 1);
    assert_eq!(
        report.source.size,
        include_bytes!("fixtures/synthetic_a.hex").len() as u64
    );
}

#[test]
fn cli_emits_structured_error_for_missing_input() {
    let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("does-not-exist.bin");
    let output = Command::new(env!("CARGO_BIN_EXE_ov580-fw-analyzer"))
        .arg("analyze")
        .arg(&missing)
        .output()
        .expect("analyzer CLI runs");

    assert_eq!(output.status.code(), Some(2));
    let document: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("valid structured error JSON");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["error"]["code"], "input_read_failed");
    assert_eq!(
        document["error"]["path"],
        missing.to_string_lossy().as_ref()
    );
}
