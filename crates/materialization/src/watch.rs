use crate::{CaptureOptions, MaterializationError, WorkingDirectory};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// A filesystem change hint emitted by a materializer watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    /// Changed paths relative to the working-directory root.
    pub paths: Vec<PathBuf>,
}

impl WatchEvent {
    /// Creates a watch event from relative paths.
    #[must_use]
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

/// A live filesystem watcher for a working directory.
#[derive(Debug)]
pub struct WorkingDirectoryWatch {
    root: PathBuf,
    receiver: mpsc::UnboundedReceiver<Result<WatchEvent, MaterializationError>>,
    _watcher: RecommendedWatcher,
}

impl WorkingDirectoryWatch {
    pub(crate) fn new(
        working_directory: &WorkingDirectory,
        options: CaptureOptions,
    ) -> Result<Self, MaterializationError> {
        let root = working_directory.root().to_path_buf();
        let callback_root = root.clone();
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => {
                    let paths = relative_included_paths(&callback_root, &options, event.paths);
                    if !paths.is_empty() {
                        let _ = sender.send(Ok(WatchEvent::new(paths)));
                    }
                }
                Err(source) => {
                    let _ = sender.send(Err(MaterializationError::Watch {
                        path: callback_root.clone(),
                        source,
                    }));
                }
            },
            Config::default(),
        )
        .map_err(|source| MaterializationError::Watch {
            path: root.clone(),
            source,
        })?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|source| MaterializationError::Watch {
                path: root.clone(),
                source,
            })?;

        Ok(Self {
            root,
            receiver,
            _watcher: watcher,
        })
    }

    /// Returns the watched working-directory root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Waits for the next filesystem event hint.
    pub async fn next_event(&mut self) -> Option<Result<WatchEvent, MaterializationError>> {
        self.receiver.recv().await
    }
}

fn relative_included_paths(
    root: &Path,
    options: &CaptureOptions,
    paths: Vec<PathBuf>,
) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_path_buf))
        .filter(|path| !options.excludes_path(path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_included_paths_filters_excluded_directories() {
        let root = Path::new("/repo");
        let paths = vec![
            PathBuf::from("/repo/src/main.rs"),
            PathBuf::from("/repo/.era/HEAD"),
            PathBuf::from("/repo/target/debug/app"),
        ];

        assert_eq!(
            relative_included_paths(root, &CaptureOptions::default(), paths),
            vec![PathBuf::from("src/main.rs")]
        );
    }
}
