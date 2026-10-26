//! `--capture-frames`: records every clientbound frame as the server sent
//! it, before translation, in `pomme_protocol::capture`'s format.

use pomme_protocol::Phase;

#[cfg(feature = "test-harness")]
static FILE: std::sync::OnceLock<std::sync::Mutex<std::fs::File>> = std::sync::OnceLock::new();

/// Starts recording into `path`, truncating it.
#[cfg(feature = "test-harness")]
pub fn start(path: &std::path::Path) -> std::io::Result<()> {
    let _ = FILE.set(std::sync::Mutex::new(std::fs::File::create(path)?));
    Ok(())
}

/// Appends `frame`, written at once so a harness exit loses nothing; a no-op
/// unless [`start`] ran.
pub fn record(phase: Phase, frame: &[u8]) {
    #[cfg(feature = "test-harness")]
    if let Some(file) = FILE.get() {
        use std::io::Write;
        let mut record = Vec::with_capacity(frame.len() + 5);
        pomme_protocol::capture::write_record(&mut record, phase, frame);
        if let Err(e) = file.lock().unwrap().write_all(&record) {
            tracing::warn!("Couldn't record a frame: {e}");
        }
    }
    #[cfg(not(feature = "test-harness"))]
    let _ = (phase, frame);
}
