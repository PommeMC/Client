use std::path::Path;

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
    let file_appender = tracing_appender::rolling::never(log_dir, "latest.log");
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

/// Panics to the log. Android drops stderr, so one before logging starts goes
/// straight to logcat.
#[cfg(target_os = "android")]
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if tracing::dispatcher::has_been_set() {
            tracing::error!("{info}");
        } else {
            let logcat = tracing_subscriber::registry().with(console_layer());
            tracing::subscriber::with_default(logcat, || tracing::error!("{info}"));
        }
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
    let latest = log_dir.join("latest.log");
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
