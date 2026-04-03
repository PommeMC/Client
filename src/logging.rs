use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub fn init(log_dir: &Path) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::never(log_dir, "latest.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

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
        .with(
            fmt::layer()
                .with_writer(std::io::stdout)
                .with_filter(stdout_filter),
        )
        .init();

    guard
}

const MAX_LOG_FILES: usize = 5;

pub fn rotate(log_dir: &Path) -> std::io::Result<()> {
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
