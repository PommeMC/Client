use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn download_server_pack(
    cache_dir: &Path,
    url: &str,
    hash: &str,
) -> Result<PathBuf, PackDownloadError> {
    let dir = cache_dir.join(hash);
    if !dir.is_dir() {
        let data = crate::net::http::blocking_builder()
            .build()
            .and_then(|client| client.get(url).send())
            .map_err(|e| PackDownloadError::Download(e.to_string()))?
            .bytes()
            .map_err(|e| PackDownloadError::Download(e.to_string()))?;
        tracing::info!("Downloaded {} bytes", data.len());
        validate_hash(&data, hash)?;
        extract_zip(&data, &dir).map_err(|e| PackDownloadError::Extract(e.to_string()))?;
    }
    Ok(dir)
}

#[derive(Debug)]
pub(crate) enum PackDownloadError {
    Download(String),
    HashMismatch,
    Extract(String),
}

impl std::fmt::Display for PackDownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Download(e) => write!(f, "download failed: {e}"),
            Self::HashMismatch => write!(f, "SHA-1 hash mismatch"),
            Self::Extract(e) => write!(f, "extraction failed: {e}"),
        }
    }
}

impl std::error::Error for PackDownloadError {}

fn validate_hash(data: &[u8], expected: &str) -> Result<(), PackDownloadError> {
    if expected.is_empty() {
        return Ok(());
    }
    let actual = sha1_smol::Sha1::from(data).digest().to_string();
    if actual != expected {
        tracing::error!("Hash mismatch: expected {expected}, got {actual}");
        return Err(PackDownloadError::HashMismatch);
    }
    Ok(())
}

fn extract_zip(data: &[u8], dest: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dest)?;
    let cursor = io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let Some(enclosed) = file.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(enclosed);
        if file.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            io::copy(&mut file, &mut out)?;
        }
    }
    tracing::info!("Extracted resource pack to {}", dest.display());
    Ok(())
}
