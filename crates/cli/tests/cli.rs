use assert_cmd::prelude::*;
use std::{error::Error, fs, process::Command};
use tempfile::TempDir;

#[test]
fn init_snap_and_timeline_full_flow() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"hello\n")?;

    let init = era(work).arg("init").assert().success();
    let init_stdout = output_text(&init.get_output().stdout)?;
    assert!(init_stdout.starts_with("initialized snapshot="));
    assert!(init_stdout.contains(" files=1 "));
    let initial_snapshot = field_value(&init_stdout, "snapshot");
    assert_object_id(initial_snapshot);
    assert!(work.join(".era/HEAD").is_file());
    assert!(work.join(".era/refs/heads/main").is_file());

    let first_timeline = era(work).arg("timeline").assert().success();
    let first_timeline_stdout = output_text(&first_timeline.get_output().stdout)?;
    let first_lines = lines(&first_timeline_stdout);
    assert_eq!(first_lines.len(), 1);
    assert!(first_lines[0].starts_with(initial_snapshot));
    assert!(first_lines[0].contains("source=repository-init"));

    fs::create_dir(work.join("src"))?;
    fs::write(work.join("README.md"), b"hello again\n")?;
    fs::write(work.join("src/main.rs"), b"fn main() {}\n")?;

    let snap = era(work)
        .args([
            "snap",
            "--message",
            "feature checkpoint",
            "--author",
            "agent@example",
        ])
        .assert()
        .success();
    let snap_stdout = output_text(&snap.get_output().stdout)?;
    assert!(snap_stdout.starts_with("created snapshot="));
    assert!(snap_stdout.contains(" files=2 "));
    let second_snapshot = field_value(&snap_stdout, "snapshot");
    assert_object_id(second_snapshot);
    assert_ne!(second_snapshot, initial_snapshot);

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    let timeline_lines = lines(&timeline_stdout);
    assert_eq!(timeline_lines.len(), 2);
    assert!(timeline_lines[0].starts_with(second_snapshot));
    assert!(timeline_lines[0].contains("parents=1"));
    assert!(timeline_lines[0].contains("source=manual-snapshot"));
    assert!(timeline_lines[0].contains("message=feature checkpoint"));
    assert!(timeline_lines[1].starts_with(initial_snapshot));
    assert!(timeline_lines[1].contains("parents=0"));

    Ok(())
}

#[test]
fn init_reports_existing_repository_error() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();

    era(work).arg("init").assert().success();

    let second_init = era(work).arg("init").assert().failure();
    let stderr = output_text(&second_init.get_output().stderr)?;

    assert!(stderr.contains("error: repository is already initialized:"));
    Ok(())
}

#[test]
fn snap_and_timeline_report_missing_repository_error() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();

    let snap = era(work)
        .args(["snap", "--message", "outside repo"])
        .assert()
        .failure();
    let snap_stderr = output_text(&snap.get_output().stderr)?;
    assert!(snap_stderr.contains("error: not an Era repository:"));

    let timeline = era(work).arg("timeline").assert().failure();
    let timeline_stderr = output_text(&timeline.get_output().stderr)?;
    assert!(timeline_stderr.contains("error: not an Era repository:"));

    Ok(())
}

#[test]
fn snap_requires_message() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    era(work).arg("init").assert().success();

    let snap = era(work).arg("snap").assert().failure();
    let stderr = output_text(&snap.get_output().stderr)?;

    assert!(stderr.contains("required arguments were not provided"));
    assert!(stderr.contains("--message <MESSAGE>"));
    Ok(())
}

fn era(current_dir: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("era").expect("era binary should be built for tests");
    command.current_dir(current_dir);
    command
}

fn output_text(bytes: &[u8]) -> Result<String, std::string::FromUtf8Error> {
    String::from_utf8(bytes.to_vec())
}

fn lines(output: &str) -> Vec<&str> {
    output.lines().collect()
}

fn field_value<'a>(line: &'a str, field: &str) -> &'a str {
    let prefix = format!("{field}=");
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("missing field {field:?} in {line:?}"))
}

fn assert_object_id(value: &str) {
    assert_eq!(value.len(), 64, "object ID should be 64 hex chars");
    assert!(
        value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "object ID should be hex: {value}"
    );
}
