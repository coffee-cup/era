use assert_cmd::prelude::*;
use std::{error::Error, fs, process::Command};
use tempfile::TempDir;

#[test]
fn init_snap_and_timeline_full_flow_uses_clean_default_output() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"hello\n")?;

    let init = era(work).arg("init").assert().success();
    let init_stdout = output_text(&init.get_output().stdout)?;
    assert_no_ansi(&init_stdout);
    assert!(init_stdout.starts_with("✓ Initialized Era repository\n"));
    assert!(init_stdout.contains("Snapshot"));
    assert!(init_stdout.contains("Captured   1 file, 1 directory, 6 B"));
    assert!(!init_stdout.contains("Full snapshot"));
    assert!(work.join(".era/HEAD").is_file());
    assert!(work.join(".era/refs/heads/main").is_file());
    let initial_snapshot = field_line_value(&init_stdout, "Snapshot");
    assert_short_object_id(initial_snapshot);

    let status = era(work).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_no_ansi(&status_stdout);
    assert!(status_stdout.starts_with("✓ Repository status\n"));
    assert_eq!(field_line_value(&status_stdout, "Branch"), "main");
    assert_eq!(
        field_line_value(&status_stdout, "Snapshot"),
        initial_snapshot
    );
    assert_eq!(field_line_value(&status_stdout, "Timeline"), "1 snapshot");
    assert!(field_line_value(&status_stdout, "Working").contains("not compared yet"));

    let first_timeline = era(work).arg("timeline").assert().success();
    let first_timeline_stdout = output_text(&first_timeline.get_output().stdout)?;
    assert_no_ansi(&first_timeline_stdout);
    let first_lines = lines(&first_timeline_stdout);
    assert_eq!(first_lines.len(), 2);
    assert_eq!(first_lines[0], "Timeline for main");
    assert!(first_lines[1].starts_with(&format!("● {initial_snapshot}  ")));
    assert!(first_lines[1].contains("repository init"));

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
    assert_no_ansi(&snap_stdout);
    assert!(snap_stdout.starts_with("✓ Created snapshot\n"));
    assert!(snap_stdout.contains("Message    feature checkpoint"));
    assert!(snap_stdout.contains("Captured   2 files, 2 directories"));
    let second_snapshot = field_line_value(&snap_stdout, "Snapshot");
    assert_short_object_id(second_snapshot);
    assert_ne!(second_snapshot, initial_snapshot);

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    assert_no_ansi(&timeline_stdout);
    let timeline_lines = lines(&timeline_stdout);
    assert_eq!(timeline_lines.len(), 3);
    assert_eq!(timeline_lines[0], "Timeline for main");
    assert!(timeline_lines[1].starts_with(&format!("● {second_snapshot}  ")));
    assert!(timeline_lines[1].contains("feature checkpoint"));
    assert!(timeline_lines[2].starts_with(&format!("● {initial_snapshot}  ")));
    assert!(timeline_lines[2].contains("repository init"));

    Ok(())
}

#[test]
fn verbose_output_includes_full_snapshot_details() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"hello\n")?;

    let init = era(work).args(["--verbose", "init"]).assert().success();
    let init_stdout = output_text(&init.get_output().stdout)?;
    assert!(init_stdout.contains("Details\n"));
    assert_object_id(field_line_value(&init_stdout, "Full snapshot"));
    assert_object_id(field_line_value(&init_stdout, "Root tree"));
    assert_eq!(field_line_value(&init_stdout, "Branch"), "main");
    assert_eq!(field_line_value(&init_stdout, "Parents"), "0");
    assert_eq!(field_line_value(&init_stdout, "Source"), "repository-init");
    assert_eq!(field_line_value(&init_stdout, "Files"), "1");
    assert_eq!(field_line_value(&init_stdout, "Blobs stored"), "1");

    fs::write(work.join("README.md"), b"hello again\n")?;
    let snap = era(work)
        .args([
            "snap",
            "--message",
            "verbose checkpoint",
            "--author",
            "agent@example",
            "--verbose",
        ])
        .assert()
        .success();
    let snap_stdout = output_text(&snap.get_output().stdout)?;
    assert_eq!(
        field_line_value(&snap_stdout, "Message"),
        "verbose checkpoint"
    );
    assert_eq!(field_line_value(&snap_stdout, "Author"), "agent@example");
    assert_eq!(field_line_value(&snap_stdout, "Parents"), "1");
    assert_eq!(field_line_value(&snap_stdout, "Source"), "manual-snapshot");

    let status = era(work).args(["status", "--verbose"]).assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_eq!(field_line_value(&status_stdout, "Branch"), "main");
    assert_object_id(field_line_value(&status_stdout, "Full snapshot"));
    assert_object_id(field_line_value(&status_stdout, "Root tree"));
    assert!(status_stdout.contains("Metadata"));
    assert!(status_stdout.contains("Objects"));
    assert!(status_stdout.contains("Branch ref"));

    let timeline = era(work).args(["timeline", "--verbose"]).assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    assert!(timeline_stdout.contains("Timeline for main\n"));
    assert!(timeline_stdout.contains("Full snapshot"));
    assert!(timeline_stdout.contains("Root tree"));
    assert_eq!(
        field_line_value(&timeline_stdout, "Source"),
        "manual-snapshot"
    );
    assert_eq!(
        field_line_value(&timeline_stdout, "Author"),
        "agent@example"
    );

    Ok(())
}

#[test]
fn init_reports_existing_repository_error() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();

    era(work).arg("init").assert().success();

    let second_init = era(work).arg("init").assert().failure();
    let stderr = output_text(&second_init.get_output().stderr)?;

    assert_no_ansi(&stderr);
    assert!(stderr.contains("error: repository is already initialized:"));
    Ok(())
}

#[test]
fn snap_status_and_timeline_report_missing_repository_error() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();

    let snap = era(work)
        .args(["snap", "--message", "outside repo"])
        .assert()
        .failure();
    let snap_stderr = output_text(&snap.get_output().stderr)?;
    assert_no_ansi(&snap_stderr);
    assert!(snap_stderr.contains("error: not an Era repository:"));

    let status = era(work).arg("status").assert().failure();
    let status_stderr = output_text(&status.get_output().stderr)?;
    assert_no_ansi(&status_stderr);
    assert!(status_stderr.contains("error: not an Era repository:"));

    let timeline = era(work).arg("timeline").assert().failure();
    let timeline_stderr = output_text(&timeline.get_output().stderr)?;
    assert_no_ansi(&timeline_stderr);
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

fn field_line_value<'a>(output: &'a str, label: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| {
            line.trim_start()
                .strip_prefix(label)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| panic!("missing label {label:?} in {output:?}"))
}

fn assert_short_object_id(value: &str) {
    assert_eq!(value.len(), 12, "short object ID should be 12 hex chars");
    assert_hex(value);
}

fn assert_object_id(value: &str) {
    assert_eq!(value.len(), 64, "object ID should be 64 hex chars");
    assert_hex(value);
}

fn assert_hex(value: &str) {
    assert!(
        value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "object ID should be hex: {value}"
    );
}

fn assert_no_ansi(output: &str) {
    assert!(
        !output.contains("\u{1b}"),
        "captured output should not contain ANSI escapes: {output:?}"
    );
}
