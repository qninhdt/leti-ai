//! Regression guards for the engine/host dependency boundary.

use std::fs;
use std::path::Path;
use std::process::Command;

const FORBIDDEN_SOURCE_TOKENS: &[&str] = &[
    "openlet",
    "file-service",
    "grpc",
    "jwt",
    "rfc8693",
    "cloudfs",
    "cloud_fs",
    "workspace_id",
    "bearer",
    "principal",
    "tenant",
];

#[test]
fn normal_dependencies_do_not_point_out_of_core() {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata");
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["name"] == "leti-core")
        })
        .expect("leti-core package in cargo metadata");

    let forbidden: Vec<String> = package["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|dependency| dependency["kind"].as_str() != Some("dev"))
        .filter_map(|dependency| dependency["name"].as_str())
        .filter(|name| {
            *name == "leti-adapters"
                || *name == "leti-server"
                || name.starts_with("leti-plugin-")
                || *name == "leti-test-mock-provider"
        })
        .map(str::to_owned)
        .collect();

    assert!(
        forbidden.is_empty(),
        "leti-core production dependencies must remain port-only; forbidden dependencies: {forbidden:?}"
    );
}

#[test]
fn core_source_has_no_host_business_identifiers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan_source(&root, &mut violations);
    assert!(
        violations.is_empty(),
        "host/business identifiers leaked into leti-core source: {violations:?}"
    );
}

#[test]
fn config_has_no_host_credentials_or_tenant_fields() {
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config.rs"))
        .expect("read core config");
    let lower = source.to_ascii_lowercase();
    for forbidden in [
        "secretstring",
        "bearer",
        "workspace_id",
        "credential",
        "tenant",
    ] {
        assert!(
            !lower.contains(forbidden),
            "leti-core Config must not carry host credential/tenant field {forbidden:?}"
        );
    }
}

fn scan_source(path: &Path, violations: &mut Vec<String>) {
    let metadata = fs::metadata(path).expect("stat core source path");
    if metadata.is_dir() {
        for entry in fs::read_dir(path).expect("read core source directory") {
            scan_source(&entry.expect("read core source entry").path(), violations);
        }
        return;
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return;
    }

    let source = fs::read_to_string(path).expect("read core source file");
    let mut code = strip_comments(&source).to_ascii_lowercase();
    // The config loader intentionally names the pre-release legacy prefix so
    // operators get a fail-loud migration error. This is the sole permitted
    // old-product identifier in core source.
    if path.file_name().and_then(|name| name.to_str()) == Some("config.rs") {
        code = code.replace("openlet_", "");
    }
    for token in FORBIDDEN_SOURCE_TOKENS {
        if code.contains(token) {
            violations.push(format!("{} contains {token:?}", path.display()));
        }
    }
}

fn strip_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}
