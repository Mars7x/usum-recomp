//! End-to-end tests of the CLI on synthetic images.

use std::path::PathBuf;
use std::process::{Command, Output};

fn usumctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_usumctl")).args(args).output().expect("usumctl runs")
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("usumctl-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn fixture_inspects_but_is_not_a_supported_profile() {
    let path = temp("fixture.3ds");
    assert!(usumctl(&["make-fixture", path.to_str().unwrap()]).status.success());
    let out = usumctl(&["inspect", path.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("program id      0004000000FFF000"), "{stdout}");
    assert!(stdout.contains("SHA-256 all match"), "{stdout}");
    assert!(stdout.contains("unsupported"), "{stdout}");
    assert!(!out.status.success(), "a synthetic image must never pass as a supported game");
}

#[test]
fn json_output_parses() {
    let path = temp("fixture-json.3ds");
    assert!(usumctl(&["make-fixture", "--bare", path.to_str().unwrap()]).status.success());
    let out = usumctl(&["inspect", "--json", path.to_str().unwrap()]);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(value["format"], "ncch");
    assert_eq!(value["profile"]["kind"], "other_title");
}

#[test]
fn encrypted_fixture_gets_an_actionable_error() {
    let path = temp("fixture-enc.3ds");
    assert!(usumctl(&["make-fixture", "--encrypted", path.to_str().unwrap()]).status.success());
    let out = usumctl(&["inspect", path.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("encrypted"));
}

#[test]
fn missing_file_and_bad_usage_fail_cleanly() {
    let out = usumctl(&["inspect", "/nonexistent/dump.3ds"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not read"));
    assert!(!usumctl(&["inspect"]).status.success());
    assert!(!usumctl(&["inspect", "a", "b"]).status.success());
    assert!(!usumctl(&["frobnicate"]).status.success());
}
