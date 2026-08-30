use std::{env, fs, io::Write, path::PathBuf};

const MAGIC: &[u8; 8] = b"PS5PKG1\0";

fn release_version(manifest: &str) -> String {
    let marker = "\"release_version\": \"";
    let start = manifest
        .find(marker)
        .expect("release manifest has release_version")
        + marker.len();
    let end = manifest[start..].find('"').expect("release version closes");
    manifest[start..start + end].to_owned()
}

fn main() {
    println!("cargo:rerun-if-env-changed=PS5CAM_SETUP_PAYLOAD_DIR");
    println!("cargo:rustc-check-cfg=cfg(ps5cam_setup_without_payload)");
    let destination = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ps5cam-setup.payload");
    let Some(source) = env::var_os("PS5CAM_SETUP_PAYLOAD_DIR").map(PathBuf::from) else {
        fs::write(&destination, []).expect("create empty verification payload");
        println!("cargo:rustc-cfg=ps5cam_setup_without_payload");
        println!(
            "cargo:rustc-env=PS5CAM_SETUP_RELEASE_VERSION={}",
            env::var("CARGO_PKG_VERSION").expect("package version")
        );
        return;
    };
    let manifest_path = source.join("release-manifest.json");
    let manifest = fs::read_to_string(&manifest_path)
        .expect("PS5CAM setup payload release-manifest.json is required");
    let mut files: Vec<PathBuf> = fs::read_dir(&source)
        .expect("PS5CAM setup payload directory is required")
        .map(|entry| entry.expect("read payload entry").path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    assert!(!files.is_empty(), "setup payload cannot be empty");

    let mut output = fs::File::create(destination).expect("create embedded setup payload");
    output.write_all(MAGIC).unwrap();
    output
        .write_all(&(files.len() as u16).to_le_bytes())
        .unwrap();
    for path in files {
        let name = path
            .file_name()
            .unwrap()
            .to_str()
            .expect("payload file name must be UTF-8");
        assert!(
            !name.contains(['\\', '/', ':']),
            "payload file name must be flat"
        );
        let bytes = fs::read(&path).expect("read payload file");
        assert!(
            name.len() <= u16::MAX as usize,
            "payload file name too long"
        );
        output
            .write_all(&(name.len() as u16).to_le_bytes())
            .unwrap();
        output.write_all(name.as_bytes()).unwrap();
        output
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .unwrap();
        output.write_all(&bytes).unwrap();
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!(
        "cargo:rustc-env=PS5CAM_SETUP_RELEASE_VERSION={}",
        release_version(&manifest)
    );
}
