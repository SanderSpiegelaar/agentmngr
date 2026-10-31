use std::path::Path;

use tracing_appender::{non_blocking::WorkerGuard, rolling::Rotation};
use tracing_subscriber::util::SubscriberInitExt;

use crate::Error;

/// Installs the process-wide logger. Keep the guard alive until shutdown to flush logs.
/// Pass `$SKILLSMGR_HOME/logs`; home discovery belongs to the caller.
pub fn init_logging(logs_dir: &Path) -> Result<WorkerGuard, Error> {
    let appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("skillsmgr.log")
        .max_log_files(7)
        .build(logs_dir)
        .map_err(|error| Error::Logging(error.to_string()))?;
    let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .finish(appender);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(writer)
        .finish()
        .try_init()
        .map_err(|error| Error::Logging(error.to_string()))?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::init_logging;

    #[test]
    fn creates_and_flushes_daily_log_under_home_logs() {
        let home = tempfile::tempdir().unwrap();
        let logs = home.path().join("logs");
        let guard = init_logging(&logs).unwrap();
        tracing::info!("T-004 log file proof");
        drop(guard);

        let files: Vec<_> = std::fs::read_dir(&logs)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("skillsmgr.log."));
        assert_eq!(name.len(), "skillsmgr.log.YYYY-MM-DD".len());
        let contents = std::fs::read_to_string(&files[0]).unwrap();
        assert!(contents.contains("T-004 log file proof"));
        assert!(!contents.contains('\u{1b}'));
        println!("created {} with flushed INFO message", files[0].display());

        let error = init_logging(&logs).unwrap_err();
        assert_eq!(error.code(), "E_LOGGING");
    }

    #[test]
    fn unwritable_log_directory_returns_error() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let error = init_logging(&file.path().join("logs")).unwrap_err();
        assert_eq!(error.code(), "E_LOGGING");
    }
}
