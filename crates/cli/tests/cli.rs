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
    assert_eq!(first_lines.len(), 6);
    assert_eq!(first_lines[0], "Snapshot tree");
    assert_eq!(
        field_line_value(&first_timeline_stdout, "Cursor"),
        format!("branch main @ {initial_snapshot}")
    );
    assert_eq!(
        field_line_value(&first_timeline_stdout, "Worktree"),
        "clean at cursor"
    );
    assert_eq!(
        field_line_value(&first_timeline_stdout, "Snapshots"),
        "1 snapshot"
    );
    assert!(first_lines[5].starts_with(&format!("@ {initial_snapshot}  ")));
    assert!(first_lines[5].contains("repository init"));

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
    assert_eq!(timeline_lines.len(), 7);
    assert_eq!(timeline_lines[0], "Snapshot tree");
    assert_eq!(
        field_line_value(&timeline_stdout, "Cursor"),
        format!("branch main @ {second_snapshot}")
    );
    assert_eq!(
        field_line_value(&timeline_stdout, "Worktree"),
        "clean at cursor"
    );
    assert_eq!(
        field_line_value(&timeline_stdout, "Snapshots"),
        "2 snapshots"
    );
    assert!(timeline_lines[5].starts_with(&format!("● {initial_snapshot}  ")));
    assert!(timeline_lines[5].contains("repository init"));
    assert!(timeline_lines[6].starts_with(&format!("└─@ {second_snapshot}  ")));
    assert!(timeline_lines[6].contains("feature checkpoint"));

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
    assert!(timeline_stdout.contains("Snapshot tree\n"));
    assert!(timeline_stdout.contains("Full snapshot"));
    assert!(timeline_stdout.contains("Root tree"));
    assert!(timeline_stdout.contains("Source         manual-snapshot"));
    assert!(timeline_stdout.contains("Author         agent@example"));

    Ok(())
}

#[test]
fn one_shot_commands_reuse_persistent_capture_cache() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"hello\n")?;

    era(work).arg("init").assert().success();
    era(work).args(["status", "--verbose"]).assert().success();
    let second_status = era(work).args(["status", "--verbose"]).assert().success();
    let stdout = output_text(&second_status.get_output().stdout)?;

    assert_eq!(field_line_value(&stdout, "Bytes"), "0");
    assert_eq!(field_line_value(&stdout, "Cache hits"), "1");
    assert!(
        work.join(".era/workspaces/default/cache/capture-v2.redb")
            .is_file()
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

    let watch = era(work).args(["watch", "--once"]).assert().failure();
    let watch_stderr = output_text(&watch.get_output().stderr)?;
    assert_no_ansi(&watch_stderr);
    assert!(watch_stderr.contains("error: not an Era repository:"));

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
    assert!(dirty_stdout.contains("\nChanges\n  M README.md\n"));

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
fn watch_once_saves_dirty_work_as_unlabeled_auto_snapshot() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();

    fs::write(work.join("README.md"), b"two")?;
    let watch = era(work)
        .args([
            "--verbose",
            "watch",
            "--once",
            "--workspace",
            "agent-1",
            "--agent",
            "claude",
            "--task",
            "fix-parser",
            "--model",
            "sonnet",
        ])
        .assert()
        .success();
    let watch_stdout = output_text(&watch.get_output().stdout)?;
    assert_no_ansi(&watch_stdout);
    assert!(watch_stdout.starts_with("Saved auto snapshot "));
    let watch_lines = lines(&watch_stdout);
    let saved_snapshot = watch_lines[0]
        .strip_prefix("Saved auto snapshot ")
        .expect("watch output should include saved snapshot ID");
    assert_short_object_id(saved_snapshot);
    assert_eq!(field_line_value(&watch_stdout, "Source"), "auto-snapshot");
    assert_eq!(field_line_value(&watch_stdout, "trigger"), "reconcile");
    assert_eq!(field_line_value(&watch_stdout, "workspace"), "agent-1");
    assert_eq!(field_line_value(&watch_stdout, "agent"), "claude");
    assert_eq!(field_line_value(&watch_stdout, "task"), "fix-parser");
    assert_eq!(field_line_value(&watch_stdout, "model"), "sonnet");

    let status = era(work).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_eq!(field_line_value(&status_stdout, "Working"), "no changes");

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    let timeline_lines = lines(&timeline_stdout);
    assert_eq!(timeline_lines[0], "Snapshot tree");
    assert!(timeline_stdout.contains(&format!("└─@ {saved_snapshot}  auto snapshot · ")));

    Ok(())
}

#[test]
fn timeline_collapses_linear_auto_snapshot_spam() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();

    let mut current_snapshot = String::new();
    for index in 0..4 {
        fs::write(work.join("README.md"), format!("auto {index}"))?;
        let watch = era(work).args(["watch", "--once"]).assert().success();
        let watch_stdout = output_text(&watch.get_output().stdout)?;
        current_snapshot = watch_stdout
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("Saved auto snapshot "))
            .expect("watch should save a snapshot")
            .to_owned();
    }

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;

    assert!(timeline_stdout.contains("… 3 auto snapshots · "));
    assert!(timeline_stdout.contains(&format!("└─@ {current_snapshot}  auto snapshot · ")));
    assert_eq!(
        field_line_value(&timeline_stdout, "Snapshots"),
        "5 snapshots"
    );

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
fn workspace_add_creates_external_workspace_and_commands_infer_pointer()
-> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path().join("project");
    let agent = temp.path().join("agent-1");
    fs::create_dir(&work)?;
    fs::write(work.join("README.md"), b"one")?;
    era(&work).arg("init").assert().success();

    let add = era(&work)
        .args(["workspace", "add", agent.to_str().unwrap()])
        .assert()
        .success();
    let add_stdout = output_text(&add.get_output().stdout)?;
    assert_no_ansi(&add_stdout);
    assert!(add_stdout.starts_with("✓ Added workspace\n"));
    assert_eq!(field_line_value(&add_stdout, "Workspace"), "agent-1");
    assert_eq!(fs::read(agent.join("README.md"))?, b"one");
    assert!(agent.join(".era").is_file());

    let status = era(&agent).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_eq!(field_line_value(&status_stdout, "Workspace"), "agent-1");
    assert_eq!(field_line_value(&status_stdout, "Working"), "no changes");

    fs::write(agent.join("README.md"), b"agent work")?;
    era(&agent).arg("snap").assert().success();
    let agent_status = era(&agent).arg("status").assert().success();
    let agent_status_stdout = output_text(&agent_status.get_output().stdout)?;
    assert_eq!(
        field_line_value(&agent_status_stdout, "Working"),
        "no changes"
    );

    let root_status = era(&work).arg("status").assert().success();
    let root_status_stdout = output_text(&root_status.get_output().stdout)?;
    assert_eq!(field_line_value(&root_status_stdout, "Branch"), "main");
    assert_eq!(
        field_line_value(&root_status_stdout, "Working"),
        "no changes"
    );
    assert_eq!(fs::read(work.join("README.md"))?, b"one");

    let list = era(&work).args(["workspace", "list"]).assert().success();
    let list_stdout = output_text(&list.get_output().stdout)?;
    assert!(list_stdout.contains("agent-1"));
    assert!(list_stdout.contains(agent.to_str().unwrap()));

    Ok(())
}

#[test]
fn repo_workspace_options_lazily_connect_existing_directory() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let project = temp.path().join("project");
    let agent = temp.path().join("agent-lazy");
    fs::create_dir(&project)?;
    fs::write(project.join("README.md"), b"base")?;
    era(&project).arg("init").assert().success();
    fs::create_dir(&agent)?;
    fs::write(agent.join("README.md"), b"agent work")?;

    let snap = era(&agent)
        .args([
            "snap",
            "--repo",
            project.to_str().unwrap(),
            "--workspace",
            "agent-lazy",
        ])
        .assert()
        .success();
    let snap_stdout = output_text(&snap.get_output().stdout)?;
    assert!(snap_stdout.starts_with("✓ Created snapshot\n"));
    assert!(agent.join(".era").is_file());

    let status = era(&agent).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_eq!(field_line_value(&status_stdout, "Workspace"), "agent-lazy");
    assert_eq!(field_line_value(&status_stdout, "Working"), "no changes");

    let list = era(&project).args(["workspace", "list"]).assert().success();
    let list_stdout = output_text(&list.get_output().stdout)?;
    assert!(list_stdout.contains("agent-lazy"));

    Ok(())
}

#[test]
fn workspace_add_rejects_nested_paths() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    era(work).arg("init").assert().success();

    let nested = era(work)
        .args(["workspace", "add", "agent-1"])
        .assert()
        .failure();
    let stderr = output_text(&nested.get_output().stderr)?;
    assert_no_ansi(&stderr);
    assert!(stderr.contains("error: refusing to create a workspace inside another workspace"));

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
fn restore_moves_cursor_to_restored_snapshot() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();

    fs::write(work.join("README.md"), b"two")?;
    let second = era(work).args(["snap", "two"]).assert().success();
    let second_stdout = output_text(&second.get_output().stdout)?;
    let second_snapshot = field_line_value(&second_stdout, "Snapshot").to_owned();

    fs::write(work.join("README.md"), b"three")?;
    let third = era(work).args(["snap", "three"]).assert().success();
    let third_stdout = output_text(&third.get_output().stdout)?;
    let third_snapshot = field_line_value(&third_stdout, "Snapshot").to_owned();

    let restore = era(work).args(["restore", "two"]).assert().success();
    let restore_stdout = output_text(&restore.get_output().stdout)?;
    assert_eq!(
        field_line_value(&restore_stdout, "Cursor"),
        format!("branch main @ {second_snapshot}")
    );

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;

    assert_eq!(
        field_line_value(&timeline_stdout, "Cursor"),
        format!("branch main @ {second_snapshot}")
    );
    assert_eq!(
        field_line_value(&timeline_stdout, "Worktree"),
        "clean at cursor"
    );
    assert!(timeline_stdout.contains(&format!(
        "@ {second_snapshot}  two  main, current, worktree"
    )));
    assert!(timeline_stdout.contains(&format!("● {third_snapshot}  three")));

    Ok(())
}

#[test]
fn restore_then_snap_branches_from_restored_snapshot() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();

    fs::write(work.join("README.md"), b"two")?;
    let second = era(work).args(["snap", "two"]).assert().success();
    let second_stdout = output_text(&second.get_output().stdout)?;
    let second_snapshot = field_line_value(&second_stdout, "Snapshot").to_owned();

    fs::write(work.join("README.md"), b"three")?;
    let third = era(work).args(["snap", "three"]).assert().success();
    let third_stdout = output_text(&third.get_output().stdout)?;
    let third_snapshot = field_line_value(&third_stdout, "Snapshot").to_owned();

    era(work).args(["restore", "two"]).assert().success();
    fs::write(work.join("README.md"), b"side")?;
    let side = era(work).args(["snap", "side"]).assert().success();
    let side_stdout = output_text(&side.get_output().stdout)?;
    let side_snapshot = field_line_value(&side_stdout, "Snapshot").to_owned();

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;

    assert_eq!(
        field_line_value(&timeline_stdout, "Cursor"),
        format!("branch main @ {side_snapshot}")
    );
    assert!(timeline_stdout.contains(&format!("● {second_snapshot}  two")));
    assert!(timeline_stdout.contains(&format!("├─● {third_snapshot}  three")));
    assert!(timeline_stdout.contains(&format!(
        "└─@ {side_snapshot}  side  main, current, worktree"
    )));

    Ok(())
}

#[test]
fn restore_saves_dirty_work_before_moving_cursor() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();
    let status = era(work).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    let initial_snapshot = field_line_value(&status_stdout, "Snapshot").to_owned();

    fs::write(work.join("README.md"), b"two")?;
    era(work).args(["snap", "two"]).assert().success();
    fs::write(work.join("README.md"), b"three unsnapped")?;

    let restore = era(work)
        .args(["--verbose", "restore", &initial_snapshot])
        .assert()
        .success();
    let restore_stdout = output_text(&restore.get_output().stdout)?;
    assert_eq!(
        field_line_value(&restore_stdout, "Cursor"),
        format!("branch main @ {initial_snapshot}")
    );
    assert!(restore_stdout.contains("Saved current work\n"));
    assert_eq!(field_line_value(&restore_stdout, "Source"), "auto-snapshot");
    assert_eq!(field_line_value(&restore_stdout, "trigger"), "safety");
    assert_eq!(fs::read(work.join("README.md"))?, b"one");

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    assert_eq!(
        field_line_value(&timeline_stdout, "Cursor"),
        format!("branch main @ {initial_snapshot}")
    );
    assert_eq!(
        field_line_value(&timeline_stdout, "Worktree"),
        "clean at cursor"
    );
    assert_eq!(
        field_line_value(&timeline_stdout, "Snapshots"),
        "3 snapshots"
    );
    let safety_snapshot = timeline_stdout
        .lines()
        .find(|line| line.contains("auto snapshot · "))
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("timeline should show safety snapshot")
        .to_owned();

    era(work)
        .args(["restore", &safety_snapshot])
        .assert()
        .success();
    assert_eq!(fs::read(work.join("README.md"))?, b"three unsnapped");

    Ok(())
}

#[test]
fn workspace_restore_moves_workspace_cursor_without_switching_branch() -> Result<(), Box<dyn Error>>
{
    let temp = TempDir::new()?;
    let project = temp.path().join("project");
    let agent = temp.path().join("agent");
    fs::create_dir(&project)?;
    fs::write(project.join("README.md"), b"one")?;
    era(&project).arg("init").assert().success();

    fs::write(project.join("README.md"), b"two")?;
    let second = era(&project).args(["snap", "two"]).assert().success();
    let second_stdout = output_text(&second.get_output().stdout)?;
    let second_snapshot = field_line_value(&second_stdout, "Snapshot").to_owned();

    era(&project)
        .args(["workspace", "add", agent.to_str().unwrap(), "--from", "two"])
        .assert()
        .success();
    fs::write(agent.join("README.md"), b"agent work")?;
    let agent_snap = era(&agent).args(["snap", "agent work"]).assert().success();
    let agent_stdout = output_text(&agent_snap.get_output().stdout)?;
    let agent_snapshot = field_line_value(&agent_stdout, "Snapshot").to_owned();

    let restore = era(&agent).args(["restore", "two"]).assert().success();
    let restore_stdout = output_text(&restore.get_output().stdout)?;
    assert_eq!(
        field_line_value(&restore_stdout, "Cursor"),
        format!("workspace agent @ {second_snapshot}")
    );

    let agent_timeline = era(&agent).arg("timeline").assert().success();
    let agent_timeline_stdout = output_text(&agent_timeline.get_output().stdout)?;
    assert_eq!(
        field_line_value(&agent_timeline_stdout, "Cursor"),
        format!("workspace agent @ {second_snapshot}")
    );
    assert!(agent_timeline_stdout.contains(&format!(
        "@ {second_snapshot}  two  agent, main, current, worktree"
    )));
    assert!(agent_timeline_stdout.contains(&format!("● {agent_snapshot}  agent work")));

    let project_timeline = era(&project).arg("timeline").assert().success();
    let project_timeline_stdout = output_text(&project_timeline.get_output().stdout)?;
    assert_eq!(
        field_line_value(&project_timeline_stdout, "Cursor"),
        format!("branch main @ {second_snapshot}")
    );

    Ok(())
}

#[test]
fn snap_without_label_saves_only_when_files_changed() -> Result<(), Box<dyn Error>> {
    let temp = TempDir::new()?;
    let work = temp.path();
    fs::write(work.join("README.md"), b"one")?;
    era(work).arg("init").assert().success();

    let clean_snap = era(work).arg("snap").assert().success();
    let clean_stdout = output_text(&clean_snap.get_output().stdout)?;
    assert_eq!(clean_stdout, "No changes\n");

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    assert_eq!(lines(&timeline_stdout).len(), 6);

    fs::write(work.join("README.md"), b"two")?;
    let snap = era(work).arg("snap").assert().success();
    let snap_stdout = output_text(&snap.get_output().stdout)?;
    assert!(snap_stdout.starts_with("✓ Created snapshot\n"));
    assert!(!snap_stdout.contains("Message"));
    assert_short_object_id(field_line_value(&snap_stdout, "Snapshot"));

    let status = era(work).arg("status").assert().success();
    let status_stdout = output_text(&status.get_output().stdout)?;
    assert_eq!(field_line_value(&status_stdout, "Working"), "no changes");

    let timeline = era(work).arg("timeline").assert().success();
    let timeline_stdout = output_text(&timeline.get_output().stdout)?;
    let timeline_lines = lines(&timeline_stdout);
    assert_eq!(timeline_lines.len(), 7);
    assert!(timeline_lines[6].contains("snapshot · "));
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
