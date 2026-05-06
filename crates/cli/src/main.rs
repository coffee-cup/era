use anstyle::{AnsiColor, Style};
use chrono::Local;
use clap::{Parser, Subcommand};
use era_core::{ObjectId, Snapshot};
use era_materialization::{
    CaptureIssueKind, CaptureStats, FilesystemMaterializer, MaterializationError, WatchEvent,
    WorkingDirectory,
};
use era_repository::{
    AddWorkspaceOptions, AutoSnapshotTrigger, BranchHead, BranchName, BranchOperationResult,
    CursorInfo, Repository, RepositoryError, RestoreResult, SnapshotGraph, SnapshotRequest,
    SnapshotResult, SwitchResult, TimelineEntry, TreeChange, TreeChangeKind, WorkingTreeStatus,
    WorkspaceAddResult, WorkspaceHead, WorkspaceId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    path::PathBuf,
    pin::Pin,
    process::ExitCode,
    time::Duration,
};
use tokio::time::{self, MissedTickBehavior, Sleep};
use tracing_subscriber::EnvFilter;

const MIN_COLLAPSED_AUTO_SNAPSHOTS: usize = 3;

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
    /// Show full object IDs, root tree IDs, provenance, and capture/cache stats.
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Initialize an Era repository in the current directory.
    Init,
    /// Capture a snapshot if files changed, optionally attaching a label.
    Snap {
        /// Optional human-facing label attached to the snapshot.
        #[arg(value_name = "LABEL", conflicts_with = "message")]
        label: Option<String>,
        /// Optional human-facing label attached to the snapshot. Alias for the positional label.
        #[arg(short, long, value_name = "MESSAGE")]
        message: Option<String>,
        /// Optional author recorded on the snapshot.
        #[arg(long)]
        author: Option<String>,
        /// Shared Era repository to lazily connect this directory to before snapping.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Workspace ID to use with --repo. Defaults to the current directory name.
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Show the current repository state.
    Status {
        /// Shared Era repository to lazily connect this directory to before checking status.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Workspace ID to use with --repo. Defaults to the current directory name.
        #[arg(long)]
        workspace: Option<String>,
    },
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
    /// Restore a snapshot ID, unique prefix, or exact label and move the current cursor to it.
    Restore {
        /// Snapshot ID, unique ID prefix, or exact snapshot label.
        target: String,
        /// Shared Era repository to lazily connect this directory to before restoring.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Workspace ID to use with --repo. Defaults to the current directory name.
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Watch the working directory and create automatic snapshots after edits settle.
    Watch {
        /// Shared Era repository to lazily connect this directory to before watching.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Run one reconciliation pass and exit.
        #[arg(long)]
        once: bool,
        /// Quiet period before saving after a filesystem event.
        #[arg(long, default_value_t = 1000)]
        debounce_ms: u64,
        /// Periodic full reconciliation interval for missed watcher events.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
        reconcile_secs: u64,
        /// Workspace ID used for lazy --repo connection and automatic snapshot provenance.
        #[arg(long)]
        workspace: Option<String>,
        /// Agent ID recorded in automatic snapshot provenance.
        #[arg(long)]
        agent: Option<String>,
        /// Agent task ID recorded in automatic snapshot provenance.
        #[arg(long)]
        task: Option<String>,
        /// Model name recorded in automatic snapshot provenance.
        #[arg(long)]
        model: Option<String>,
    },
    /// Show the indexed snapshot graph with the current cursor marked.
    Timeline {
        /// Shared Era repository to lazily connect this directory to before showing the timeline.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Workspace ID to use with --repo. Defaults to the current directory name.
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Manage connected workspaces.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommands,
    },
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommands {
    /// Ensure a path is connected as a workspace of a shared repository.
    Add {
        /// Directory to create, populate, or adopt as a workspace.
        path: PathBuf,
        /// Shared Era repository. Defaults to the current repository or workspace.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Workspace ID. Defaults to the target directory name.
        #[arg(long)]
        workspace: Option<String>,
        /// Base snapshot, branch, workspace, unique prefix, or exact label. Defaults to current state.
        #[arg(long)]
        from: Option<String>,
    },
    /// List workspaces registered in the shared repository.
    List {
        /// Shared Era repository. Defaults to the current repository or workspace.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Commands::Init => init(cli.verbose).await,
        Commands::Snap {
            label,
            message,
            author,
            repo,
            workspace,
        } => snap(label, message, author, repo, workspace, cli.verbose).await,
        Commands::Status { repo, workspace } => status(repo, workspace, cli.verbose).await,
        Commands::Branch { name } => branch(name, cli.verbose).await,
        Commands::Switch { name } => switch(name, cli.verbose).await,
        Commands::Restore {
            target,
            repo,
            workspace,
        } => restore(target, repo, workspace, cli.verbose).await,
        Commands::Watch {
            repo,
            once,
            debounce_ms,
            reconcile_secs,
            workspace,
            agent,
            task,
            model,
        } => {
            watch(
                WatchArgs {
                    repo,
                    once,
                    debounce_ms,
                    reconcile_secs,
                    metadata: WatchMetadata {
                        workspace,
                        agent,
                        task,
                        model,
                    },
                },
                cli.verbose,
            )
            .await
        }
        Commands::Timeline { repo, workspace } => timeline(repo, workspace, cli.verbose).await,
        Commands::Workspace { command } => workspace(command, cli.verbose).await,
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
    repo: Option<PathBuf>,
    workspace: Option<String>,
    verbose: bool,
) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = open_command_repository(&materializer, repo, workspace).await?;
    let cursor = repository.cursor_info().await?;
    let message = message.or(label);
    let has_label = message.is_some();
    let mut request = match message {
        Some(message) => SnapshotRequest::manual(message),
        None => SnapshotRequest::manual_unlabeled(),
    };
    if let Some(author) = author {
        request = request.with_author(author);
    }
    if let Some(workspace) = repository.workspace_id() {
        request = request.with_provenance_attribute("workspace", workspace.as_str());
    }

    if has_label {
        let result = repository.snapshot(&materializer, request).await?;
        print_snapshot_result("Created snapshot", &result, &cursor, verbose);
        print_capture_warnings(&result);
    } else if let Some(result) = repository
        .snapshot_if_changed(&materializer, request)
        .await?
    {
        print_snapshot_result("Created snapshot", &result, &cursor, verbose);
        print_capture_warnings(&result);
    } else {
        anstream::println!("No changes");
    }

    Ok(())
}

async fn status(
    repo: Option<PathBuf>,
    workspace: Option<String>,
    verbose: bool,
) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = open_command_repository(&materializer, repo, workspace).await?;
    let cursor = repository.cursor_info().await?;
    let timeline = repository.timeline().await?;
    if timeline.is_empty() {
        return Err(CliError::EmptyTimeline);
    }
    let status = repository.working_tree_status(&materializer).await?;
    let cursor_ref = repository.current_cursor_ref_path().await?;

    print_status(
        &repository,
        &cursor,
        &status,
        timeline.len(),
        verbose,
        &cursor_ref,
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

async fn restore(
    target: String,
    repo: Option<PathBuf>,
    workspace: Option<String>,
    verbose: bool,
) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = open_command_repository(&materializer, repo, workspace).await?;
    let result = repository.restore(&materializer, &target).await?;

    print_restore_result(&result, verbose);
    print_optional_saved_warnings(result.saved_snapshot.as_ref());
    Ok(())
}

#[derive(Debug)]
struct WatchArgs {
    repo: Option<PathBuf>,
    once: bool,
    debounce_ms: u64,
    reconcile_secs: u64,
    metadata: WatchMetadata,
}

#[derive(Debug)]
struct WatchMetadata {
    workspace: Option<String>,
    agent: Option<String>,
    task: Option<String>,
    model: Option<String>,
}

async fn watch(args: WatchArgs, verbose: bool) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository =
        open_command_repository(&materializer, args.repo, args.metadata.workspace.clone()).await?;

    if args.once {
        let saved = snapshot_if_changed_for_watch(
            &repository,
            &materializer,
            AutoSnapshotTrigger::Reconcile,
            &args.metadata,
        )
        .await?;
        print_watch_snapshot_result(saved.as_ref(), verbose, true);
        return Ok(());
    }

    let working_directory = WorkingDirectory::new(repository.root());
    let mut watch = materializer.watch(&working_directory).await?;
    let debounce = Duration::from_millis(args.debounce_ms);
    let mut debounce_sleep: Option<Pin<Box<Sleep>>> = None;
    let mut reconcile = time::interval(Duration::from_secs(args.reconcile_secs));
    reconcile.set_missed_tick_behavior(MissedTickBehavior::Skip);
    reconcile.tick().await;

    anstream::println!("Watching for changes");

    loop {
        tokio::select! {
            event = watch.next_event() => {
                match event {
                    Some(Ok(event)) => {
                        invalidate_watch_event_paths(&materializer, &event);
                        debounce_sleep = Some(Box::pin(time::sleep(debounce)));
                    }
                    Some(Err(error)) => return Err(CliError::from(error)),
                    None => return Err(CliError::WatchStopped),
                }
            }
            _ = async {
                if let Some(sleep) = debounce_sleep.as_mut() {
                    sleep.as_mut().await;
                }
            }, if debounce_sleep.is_some() => {
                debounce_sleep = None;
                let saved = snapshot_if_changed_for_watch(
                    &repository,
                    &materializer,
                    AutoSnapshotTrigger::Watch,
                    &args.metadata,
                )
                .await?;
                print_watch_snapshot_result(saved.as_ref(), verbose, false);
            }
            _ = reconcile.tick() => {
                let saved = snapshot_if_changed_for_watch(
                    &repository,
                    &materializer,
                    AutoSnapshotTrigger::Reconcile,
                    &args.metadata,
                )
                .await?;
                print_watch_snapshot_result(saved.as_ref(), verbose, false);
            }
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|source| CliError::Signal { source })?;
                anstream::println!("Stopped");
                break;
            }
        }
    }

    Ok(())
}

async fn snapshot_if_changed_for_watch(
    repository: &Repository,
    materializer: &FilesystemMaterializer,
    trigger: AutoSnapshotTrigger,
    metadata: &WatchMetadata,
) -> Result<Option<SnapshotResult>, CliError> {
    repository
        .snapshot_if_changed(
            materializer,
            auto_snapshot_request(trigger, metadata, repository),
        )
        .await
        .map_err(CliError::from)
}

fn auto_snapshot_request(
    trigger: AutoSnapshotTrigger,
    metadata: &WatchMetadata,
    repository: &Repository,
) -> SnapshotRequest {
    let workspace = metadata
        .workspace
        .clone()
        .or_else(|| repository.workspace_id().map(|id| id.as_str().to_owned()))
        .unwrap_or_else(|| era_repository::DEFAULT_WORKSPACE_ID.to_owned());
    let mut request = SnapshotRequest::automatic_for_trigger(trigger)
        .with_provenance_attribute("workspace", workspace);

    if let Some(agent) = &metadata.agent {
        request = request.with_provenance_attribute("agent", agent.clone());
    }
    if let Some(task) = &metadata.task {
        request = request.with_provenance_attribute("task", task.clone());
    }
    if let Some(model) = &metadata.model {
        request = request.with_provenance_attribute("model", model.clone());
    }

    request
}

fn invalidate_watch_event_paths(materializer: &FilesystemMaterializer, event: &WatchEvent) {
    materializer.invalidate_paths(event.paths.iter());
}

async fn timeline(
    repo: Option<PathBuf>,
    workspace: Option<String>,
    verbose: bool,
) -> Result<(), CliError> {
    let materializer = FilesystemMaterializer::new();
    let repository = open_command_repository(&materializer, repo, workspace).await?;
    let cursor = repository.cursor_info().await?;
    let graph = load_printable_timeline_graph(&repository, &materializer).await?;

    print_timeline_graph(&cursor, &graph, verbose);
    Ok(())
}

#[derive(Debug, Clone)]
struct PrintableTimelineGraph {
    graph: SnapshotGraph,
    current_snapshot_id: ObjectId,
    worktree_root_tree_id: ObjectId,
    worktree_matches: BTreeSet<ObjectId>,
    worktree_clean: bool,
}

async fn load_printable_timeline_graph(
    repository: &Repository,
    materializer: &FilesystemMaterializer,
) -> Result<PrintableTimelineGraph, CliError> {
    let graph = repository.snapshot_graph().await?;
    let current_snapshot_id = repository.current_snapshot_id().await?;
    let status = repository.working_tree_status(materializer).await?;
    let worktree_root_tree_id = status.current_root_tree_id;
    let worktree_matches = graph
        .entries
        .iter()
        .filter(|entry| entry.snapshot.root_tree_id() == worktree_root_tree_id)
        .map(|entry| entry.snapshot_id)
        .collect();

    Ok(PrintableTimelineGraph {
        graph,
        current_snapshot_id,
        worktree_root_tree_id,
        worktree_matches,
        worktree_clean: status.is_clean(),
    })
}

async fn workspace(command: WorkspaceCommands, verbose: bool) -> Result<(), CliError> {
    match command {
        WorkspaceCommands::Add {
            path,
            repo,
            workspace,
            from,
        } => {
            let materializer = FilesystemMaterializer::new();
            let source = match repo {
                Some(repo) => Repository::open_repository_path(repo).await?,
                None => Repository::open(current_directory()?).await?,
            };
            let workspace_id = infer_workspace_id(&path, workspace)?;
            let result = source
                .add_workspace(
                    &materializer,
                    AddWorkspaceOptions {
                        path,
                        workspace_id,
                        from,
                    },
                )
                .await?;
            print_workspace_add_result(&result, verbose);
        }
        WorkspaceCommands::List { repo } => {
            let repository = match repo {
                Some(repo) => Repository::open_repository_path(repo).await?,
                None => Repository::open(current_directory()?).await?,
            };
            let workspaces = repository.workspaces().await?;
            print_workspaces(&workspaces, verbose);
        }
    }

    Ok(())
}

async fn open_command_repository(
    materializer: &FilesystemMaterializer,
    repo: Option<PathBuf>,
    workspace: Option<String>,
) -> Result<Repository, CliError> {
    match repo {
        Some(repo) => {
            let current_dir = current_directory()?;
            let workspace_id = infer_workspace_id(&current_dir, workspace)?;
            Repository::open_or_add_workspace(repo, current_dir, workspace_id, materializer)
                .await
                .map_err(CliError::from)
        }
        None => Repository::open(current_directory()?)
            .await
            .map_err(CliError::from),
    }
}

fn infer_workspace_id(
    path: &std::path::Path,
    workspace: Option<String>,
) -> Result<WorkspaceId, CliError> {
    if let Some(workspace) = workspace {
        return WorkspaceId::new(workspace)
            .map_err(RepositoryError::from)
            .map_err(CliError::from);
    }

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .or_else(|| {
            std::env::current_dir().ok().and_then(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
        })
        .unwrap_or_else(|| era_repository::DEFAULT_WORKSPACE_ID.to_owned());
    WorkspaceId::new(name)
        .map_err(RepositoryError::from)
        .map_err(CliError::from)
}

fn current_directory() -> Result<PathBuf, CliError> {
    std::env::current_dir().map_err(|source| CliError::CurrentDirectory { source })
}

fn print_init_result(result: &era_repository::InitResult, branch: &BranchName, verbose: bool) {
    let metadata = result.repository.metadata_dir().display();
    let style = success_style();
    anstream::println!("{style}Initialized Era repository in {metadata}{style:#}");

    if verbose {
        anstream::println!();
        let cursor = CursorInfo::Branch(branch.clone());
        print_snapshot_details(&result.snapshot, &cursor);
    }
}

fn print_snapshot_result(
    heading: &str,
    result: &SnapshotResult,
    cursor: &CursorInfo,
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
        print_snapshot_details(result, cursor);
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

fn print_snapshot_details(result: &SnapshotResult, cursor: &CursorInfo) {
    print_section("Details");
    print_detail("Full snapshot", result.snapshot_id);
    print_detail("Root tree", result.snapshot.root_tree_id());
    print_detail(cursor.kind(), cursor.name());
    print_detail(
        "Timestamp",
        format!("{} ms", result.snapshot.timestamp_millis()),
    );
    print_detail("Parents", result.snapshot.parents().len());
    print_detail("Source", result.snapshot.provenance().source());
    print_provenance_attributes(&result.snapshot);

    if let Some(author) = result.snapshot.author() {
        print_detail("Author", author);
    }

    print_capture_stats(&result.capture.stats);
}

fn print_capture_stats(stats: &CaptureStats) {
    print_detail("Files", stats.files_seen);
    print_detail("Directories", stats.directories_seen);
    print_detail("Bytes", stats.bytes_read);
    print_detail("Blobs stored", stats.blobs_stored);
    print_detail("Trees stored", stats.trees_stored);
    print_detail("Ignored", stats.ignored_entries);
    print_detail("Symlinks", stats.symlinks_skipped);
    print_detail("Cache hits", stats.hash_cache_hits);
    print_detail("Cache misses", stats.hash_cache_misses);
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
    cursor: &CursorInfo,
    status: &WorkingTreeStatus,
    timeline_len: usize,
    verbose: bool,
    cursor_ref: &std::path::Path,
) {
    print_success_heading("Repository status");
    let snapshot = &status.snapshot;
    print_field(cursor.kind(), cursor.name());
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
        let ref_label = match cursor {
            CursorInfo::Branch(_) => "Branch ref",
            CursorInfo::Workspace(_) => "Cursor ref",
        };
        print_detail(ref_label, cursor_ref.display());
        print_detail("Full snapshot", status.snapshot_id);
        print_detail("Root tree", snapshot.root_tree_id());
        print_detail("Current tree", status.current_root_tree_id);
        print_detail("Timestamp", format!("{} ms", snapshot.timestamp_millis()));
        print_detail("Parents", snapshot.parents().len());
        print_detail("Source", snapshot.provenance().source());
        print_provenance_attributes(snapshot);

        if let Some(author) = snapshot.author() {
            print_detail("Author", author);
        }

        let stats = &status.comparison.stats;
        print_detail("Files", stats.files_seen);
        print_detail("Directories", stats.directories_seen);
        print_detail("Bytes", stats.bytes_read);
        print_detail("Ignored", stats.ignored_entries);
        print_detail("Symlinks", stats.symlinks_skipped);
        print_detail("Cache hits", stats.hash_cache_hits);
        print_detail("Cache misses", stats.hash_cache_misses);
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

fn print_workspace_add_result(result: &WorkspaceAddResult, verbose: bool) {
    let heading = if result.created {
        "Added workspace"
    } else {
        "Workspace already connected"
    };
    print_success_heading(heading);
    print_field("Workspace", &result.workspace_id);
    print_field("Path", result.path.display());
    print_field(
        "Snapshot",
        styled(accent_style(), short_id(result.snapshot_id)),
    );
    print_field(
        "Files",
        if result.materialized {
            "materialized"
        } else {
            "adopted"
        },
    );

    if verbose && let Some(materialization) = &result.materialization {
        print_materialize_details(materialization);
    }
}

fn print_workspaces(workspaces: &[WorkspaceHead], verbose: bool) {
    anstream::println!("Workspaces");
    for workspace in workspaces {
        let marker = if workspace.is_current { "*" } else { " " };
        let name = if workspace.is_current {
            styled(accent_style(), &workspace.id)
        } else {
            workspace.id.to_string()
        };
        let path = workspace
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_owned());
        anstream::println!(
            "{marker} {name} {} {path}",
            styled(accent_style(), short_id(workspace.snapshot_id))
        );

        if verbose {
            print_indented_detail("Full snapshot", workspace.snapshot_id);
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
    print_field("Cursor", cursor_summary(&result.cursor, result.snapshot_id));

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
        print_detail("Source", saved.snapshot.provenance().source());
        print_provenance_attributes(&saved.snapshot);
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

fn print_watch_snapshot_result(
    result: Option<&SnapshotResult>,
    verbose: bool,
    print_no_changes: bool,
) {
    match result {
        Some(result) => {
            anstream::println!(
                "Saved auto snapshot {}",
                styled(accent_style(), short_id(result.snapshot_id))
            );
            if verbose {
                anstream::println!();
                print_section("Details");
                print_detail("Full snapshot", result.snapshot_id);
                print_detail("Root tree", result.snapshot.root_tree_id());
                print_detail(
                    "Timestamp",
                    format!("{} ms", result.snapshot.timestamp_millis()),
                );
                print_detail("Parents", result.snapshot.parents().len());
                print_detail("Source", result.snapshot.provenance().source());
                print_provenance_attributes(&result.snapshot);
                print_capture_stats(&result.capture.stats);
            }
            print_capture_warnings(result);
        }
        None if print_no_changes || verbose => anstream::println!("No changes"),
        None => {}
    }
}

fn print_timeline_graph(cursor: &CursorInfo, graph: &PrintableTimelineGraph, verbose: bool) {
    anstream::println!("Snapshot tree");
    print_field("Cursor", cursor_summary(cursor, graph.current_snapshot_id));
    print_field("Worktree", worktree_summary(graph));
    print_field(
        "Snapshots",
        pluralize(graph.graph.entries.len(), "snapshot", "snapshots"),
    );
    anstream::println!();

    let renderer = TimelineGraphRenderer::new(graph);
    renderer.print(verbose);
}

fn cursor_summary(cursor: &CursorInfo, snapshot_id: ObjectId) -> String {
    let kind = match cursor {
        CursorInfo::Branch(_) => "branch",
        CursorInfo::Workspace(_) => "workspace",
    };
    format!("{kind} {} @ {}", cursor.name(), short_id(snapshot_id))
}

fn worktree_summary(graph: &PrintableTimelineGraph) -> String {
    let mut other_matches: Vec<_> = graph
        .worktree_matches
        .iter()
        .copied()
        .filter(|snapshot_id| *snapshot_id != graph.current_snapshot_id)
        .map(short_id)
        .collect();
    other_matches.sort();

    if graph.worktree_clean {
        if other_matches.is_empty() {
            "clean at cursor".to_owned()
        } else {
            format!(
                "clean at cursor; same tree also at {}",
                other_matches.join(", ")
            )
        }
    } else if graph.worktree_matches.is_empty() {
        format!(
            "dirty; tree {} is not a saved snapshot",
            short_id(graph.worktree_root_tree_id)
        )
    } else {
        format!("matches {}", other_matches.join(", "))
    }
}

struct TimelineGraphRenderer<'a> {
    graph: &'a PrintableTimelineGraph,
    entries: BTreeMap<ObjectId, &'a TimelineEntry>,
    children: BTreeMap<ObjectId, Vec<ObjectId>>,
    roots: Vec<ObjectId>,
    ref_labels: BTreeMap<ObjectId, Vec<String>>,
}

impl<'a> TimelineGraphRenderer<'a> {
    fn new(graph: &'a PrintableTimelineGraph) -> Self {
        let entries: BTreeMap<_, _> = graph
            .graph
            .entries
            .iter()
            .map(|entry| (entry.snapshot_id, entry))
            .collect();
        let mut children: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
        let mut has_parent = BTreeSet::new();

        for entry in &graph.graph.entries {
            if let Some(parent) = entry.snapshot.parents().first()
                && entries.contains_key(parent)
            {
                children.entry(*parent).or_default().push(entry.snapshot_id);
                has_parent.insert(entry.snapshot_id);
            }
        }

        for children in children.values_mut() {
            children.sort_by_key(|id| {
                let entry = entries.get(id).expect("child entry should exist");
                (entry.snapshot.timestamp_millis(), *id)
            });
        }

        let mut roots: Vec<_> = entries
            .keys()
            .copied()
            .filter(|snapshot_id| !has_parent.contains(snapshot_id))
            .collect();
        roots.sort_by_key(|id| {
            let entry = entries.get(id).expect("root entry should exist");
            (entry.snapshot.timestamp_millis(), *id)
        });

        let mut ref_labels: BTreeMap<ObjectId, Vec<String>> = BTreeMap::new();
        for branch in &graph.graph.branches {
            ref_labels
                .entry(branch.snapshot_id)
                .or_default()
                .push(branch.name.to_string());
        }
        for workspace in &graph.graph.workspaces {
            ref_labels
                .entry(workspace.snapshot_id)
                .or_default()
                .push(workspace.id.to_string());
        }
        for labels in ref_labels.values_mut() {
            labels.sort();
        }

        Self {
            graph,
            entries,
            children,
            roots,
            ref_labels,
        }
    }

    fn print(&self, verbose: bool) {
        if self.roots.is_empty() {
            anstream::println!("(no snapshots)");
            return;
        }

        for (index, root) in self.roots.iter().enumerate() {
            let is_last = index + 1 == self.roots.len();
            self.print_node_or_auto_chain(*root, "", "", is_last, verbose);
        }
    }

    fn print_node_or_auto_chain(
        &self,
        snapshot_id: ObjectId,
        prefix: &str,
        connector: &str,
        is_last: bool,
        verbose: bool,
    ) {
        let chain = self.auto_chain(snapshot_id);
        if chain.len() >= MIN_COLLAPSED_AUTO_SNAPSHOTS {
            self.print_collapsed_auto_chain(&chain, prefix, connector);
            if let Some(next) = self.only_child(*chain.last().expect("chain is not empty")) {
                let child_prefix = child_prefix(prefix, is_last, connector.is_empty());
                self.print_node_or_auto_chain(next, &child_prefix, "└─", true, verbose);
            }
            return;
        }

        self.print_node(snapshot_id, prefix, connector, verbose);
        let child_prefix = child_prefix(prefix, is_last, connector.is_empty());
        if let Some(children) = self.children.get(&snapshot_id) {
            for (index, child) in children.iter().enumerate() {
                self.print_node_or_auto_chain(
                    *child,
                    &child_prefix,
                    if index + 1 == children.len() {
                        "└─"
                    } else {
                        "├─"
                    },
                    index + 1 == children.len(),
                    verbose,
                );
            }
        }
    }

    fn print_node(&self, snapshot_id: ObjectId, prefix: &str, connector: &str, verbose: bool) {
        let entry = self
            .entries
            .get(&snapshot_id)
            .expect("timeline entry should exist");
        let marker = styled(timeline_style(), self.marker(snapshot_id));
        let id = styled(accent_style(), short_id(snapshot_id));
        let annotations = self.annotations(snapshot_id, &entry.snapshot);
        let suffix = if annotations.is_empty() {
            String::new()
        } else {
            format!("  {}", annotations.join(", "))
        };
        anstream::println!(
            "{prefix}{connector}{marker} {id}  {}{suffix}",
            timeline_title(&entry.snapshot)
        );

        if verbose {
            print_timeline_details(snapshot_id, &entry.snapshot);
        }
    }

    fn print_collapsed_auto_chain(&self, chain: &[ObjectId], prefix: &str, connector: &str) {
        let first = self
            .entries
            .get(&chain[0])
            .expect("chain entry should exist");
        let last = self
            .entries
            .get(chain.last().expect("chain is not empty"))
            .expect("chain entry should exist");
        let marker = styled(timeline_style(), "…");
        anstream::println!(
            "{prefix}{connector}{marker} {} auto snapshots · {}–{}",
            chain.len(),
            format_snapshot_time(first.snapshot.timestamp_millis()),
            format_snapshot_time(last.snapshot.timestamp_millis())
        );
    }

    fn marker(&self, snapshot_id: ObjectId) -> &'static str {
        let is_cursor = snapshot_id == self.graph.current_snapshot_id;
        let is_worktree = self.graph.worktree_matches.contains(&snapshot_id);
        match (is_cursor, is_worktree) {
            (true, true) => "@",
            (true, false) => "@",
            (false, true) => "◎",
            (false, false) => "●",
        }
    }

    fn annotations(&self, snapshot_id: ObjectId, snapshot: &Snapshot) -> Vec<String> {
        let mut annotations = self
            .ref_labels
            .get(&snapshot_id)
            .cloned()
            .unwrap_or_default();
        if snapshot_id == self.graph.current_snapshot_id {
            annotations.push("current".to_owned());
        }
        if self.graph.worktree_matches.contains(&snapshot_id) {
            annotations.push("worktree".to_owned());
        }
        if snapshot.parents().len() > 1 {
            annotations.push(format!("{} parents", snapshot.parents().len()));
        }
        annotations
    }

    fn auto_chain(&self, start: ObjectId) -> Vec<ObjectId> {
        let mut chain = Vec::new();
        let mut current = start;

        while let Some(entry) = self.entries.get(&current) {
            if !self.is_collapsible_auto_snapshot(current, &entry.snapshot) {
                break;
            }

            chain.push(current);
            let Some(next) = self.only_child(current) else {
                break;
            };
            current = next;
        }

        chain
    }

    fn is_collapsible_auto_snapshot(&self, snapshot_id: ObjectId, snapshot: &Snapshot) -> bool {
        snapshot.message().is_none()
            && snapshot.provenance().source() == "auto-snapshot"
            && snapshot_id != self.graph.current_snapshot_id
            && !self.graph.worktree_matches.contains(&snapshot_id)
            && !self.ref_labels.contains_key(&snapshot_id)
            && snapshot.parents().len() <= 1
            && self.children.get(&snapshot_id).map_or(0, Vec::len) <= 1
    }

    fn only_child(&self, snapshot_id: ObjectId) -> Option<ObjectId> {
        let children = self.children.get(&snapshot_id)?;
        (children.len() == 1).then_some(children[0])
    }
}

fn child_prefix(prefix: &str, is_last: bool, is_root: bool) -> String {
    if is_root {
        String::new()
    } else if is_last {
        format!("{prefix}  ")
    } else {
        format!("{prefix}│ ")
    }
}

fn print_timeline_details(snapshot_id: ObjectId, snapshot: &Snapshot) {
    print_indented_detail("Full snapshot", snapshot_id);
    print_indented_detail("Root tree", snapshot.root_tree_id());
    print_indented_detail("Timestamp", format!("{} ms", snapshot.timestamp_millis()));
    print_indented_detail("Parents", snapshot.parents().len());
    print_indented_detail("Source", snapshot.provenance().source());
    print_indented_provenance_attributes(snapshot);

    if let Some(author) = snapshot.author() {
        print_indented_detail("Author", author);
    }
}

fn print_provenance_attributes(snapshot: &Snapshot) {
    for (key, value) in snapshot.provenance().attributes() {
        print_detail(key, value);
    }
}

fn print_indented_provenance_attributes(snapshot: &Snapshot) {
    for (key, value) in snapshot.provenance().attributes() {
        print_indented_detail(key, value);
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
        _ if snapshot.provenance().source() == "auto-snapshot" => {
            format!(
                "auto snapshot · {}",
                format_snapshot_time(snapshot.timestamp_millis())
            )
        }
        _ if snapshot.provenance().source() == "manual-snapshot" => {
            format!(
                "snapshot · {}",
                format_snapshot_time(snapshot.timestamp_millis())
            )
        }
        _ => snapshot.provenance().source().to_owned(),
    }
}

fn format_snapshot_time(timestamp_millis: u64) -> String {
    use chrono::TimeZone as _;

    i64::try_from(timestamp_millis)
        .ok()
        .and_then(|millis| Local.timestamp_millis_opt(millis).single())
        .map(|timestamp| timestamp.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| format!("{timestamp_millis} ms"))
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
    WatchStopped,
    Signal { source: std::io::Error },
    Repository { source: Box<RepositoryError> },
    Materialization { source: Box<MaterializationError> },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDirectory { source } => {
                write!(formatter, "could not determine current directory: {source}")
            }
            Self::EmptyTimeline => write!(formatter, "repository timeline is empty"),
            Self::WatchStopped => write!(formatter, "filesystem watcher stopped"),
            Self::Signal { source } => write!(formatter, "could not listen for Ctrl-C: {source}"),
            Self::Repository { source } => write!(formatter, "{source}"),
            Self::Materialization { source } => write!(formatter, "{source}"),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentDirectory { source } => Some(source),
            Self::EmptyTimeline | Self::WatchStopped => None,
            Self::Signal { source } => Some(source),
            Self::Repository { source } => Some(source.as_ref()),
            Self::Materialization { source } => Some(source.as_ref()),
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

impl From<MaterializationError> for CliError {
    fn from(source: MaterializationError) -> Self {
        Self::Materialization {
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
