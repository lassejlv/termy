use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{LazyLock, Mutex},
};

#[cfg(not(test))]
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tempfile::NamedTempFile;

use super::{ConfigIoError, DEFAULT_CONFIG};

static CONFIG_CHANGE_SUBSCRIBERS: LazyLock<Mutex<Vec<flume::Sender<()>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

#[cfg(not(test))]
static CONFIG_FILE_WATCHER: LazyLock<Mutex<Option<RecommendedWatcher>>> =
    LazyLock::new(|| Mutex::new(None));

pub(crate) fn notify_config_changed() {
    let Ok(mut subscribers) = CONFIG_CHANGE_SUBSCRIBERS.lock() else {
        return;
    };
    #[cfg(test)]
    subscribers.retain(test_subscriber_is_alive);

    #[cfg(not(test))]
    subscribers.retain(|tx| match tx.try_send(()) {
        Ok(()) | Err(flume::TrySendError::Full(())) => true,
        Err(flume::TrySendError::Disconnected(())) => false,
    });
}

#[cfg(test)]
fn test_subscriber_is_alive(tx: &flume::Sender<()>) -> bool {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    // Tests can leave behind receivers tied to torn-down async runtimes. Treat
    // panicking wakeups the same as disconnected subscribers and prune them.
    catch_unwind(AssertUnwindSafe(|| tx.try_send(())))
        .ok()
        .is_some_and(|result| !matches!(result, Err(flume::TrySendError::Disconnected(()))))
}

pub fn subscribe_config_changes() -> flume::Receiver<()> {
    // Config writes can emit several filesystem events. One pending signal per
    // subscriber is enough because every consumer reloads the latest file.
    let (tx, rx) = flume::bounded(1);
    if let Ok(mut subscribers) = CONFIG_CHANGE_SUBSCRIBERS.lock() {
        subscribers.push(tx);
    }
    #[cfg(not(test))]
    ensure_config_file_watcher_started();
    rx
}

#[cfg(not(test))]
fn ensure_config_file_watcher_started() {
    let Ok(mut watcher_slot) = CONFIG_FILE_WATCHER.lock() else {
        return;
    };
    if watcher_slot.is_some() {
        return;
    }

    let Some(config_path) = termy_core::config_core::config_path() else {
        return;
    };
    let Some(config_dir) = config_path.parent() else {
        return;
    };
    let watched_file_name = config_path.file_name().map(ToOwned::to_owned);
    let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| match event {
        Ok(event)
            if !event.kind.is_access()
                && (event.paths.is_empty()
                    || event.paths.iter().any(|path| {
                        path.file_name()
                            .is_some_and(|name| Some(name) == watched_file_name.as_deref())
                    })) =>
        {
            notify_config_changed();
        }
        Ok(_) => {}
        Err(error) => log::warn!("Config file watcher warning: {error}"),
    });
    let mut watcher = match watcher {
        Ok(watcher) => watcher,
        Err(error) => {
            log::warn!("Failed to create config file watcher: {error}");
            return;
        }
    };
    if let Err(error) = watcher.watch(config_dir, RecursiveMode::NonRecursive) {
        log::warn!(
            "Failed to watch config directory {}: {error}",
            config_dir.display()
        );
        return;
    }
    *watcher_slot = Some(watcher);
}

pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), ConfigIoError> {
    let parent = path
        .parent()
        .ok_or_else(|| ConfigIoError::InvalidConfigPath(path.to_path_buf()))?;
    let mut temp =
        NamedTempFile::new_in(parent).map_err(|source| ConfigIoError::CreateTempFile {
            path: path.to_path_buf(),
            source,
        })?;

    temp.write_all(contents.as_bytes())
        .map_err(|source| ConfigIoError::WriteConfig {
            path: path.to_path_buf(),
            source,
        })?;
    temp.flush().map_err(|source| ConfigIoError::WriteConfig {
        path: path.to_path_buf(),
        source,
    })?;
    temp.as_file()
        .sync_all()
        .map_err(|source| ConfigIoError::WriteConfig {
            path: path.to_path_buf(),
            source,
        })?;
    temp.persist(path)
        .map_err(|error| ConfigIoError::PersistTempFile {
            path: path.to_path_buf(),
            source: error.error,
        })?;

    Ok(())
}

pub fn ensure_config_file() -> Result<PathBuf, ConfigIoError> {
    let path =
        termy_core::config_core::config_path().ok_or(ConfigIoError::ConfigPathUnavailable)?;
    if !path.exists() {
        let parent = path
            .parent()
            .ok_or_else(|| ConfigIoError::InvalidConfigPath(path.clone()))?;
        fs::create_dir_all(parent).map_err(|source| ConfigIoError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
        write_atomic(&path, DEFAULT_CONFIG)?;
    }
    #[cfg(not(test))]
    ensure_config_file_watcher_started();
    Ok(path)
}

pub fn open_config_file() -> Result<(), ConfigIoError> {
    let path = ensure_config_file()?;

    #[cfg(target_os = "macos")]
    {
        run_open_command("open", &[], &path)?;
    }

    #[cfg(target_os = "linux")]
    {
        run_open_command("xdg-open", &[], &path)?;
    }

    #[cfg(target_os = "windows")]
    {
        run_open_command("cmd", &["/C", "start", ""], &path)?;
    }

    Ok(())
}

fn run_open_command(
    command: &'static str,
    args: &[&str],
    path: &Path,
) -> Result<(), ConfigIoError> {
    let path = path.to_path_buf();
    let status = Command::new(command)
        .args(args)
        .arg(&path)
        .status()
        .map_err(|source| ConfigIoError::LaunchOpenCommand {
            command,
            path: path.clone(),
            source,
        })?;
    if !status.success() {
        return Err(ConfigIoError::OpenCommandFailed {
            command,
            path,
            status,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{notify_config_changed, subscribe_config_changes, write_atomic};

    #[test]
    fn config_change_signals_coalesce_until_consumed() {
        let changes = subscribe_config_changes();

        notify_config_changed();
        notify_config_changed();
        assert!(changes.try_recv().is_ok());
        assert!(changes.try_recv().is_err());

        notify_config_changed();
        assert!(changes.try_recv().is_ok());
    }

    #[test]
    fn write_atomic_replaces_file_without_extra_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.txt");

        write_atomic(&path, "theme = termy\n").expect("write initial");
        write_atomic(&path, "theme = nord\n").expect("write replacement");

        let contents = std::fs::read_to_string(&path).expect("read config");
        assert_eq!(contents, "theme = nord\n");

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(entries, vec!["config.txt".to_string()]);
    }
}
