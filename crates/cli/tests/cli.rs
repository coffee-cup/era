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
    assert!(init_stdout.starts_with("Initialized Era repository in "));
    assert!(init_stdout.contains("/.era\n"));
    assert!(!init_stdout.contains("Snapshot"));
    assert!(!init_stdout.contains("Captured"));
    assert!(work.join(".era/HEAD").is_file());
    assert!(work.join(".era/refs/heads/main").is_file());
    let status = era(work).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_no_ansi(&status_stdout);
    assert!(status_stdout.starts_with("✓ Repository status\n"));
    assert_eq!(field_line_value(&status_stdout, "Branch"), "main");
    let initial_snapshot = field_line_value(&status_stdout, "Snapshot");
    assert_short_object_id(initial_snapshot);
    assert_eq!(field_line_value(&status_stdout, "Timeline"), "1 snapshot");
    assert_eq!(field_line_value(&status_stdout, "Working"), "no changes");

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

    let branch = era(work).arg("branch").assert().failure();
    let branch_stderr = output_text(&branch.get_output().stderr)?;
    assert_no_ansi(&branch_stderr);
    assert!(branch_stderr.contains("error: not an Era repository:"));

    let switch = era(work).args(["switch", "main"]).assert().failure();
    let switch_stderr = output_text(&switch.get_output().stderr)?;
    assert_no_ansi(&switch_stderr);
    assert!(switch_stderr.contains("error: not an Era repository:"));

    let restore = era(work).args(["restore", "main"]).assert().failure();
    let restore_stderr = output_text(&restore.get_output().stderr)?;
    assert_no_ansi(&restore_stderr);
    assert!(restore_stderr.contains("error: not an Era repository:"));

    Ok(())
}

#[test]
fn status_detects_changes_and_snap_makes_it_clean() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();

    fs::write(work.join("README.md"), b"two")?;
    let dirty = era(work).arg("status").assert().success();
    let dirty_stdout = output_text(&dirty.get_output().stdout)?;
    assert_eq!(
        field_line_value(&dirty_stdout, "Working"),
        "changes detected; run `era snap` to save"
    );

    era(work).args(["snap", "remember this"]).assert().success();
    let clean = era(work).arg("status").assert().success();
    let clean_stdout = output_text(&clean.get_output().stdout)?;
    assert_eq!(field_line_value(&clean_stdout, "Working"), "no changes");

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    assert!(timeline_stdout.contains("remember this"));
    Ok(())
}

#[test]
fn branch_switch_and_restore_support_local_workflow() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();

    let branch = era(work).args(["branch", "feature"]).assert().success();
    let branch_stdout = output_text(&branch.get_output().stdout)?;
    assert!(branch_stdout.starts_with("✓ Created branch\n"));
    assert_eq!(field_line_value(&branch_stdout, "Branch"), "feature");

    let branches = era(work).arg("branch").assert().success();
    let branches_stdout = output_text(&branches.get_output().stdout)?;
    assert_no_ansi(&branches_stdout);
    assert!(branches_stdout.contains("* main"));
    assert!(branches_stdout.contains("feature"));

    fs::write(work.join("README.md"), b"two")?;
    era(work).args(["snap", "main two"]).assert().success();

    let switch_feature = era(work).args(["switch", "feature"]).assert().success();
    let switch_feature_stdout = output_text(&switch_feature.get_output().stdout)?;
    assert!(switch_feature_stdout.starts_with("✓ Switched branch\n"));
    assert_eq!(
        field_line_value(&switch_feature_stdout, "Branch"),
        "feature"
    );
    assert_eq!(fs::read(work.join("README.md"))?, b"one");

    fs::write(work.join("README.md"), b"feature work")?;
    era(work).args(["switch", "main"]).assert().success();
    assert_eq!(fs::read(work.join("README.md"))?, b"two");

    era(work).args(["restore", "main two"]).assert().success();
    assert_eq!(fs::read(work.join("README.md"))?, b"two");
    let status = era(work).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_eq!(field_line_value(&status_stdout, "Working"), "no changes");

    Ok(())
}

#[test]
fn branch_switch_and_restore_report_clear_errors() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    era(work).arg("init").assert().success();
    era(work).args(["branch", "feature"]).assert().success();

    let duplicate = era(work).args(["branch", "feature"]).assert().failure();
    let duplicate_stderr = output_text(&duplicate.get_output().stderr)?;
    assert_no_ansi(&duplicate_stderr);
    assert!(duplicate_stderr.contains("error: branch already exists: feature"));

    let missing_branch = era(work).args(["switch", "missing"]).assert().failure();
    let missing_branch_stderr = output_text(&missing_branch.get_output().stderr)?;
    assert_no_ansi(&missing_branch_stderr);
    assert!(missing_branch_stderr.contains("error: branch not found: missing"));

    let missing_target = era(work).args(["restore", "missing"]).assert().failure();
    let missing_target_stderr = output_text(&missing_target.get_output().stderr)?;
    assert_no_ansi(&missing_target_stderr);
    assert!(missing_target_stderr.contains("error: snapshot target not found: missing"));

    Ok(())
}

#[test]
fn snap_without_message_uses_timestamp_message() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    era(work).arg("init").assert().success();

    let snap = era(work).arg("snap").assert().success();
    let stdout = output_text(&snap.get_output().stdout)?;
    let message = field_line_value(&stdout, "Message");

    assert_timestamp_message(message)?;
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

fn assert_timestamp_message(value: &str) -> Result<(), Box<dyn Error>> {
    let mut parts = value.split_whitespace();
    let month = parts.next().expect("timestamp should have month");
    let day = parts.next().expect("timestamp should have day");
    let year = parts.next().expect("timestamp should have year");
    let time = parts.next().expect("timestamp should have time");
    assert!(parts.next().is_none(), "timestamp should have four fields");
    assert!(
        matches!(
            month,
            "Jan"
                | "Feb"
                | "Mar"
                | "Apr"
                | "May"
                | "Jun"
                | "Jul"
                | "Aug"
                | "Sep"
                | "Oct"
                | "Nov"
                | "Dec"
        ),
        "unexpected month: {month}"
    );
    assert!(day.ends_with(','), "day should end with comma: {day}");
    let day_number = day.trim_end_matches(',').parse::<u8>()?;
    assert!((1..=31).contains(&day_number));
    let year_number = year.parse::<u16>()?;
    assert!(year_number >= 2024);
    let time_parts = time.split(':').collect::<Vec<_>>();
    assert_eq!(time_parts.len(), 3, "time should be HH:MM:SS");
    for part in time_parts {
        assert_eq!(part.len(), 2, "time fields should be zero-padded");
        assert!(part.parse::<u8>().is_ok(), "time field should be numeric");
    }
    Ok(())
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
