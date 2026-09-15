//! Helpers shared by unit tests.

use std::path::PathBuf;

/// A fresh directory name under the system temp dir; the test creates and
/// removes it.
pub fn test_temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("pomme_{label}_{}_{}", std::process::id(), nonce))
}
