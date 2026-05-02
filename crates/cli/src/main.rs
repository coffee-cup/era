use clap::{Parser, Subcommand};
use era_materialization::{CaptureIssueKind, FilesystemMaterializer};
use era_repository::{Repository, RepositoryError, SnapshotRequest, SnapshotResult, TimelineEntry};
use std::{error::Error, fmt, path::PathBuf, process::ExitCode};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "era", about = "Cheap snapshots and dense local history")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Initialize an Era repository in the current directory.
    Init,
    /// Capture a manual snapshot of the current repository.
    Snap {
        /// Human-facing message attached to the snapshot.
        #[arg(short, long)]
        message: String,
        /// Optional author recorded on the snapshot.
        #[arg(long)]
        author: Option<String>,
    },
    /// Show the current branch timeline, newest snapshot first.
    Timeline,
}

async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Commands::Init => init().await,
        Commands::Snap { message, author } => snap(message, author).await,
        Commands::Timeline => timeline().await,
    }
}

async fn init() -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let result = Repository::init(
        current_directory()?,
        &materializer,
        SnapshotRequest::initial(),
    )
    .await?;

    print_snapshot_result("initialized", &result.snapshot);
    print_capture_warnings(&result.snapshot);
    Ok(())
}

async fn snap(message: String, author: Option<String>) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = Repository::open(current_directory()?).await?;
    let mut request = SnapshotRequest::manual(message);
    if let Some(author) = author {
        request = request.with_author(author);
    }

    let result = repository.snapshot(&materializer, request).await?;

    print_snapshot_result("created", &result);
    print_capture_warnings(&result);
    Ok(())
}

async fn timeline() -> Result<(), CliError> {
    let repository = Repository::open(current_directory()?).await?;
    for entry in repository.timeline().await? {
        print_timeline_entry(&entry);
    }
    Ok(())
}

fn current_directory() -> Result<PathBuf, CliError> {
    std::env::current_dir().map_err(|source| CliError::CurrentDirectory { source })
}

fn print_snapshot_result(action: &str, result: &SnapshotResult) {
    let stats = &result.capture.stats;
    println!(
        "{action} snapshot={} root_tree={} files={} directories={} bytes={} blobs={} trees={} ignored={} symlinks_skipped={}",
        result.snapshot_id,
        result.snapshot.root_tree_id(),
        stats.files_seen,
        stats.directories_seen,
        stats.bytes_read,
        stats.blobs_stored,
        stats.trees_stored,
        stats.ignored_entries,
        stats.symlinks_skipped,
    );
}

fn print_capture_warnings(result: &SnapshotResult) {
    for issue in &result.capture.issues {
        let description = match issue.kind {
            CaptureIssueKind::SkippedSymlink => "skipped symlink",
        };
        eprintln!("warning: {description}: {}", issue.path.display());
    }
}

fn print_timeline_entry(entry: &TimelineEntry) {
    let snapshot = &entry.snapshot;
    println!(
        "{} timestamp={} parents={} source={} message={}",
        entry.snapshot_id,
        snapshot.timestamp_millis(),
        snapshot.parents().len(),
        snapshot.provenance().source(),
        snapshot.message().unwrap_or("")
    );
}

#[derive(Debug)]
enum CliError {
    CurrentDirectory { source: std::io::Error },
    Repository { source: Box<RepositoryError> },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory { source } => {
                write!(formatter, "could not determine current directory: {source}")
            }
            Self::Repository { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentDirectory { source } => Some(source),
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
