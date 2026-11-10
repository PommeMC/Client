use std::io;
use std::path::{Path, PathBuf};

/// The pack's extracted directory under `cache_dir`, keyed by its content's
/// SHA-1 so a server's hash never names a path. As in vanilla's
/// `DownloadedPackSource.tryParseSha1Hash`, a hash that isn't 40 hex digits
/// counts as none.
pub(crate) fn download_server_pack(
    cache_dir: &Path,
    url: &str,
    hash: &str,
) -> Result<PathBuf, PackDownloadError> {
    let hash = parse_sha1(hash);
    if let Some(hash) = &hash {
        let dir = cache_dir.join(hash);
        if dir.is_dir() {
            return Ok(dir);
        }
    }
    let data = crate::net::http::blocking_builder()
        .build()
        .and_then(|client| client.get(url).send())
        .map_err(|e| PackDownloadError::Download(e.to_string()))?
        .bytes()
        .map_err(|e| PackDownloadError::Download(e.to_string()))?;
    tracing::info!("Downloaded {} bytes", data.len());
    let actual = sha1_smol::Sha1::from(&data).digest().to_string();
    if let Some(expected) = &hash
        && actual != *expected
    {
        tracing::error!("Hash mismatch: expected {expected}, got {actual}");
        return Err(PackDownloadError::HashMismatch);
    }
    let dir = cache_dir.join(&actual);
    if !dir.is_dir() {
        extract_zip(&data, &dir).map_err(|e| PackDownloadError::Extract(e.to_string()))?;
    }
    Ok(dir)
}

fn parse_sha1(hash: &str) -> Option<String> {
    (hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
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

pub(crate) fn extract_zip(data: &[u8], dest: &Path) -> io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::parse_sha1;

    #[test]
    fn only_a_sha1_hash_is_used() {
        let upper = "0123456789ABCDEF0123456789abcdef01234567";
        assert_eq!(parse_sha1(upper), Some(upper.to_ascii_lowercase()));
        for bad in [
            "",
            "..",
            "../../etc",
            "0123",
            &format!("{upper}0"),
            &upper.replace('A', "/"),
        ] {
            assert_eq!(parse_sha1(bad), None, "{bad}");
        }
    }
}
