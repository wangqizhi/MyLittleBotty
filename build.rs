use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let cargo_toml = PathBuf::from(manifest_dir).join("Cargo.toml");
    let content = fs::read_to_string(cargo_toml).expect("failed to read Cargo.toml");

    let download_url = find_metadata_value(&content, "download_url")
        .expect("missing [package.metadata.botty] download_url in Cargo.toml");
    let latest_release_api_url = find_metadata_value(&content, "latest_release_api_url")
        .expect("missing [package.metadata.botty] latest_release_api_url in Cargo.toml");
    let install_script_url = find_metadata_value(&content, "install_script_url")
        .expect("missing [package.metadata.botty] install_script_url in Cargo.toml");
    println!("cargo:rustc-env=BOTTY_DOWNLOAD_URL={download_url}");
    println!("cargo:rustc-env=BOTTY_LATEST_RELEASE_API_URL={latest_release_api_url}");
    println!("cargo:rustc-env=BOTTY_INSTALL_SCRIPT_URL={install_script_url}");
}

fn find_metadata_value(content: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line == "[package.metadata.botty]";
            continue;
        }
        if !in_section || !line.starts_with(key) {
            continue;
        }
        let (_, value) = line.split_once('=')?;
        return Some(value.trim().trim_matches('"').to_string());
    }
    None
}
