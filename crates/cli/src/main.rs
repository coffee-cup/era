use anstyle::{AnsiColor, Style};
use chrono::{DateTime, Local};
use clap::{Parser, Subcommand};
use era_core::{ObjectId, Snapshot};
use era_materialization::{CaptureIssueKind, CaptureStats, FilesystemMaterializer};
use era_repository::{
    BranchHead, BranchName, BranchOperationResult, Repository, RepositoryError, RestoreResult,
    SnapshotRequest, SnapshotResult, SwitchResult, TimelineEntry, TreeChange, TreeChangeKind,
    WorkingTreeStatus,
};
use std::{error::Error, fmt, path::PathBuf, process::ExitCode};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let style = error_style();
            anstream::eprintln!("{style}error:{style:#} {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "era", about = "Cheap snapshots and dense local history")]
struct Cli {
    /// Show full object IDs, root tree IDs, timestamps, and capture stats.
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Initialize an Era repository in the current directory.
    Init,
    /// Capture a manual snapshot of the current repository.
    Snap {
        /// Human-facing label attached to the snapshot. Defaults to the current local time.
        #[arg(value_name = "LABEL", conflicts_with = "message")]
        label: Option<String>,
        /// Human-facing label attached to the snapshot. Alias for the positional label.
        #[arg(short, long, value_name = "MESSAGE")]
        message: Option<String>,
        /// Optional author recorded on the snapshot.
        #[arg(long)]
        author: Option<String>,
    },
    /// Show the current repository state.
    Status,
    /// List branches or create a branch at the current state.
    Branch {
        /// Branch name to create. Omit to list branches.
        name: Option<String>,
    },
    /// Switch to an existing branch, saving current work first.
    Switch {
        /// Branch name to switch to.
        name: String,
    },
    /// Restore a snapshot ID, unique prefix, or exact label into the working directory.
    Restore {
        /// Snapshot ID, unique ID prefix, or exact snapshot label.
        target: String,
    },
    /// Show the current branch timeline, newest snapshot first.
    Timeline,
}

async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Commands::Init => init(cli.verbose).await,
        Commands::Snap {
            label,
            message,
            author,
        } => snap(label, message, author, cli.verbose).await,
        Commands::Status => status(cli.verbose).await,
        Commands::Branch { name } => branch(name, cli.verbose).await,
        Commands::Switch { name } => switch(name, cli.verbose).await,
        Commands::Restore { target } => restore(target, cli.verbose).await,
        Commands::Timeline => timeline(cli.verbose).await,
    }
}

async fn init(verbose: bool) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let result = Repository::init(
        current_directory()?,
        &materializer,
        SnapshotRequest::initial(),
    )
    .await?;
    let branch = result.repository.current_branch().await?;

    print_init_result(&result, &branch, verbose);
    print_capture_warnings(&result.snapshot);
    Ok(())
}

async fn snap(
    label: Option<String>,
    message: Option<String>,
    author: Option<String>,
    verbose: bool,
) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = Repository::open(current_directory()?).await?;
    let branch = repository.current_branch().await?;
    let message = message.or(label).unwrap_or_else(default_snapshot_message);
    let mut request = SnapshotRequest::manual(message);
    if let Some(author) = author {
        request = request.with_author(author);
    }

    let result = repository.snapshot(&materializer, request).await?;

    print_snapshot_result("Created snapshot", &result, &branch, verbose);
    print_capture_warnings(&result);
    Ok(())
}

async fn status(verbose: bool) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = Repository::open(current_directory()?).await?;
    let branch = repository.current_branch().await?;
    let timeline = repository.timeline().await?;
    if timeline.is_empty() {
        return Err(CliError::EmptyTimeline);
    }
    let status = repository.working_tree_status(&materializer).await?;
    let branch_ref = repository.current_branch_ref_path().await?;

    print_status(
        &repository,
        &branch,
        &status,
        timeline.len(),
        verbose,
        &branch_ref,
    );
    print_scan_warnings(&status.comparison.issues);
    Ok(())
}

async fn branch(name: Option<String>, verbose: bool) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = Repository::open(current_directory()?).await?;

    match name {
        Some(name) => {
            let branch = BranchName::new(name).map_err(RepositoryError::from)?;
            let result = repository.create_branch(&materializer, branch).await?;
            print_branch_created(&result, verbose);
            print_optional_saved_warnings(result.saved_snapshot.as_ref());
        }
        None => {
            let branches = repository.branches().await?;
            print_branches(&branches, verbose);
        }
    }

    Ok(())
}

async fn switch(name: String, verbose: bool) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = Repository::open(current_directory()?).await?;
    let branch = BranchName::new(name).map_err(RepositoryError::from)?;
    let result = repository.switch_branch(&materializer, branch).await?;

    print_switch_result(&result, verbose);
    print_optional_saved_warnings(result.saved_snapshot.as_ref());
    Ok(())
}

async fn restore(target: String, verbose: bool) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = Repository::open(current_directory()?).await?;
    let result = repository.restore(&materializer, &target).await?;

    print_restore_result(&result, verbose);
    print_optional_saved_warnings(result.saved_snapshot.as_ref());
    Ok(())
}

async fn timeline(verbose: bool) -> Result<(), CliError> {
    let repository = Repository::open(current_directory()?).await?;
    let branch = repository.current_branch().await?;
    let entries = repository.timeline().await?;

    print_timeline(&branch, &entries, verbose);
    Ok(())
}

fn current_directory() -> Result<PathBuf, CliError> {
    std::env::current_dir().map_err(|source| CliError::CurrentDirectory { source })
}

fn default_snapshot_message() -> String {
    format_snapshot_message_time(Local::now())
}

fn format_snapshot_message_time(timestamp: DateTime<Local>) -> String {
    timestamp.format("%b %-d, %Y %H:%M:%S").to_string()
}

fn print_init_result(result: &era_repository::InitResult, branch: &BranchName, verbose: bool) {
    let metadata = result.repository.metadata_dir().display();
    let style = success_style();
    anstream::println!("{style}Initialized Era repository in {metadata}{style:#}");

    if verbose {
        anstream::println!();
        print_snapshot_details(&result.snapshot, branch);
    }
}

fn print_snapshot_result(
    heading: &str,
    result: &SnapshotResult,
    branch: &BranchName,
    verbose: bool,
) {
    print_success_heading(heading);
    print_field(
        "Snapshot",
        styled(accent_style(), short_id(result.snapshot_id)),
    );

    if let Some(message) = result.snapshot.message() {
        print_field("Message", message);
    }

    print_field("Captured", capture_summary(&result.capture.stats));

    if verbose {
        anstream::println!();
        print_snapshot_details(result, branch);
    }
}

fn print_success_heading(heading: &str) {
    let style = success_style();
    anstream::println!("{style}✓{style:#} {heading}");
}

fn print_field(label: &str, value: impl fmt::Display) {
    let label_style = label_style();
    anstream::println!("  {label_style}{label:<10}{label_style:#} {value}");
}

fn print_snapshot_details(result: &SnapshotResult, branch: &BranchName) {
    print_section("Details");
    print_detail("Full snapshot", result.snapshot_id);
    print_detail("Root tree", result.snapshot.root_tree_id());
    print_detail("Branch", branch);
    print_detail(
        "Timestamp",
        format!("{} ms", result.snapshot.timestamp_millis()),
    );
    print_detail("Parents", result.snapshot.parents().len());
    print_detail("Source", result.snapshot.provenance().source());

    if let Some(author) = result.snapshot.author() {
        print_detail("Author", author);
    }

    let stats = &result.capture.stats;
    print_detail("Files", stats.files_seen);
    print_detail("Directories", stats.directories_seen);
    print_detail("Bytes", stats.bytes_read);
    print_detail("Blobs stored", stats.blobs_stored);
    print_detail("Trees stored", stats.trees_stored);
    print_detail("Ignored", stats.ignored_entries);
    print_detail("Symlinks", stats.symlinks_skipped);
}

fn print_section(title: &str) {
    let style = label_style().bold();
    anstream::println!("{style}{title}{style:#}");
}

fn print_detail(label: &str, value: impl fmt::Display) {
    let label_style = label_style();
    anstream::println!("  {label_style}{label:<14}{label_style:#} {value}");
}

fn print_status(
    repository: &Repository,
    branch: &BranchName,
    status: &WorkingTreeStatus,
    timeline_len: usize,
    verbose: bool,
    branch_ref: &std::path::Path,
) {
    print_success_heading("Repository status");
    let snapshot = &status.snapshot;
    print_field("Branch", branch);
    print_field(
        "Snapshot",
        styled(accent_style(), short_id(status.snapshot_id)),
    );
    print_field("Timeline", pluralize(timeline_len, "snapshot", "snapshots"));
    let working = if status.is_clean() {
        "no changes"
    } else {
        "changes detected; run `era snap` to save"
    };
    print_field("Working", working);

    if let Some(message) = snapshot.message() {
        print_field("Message", message);
    }

    print_status_changes(status.changes());

    if verbose {
        anstream::println!();
        print_section("Details");
        print_detail("Root", repository.root().display());
        print_detail("Metadata", repository.metadata_dir().display());
        print_detail("Objects", repository.object_store().root().display());
        print_detail("HEAD", repository.head_path().display());
        print_detail("Branch ref", branch_ref.display());
        print_detail("Full snapshot", status.snapshot_id);
        print_detail("Root tree", snapshot.root_tree_id());
        print_detail("Current tree", status.current_root_tree_id);
        print_detail("Timestamp", format!("{} ms", snapshot.timestamp_millis()));
        print_detail("Parents", snapshot.parents().len());
        print_detail("Source", snapshot.provenance().source());

        if let Some(author) = snapshot.author() {
            print_detail("Author", author);
        }

        let stats = &status.comparison.stats;
        print_detail("Files", stats.files_seen);
        print_detail("Directories", stats.directories_seen);
        print_detail("Bytes", stats.bytes_read);
        print_detail("Ignored", stats.ignored_entries);
        print_detail("Symlinks", stats.symlinks_skipped);
    }
}

fn print_status_changes(changes: &[TreeChange]) {
    if changes.is_empty() {
        return;
    }

    anstream::println!();
    print_section("Changes");
    for change in changes {
        let marker = styled(change_style(change.kind), change_marker(change.kind));
        anstream::println!("  {marker} {}", change.path.display());
    }
}

fn change_marker(kind: TreeChangeKind) -> &'static str {
    match kind {
        TreeChangeKind::Added => "A",
        TreeChangeKind::Modified => "M",
        TreeChangeKind::Deleted => "D",
        TreeChangeKind::TypeChanged => "T",
    }
}

fn change_style(kind: TreeChangeKind) -> Style {
    match kind {
        TreeChangeKind::Added => success_style(),
        TreeChangeKind::Modified => warning_style(),
        TreeChangeKind::Deleted => error_style(),
        TreeChangeKind::TypeChanged => timeline_style(),
    }
}

fn print_branch_created(result: &BranchOperationResult, verbose: bool) {
    print_success_heading("Created branch");
    print_field("Branch", &result.branch);
    print_field(
        "Snapshot",
        styled(accent_style(), short_id(result.snapshot_id)),
    );

    if verbose {
        print_optional_saved_snapshot(result.saved_snapshot.as_ref());
    }
}

fn print_branches(branches: &[BranchHead], verbose: bool) {
    anstream::println!("Branches");
    for branch in branches {
        let marker = if branch.is_current { "*" } else { " " };
        let name = if branch.is_current {
            styled(accent_style(), &branch.name)
        } else {
            branch.name.to_string()
        };
        anstream::println!(
            "{marker} {name} {}",
            styled(accent_style(), short_id(branch.snapshot_id))
        );

        if verbose {
            print_indented_detail("Full snapshot", branch.snapshot_id);
        }
    }
}

fn print_switch_result(result: &SwitchResult, verbose: bool) {
    print_success_heading("Switched branch");
    print_field("Branch", &result.branch);
    print_field(
        "Snapshot",
        styled(accent_style(), short_id(result.snapshot_id)),
    );

    if let Some(message) = result.snapshot.message() {
        print_field("Message", message);
    }

    if verbose {
        print_materialize_details(&result.materialization);
        print_optional_saved_snapshot(result.saved_snapshot.as_ref());
    }
}

fn print_restore_result(result: &RestoreResult, verbose: bool) {
    print_success_heading("Restored snapshot");
    print_field(
        "Snapshot",
        styled(accent_style(), short_id(result.snapshot_id)),
    );

    if let Some(message) = result.snapshot.message() {
        print_field("Message", message);
    }

    if verbose {
        print_materialize_details(&result.materialization);
        print_optional_saved_snapshot(result.saved_snapshot.as_ref());
    }
}

fn print_materialize_details(result: &era_materialization::MaterializeResult) {
    anstream::println!();
    print_section("Materialized");
    print_detail("Files written", result.stats.files_written);
    print_detail("Dirs created", result.stats.directories_created);
    print_detail("Entries removed", result.stats.entries_removed);
    print_detail("Bytes written", result.stats.bytes_written);
}

fn print_optional_saved_snapshot(saved: Option<&SnapshotResult>) {
    if let Some(saved) = saved {
        anstream::println!();
        print_section("Saved current work");
        print_detail("Snapshot", saved.snapshot_id);
        print_detail("Root tree", saved.snapshot.root_tree_id());
    }
}

fn print_optional_saved_warnings(saved: Option<&SnapshotResult>) {
    if let Some(saved) = saved {
        print_capture_warnings(saved);
    }
}

fn print_capture_warnings(result: &SnapshotResult) {
    print_scan_warnings(&result.capture.issues);
}

fn print_scan_warnings(issues: &[era_materialization::CaptureIssue]) {
    for issue in issues {
        let description = match issue.kind {
            CaptureIssueKind::SkippedSymlink => "skipped symlink",
        };
        let style = warning_style();
        anstream::eprintln!(
            "{style}warning:{style:#} {description}: {}",
            issue.path.display()
        );
    }
}

fn print_timeline(branch: &BranchName, entries: &[TimelineEntry], verbose: bool) {
    anstream::println!("Timeline for {}", styled(accent_style(), branch));

    for entry in entries {
        print_timeline_entry(entry.snapshot_id, &entry.snapshot, verbose);
    }
}

fn print_timeline_entry(snapshot_id: ObjectId, snapshot: &Snapshot, verbose: bool) {
    let dot = styled(timeline_style(), "●");
    let id = styled(accent_style(), short_id(snapshot_id));
    anstream::println!("{dot} {id}  {}", timeline_title(snapshot));

    if verbose {
        print_timeline_details(snapshot_id, snapshot);
    }
}

fn print_timeline_details(snapshot_id: ObjectId, snapshot: &Snapshot) {
    print_indented_detail("Full snapshot", snapshot_id);
    print_indented_detail("Root tree", snapshot.root_tree_id());
    print_indented_detail("Timestamp", format!("{} ms", snapshot.timestamp_millis()));
    print_indented_detail("Parents", snapshot.parents().len());
    print_indented_detail("Source", snapshot.provenance().source());

    if let Some(author) = snapshot.author() {
        print_indented_detail("Author", author);
    }
}

fn print_indented_detail(label: &str, value: impl fmt::Display) {
    let label_style = label_style();
    anstream::println!("    {label_style}{label:<14}{label_style:#} {value}");
}

fn timeline_title(snapshot: &Snapshot) -> String {
    match snapshot.message() {
        Some(message) if !message.is_empty() => message.to_owned(),
        _ if snapshot.provenance().source() == "repository-init" => "repository init".to_owned(),
        _ if snapshot.provenance().source() == "auto-snapshot" => "auto snapshot".to_owned(),
        _ => snapshot.provenance().source().to_owned(),
    }
}

fn capture_summary(stats: &CaptureStats) -> String {
    format!(
        "{}, {}, {}",
        pluralize(stats.files_seen, "file", "files"),
        pluralize(stats.directories_seen, "directory", "directories"),
        format_bytes(stats.bytes_read),
    )
}

fn pluralize(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{count} {noun}")
}

fn format_bytes(bytes: u64) -> String {
    format!("{bytes} B")
}

fn short_id(id: ObjectId) -> String {
    id.to_hex().chars().take(12).collect()
}

fn styled(style: Style, value: impl fmt::Display) -> String {
    format!("{style}{value}{style:#}")
}

fn success_style() -> Style {
    AnsiColor::Green.on_default().bold()
}

fn accent_style() -> Style {
    AnsiColor::Cyan.on_default().bold()
}

fn timeline_style() -> Style {
    AnsiColor::Magenta.on_default().bold()
}

fn warning_style() -> Style {
    AnsiColor::Yellow.on_default().bold()
}

fn error_style() -> Style {
    AnsiColor::Red.on_default().bold()
}

fn label_style() -> Style {
    Style::new().dimmed()
}

#[derive(Debug)]
enum CliError {
    CurrentDirectory { source: std::io::Error },
    EmptyTimeline,
    Repository { source: Box<RepositoryError> },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory { source } => {
                write!(formatter, "could not determine current directory: {source}")
            }
            Self::EmptyTimeline => write!(formatter, "repository timeline is empty"),
            Self::Repository { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentDirectory { source } => Some(source),
            Self::EmptyTimeline => None,
            Self::Repository { source } => Some(source.as_ref()),
        }
    }
}

impl From<RepositoryError> for CliError {
    fn from(source: RepositoryError) -> Self {
        Self::Repository {
            source: Box::new(source),
        }
    }
}

fn init_tracing() {
    let filter = tracing_filter();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn tracing_filter() -> EnvFilter {
    tracing_filter_from_directive(
        std::env::var("ERA_LOG")
            .or_else(|_| std::env::var("RUST_LOG"))
            .ok(),
    )
}

fn tracing_filter_from_directive(directive: Option<String>) -> EnvFilter {
    directive
        .and_then(|directive| EnvFilter::try_new(directive).ok())
        .unwrap_or_else(|| EnvFilter::new("off"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_snapshot_message_uses_readable_local_time() {
        use chrono::TimeZone as _;

        let timestamp = Local
            .with_ymd_and_hms(2024, 1, 1, 11, 11, 11)
            .single()
            .expect("test timestamp should exist in the local timezone");

        assert_eq!(
            format_snapshot_message_time(timestamp),
            "Jan 1, 2024 11:11:11"
        );
    }

    #[test]
    fn tracing_filter_accepts_valid_directive() {
        let filter = tracing_filter_from_directive(Some("era_object_store=debug".to_owned()));

        assert_eq!(filter.to_string(), "era_object_store=debug");
    }

    #[test]
    fn tracing_filter_defaults_to_off_for_missing_or_invalid_directive() {
        let missing = tracing_filter_from_directive(None);
        let invalid = tracing_filter_from_directive(Some("era_object_store=notalevel".to_owned()));

        assert_eq!(missing.to_string(), "off");
        assert_eq!(invalid.to_string(), "off");
    }
}
