use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

/// Starts logging once per process, which Android can run `android_main` in
/// twice. The guard flushes the log file when dropped; Android keeps it for
/// the process instead, so a later run still writes to that file.
pub fn init(log_dir: &Path) -> Option<WorkerGuard> {
    if tracing::dispatcher::has_been_set() {
        return None;
    }
    if let Err(e) = rotate(log_dir) {
        eprintln!("Failed to rotate logs: {e}. latest.log will probably be overwritten.");
    }
    let _ = LOG_FILE.set(log_dir.join(LATEST_LOG));
    let file_appender = tracing_appender::rolling::never(log_dir, LATEST_LOG);
    // The default queues 128k lines up front (~4 MB); a file write keeps up
    // with far fewer, and a full queue drops lines rather than blocking.
    let (non_blocking, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(16_384)
        .finish(file_appender);

    let file_filter = EnvFilter::new("debug");
    let stdout_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(non_blocking)
                .with_filter(file_filter),
        )
        .with(console_layer().with_filter(stdout_filter))
        .init();

    if cfg!(target_os = "android") {
        std::mem::forget(guard);
        return None;
    }
    Some(guard)
}

const LATEST_LOG: &str = "latest.log";
/// The log file, for the panic hook.
static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Panics to the log, written on the spot: the abort that follows one
/// (`app::state_slot`) never lets the log's writer thread get to it, and a
/// release build has no console for the default hook's message. Android
/// drops stderr, so one before logging starts goes straight to logcat.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = LOG_FILE.get();
        if let Some(path) = path {
            let line = format!(
                "{} PANIC {info}
",
                chrono::Utc::now().to_rfc3339()
            );
            let _ = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
        }
        // Written to the file above, the default hook prints it on desktop;
        // Android drops stderr, so logcat still needs it.
        if tracing::dispatcher::has_been_set() && (path.is_none() || cfg!(target_os = "android")) {
            tracing::error!("{info}");
        }
        #[cfg(target_os = "android")]
        if !tracing::dispatcher::has_been_set() {
            let logcat = tracing_subscriber::registry().with(console_layer());
            tracing::subscriber::with_default(logcat, || tracing::error!("{info}"));
        }
        default_hook(info);
    }));
}

/// Stdout, or logcat on Android, which drops stdout.
fn console_layer<S>() -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    #[cfg(target_os = "android")]
    return paranoid_android::layer("pomme").with_ansi(false);
    #[cfg(not(target_os = "android"))]
    fmt::layer().with_writer(std::io::stdout)
}

const MAX_LOG_FILES: usize = 5;

fn rotate(log_dir: &Path) -> std::io::Result<()> {
    let latest = log_dir.join(LATEST_LOG);
    if !latest.exists() {
        return Ok(());
    }
    let modified = latest.metadata()?.modified()?;

    let datetime = time::OffsetDateTime::from(modified);
    let date = datetime
        .format(time::macros::format_description!("[year]-[month]-[day]"))
        .map_err(std::io::Error::other)?;

    let index = (1..)
        .find(|i| !log_dir.join(format!("{date}-{i}.log.gz")).exists())
        .unwrap();
    let dest = log_dir.join(format!("{date}-{index}.log.gz"));

    let input = std::fs::read(&latest)?;
    let output_file = std::fs::File::create(&dest)?;
    let mut encoder = flate2::write::GzEncoder::new(output_file, flate2::Compression::default());

    std::io::Write::write_all(&mut encoder, &input)?;
    encoder.finish().map_err(std::io::Error::other)?;
    std::fs::remove_file(&latest)?;

    cleanup_old_logs(log_dir);

    Ok(())
}

fn cleanup_old_logs(log_dir: &Path) {
    let mut gz_files: Vec<_> = std::fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "gz"))
        .collect();

    if gz_files.len() <= MAX_LOG_FILES {
        return;
    }

    gz_files.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    for entry in &gz_files[..gz_files.len() - MAX_LOG_FILES] {
        let _ = std::fs::remove_file(entry.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_lands_in_the_log() {
        let dir = std::env::temp_dir().join("pomme-panic-hook-test");
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join(LATEST_LOG);
        let _ = std::fs::remove_file(&log);
        let _ = LOG_FILE.set(log.clone());
        install_panic_hook();
        let _ = std::panic::catch_unwind(|| panic!("the message under test"));
        let written = std::fs::read_to_string(&log).unwrap();
        assert!(written.contains("PANIC") && written.contains("the message under test"));
    }
}
