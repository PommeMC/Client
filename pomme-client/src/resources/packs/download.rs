use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Vanilla `DownloadedPackSource.MAX_PACK_SIZE_BYTES`.
const MAX_DOWNLOAD_BYTES: u64 = 250 * 1024 * 1024;

/// Vanilla `HttpUtil.downloadFile`: the pack's ZIP under `cache_dir`, named by
/// its content's SHA-1. A cached copy is reused only once its hash checks out;
/// otherwise the download goes to a temporary file that is moved into place,
/// so a partial download never takes the name. As in vanilla's
/// `DownloadedPackSource.tryParseSha1Hash`, a hash that isn't 40 hex digits
/// counts as none.
// TODO: vanilla `DownloadQueue` also keeps an event log and prunes the cache to
// its 20 newest packs (`DownloadCacheCleaner`).
pub(crate) fn download_server_pack(
    cache_dir: &Path,
    url: &str,
    hash: &str,
) -> Result<PathBuf, PackDownloadError> {
    let hash = parse_sha1(hash);
    if let Some(hash) = &hash {
        let cached = cache_dir.join(hash);
        if file_has_hash(&cached, hash) {
            tracing::info!("Returning cached file since actual hash matches requested");
            return Ok(cached);
        }
    }

    std::fs::create_dir_all(cache_dir).map_err(PackDownloadError::Write)?;
    let mut download = tempfile::Builder::new()
        .prefix("download")
        .suffix(".tmp")
        .tempfile_in(cache_dir)
        .map_err(PackDownloadError::Write)?;
    let actual = fetch_into(url, download.as_file_mut())?;
    if let Some(expected) = hash
        && actual != expected
    {
        return Err(PackDownloadError::HashMismatch { expected, actual });
    }
    let path = cache_dir.join(&actual);
    if !file_has_hash(&path, &actual) {
        download
            .persist(&path)
            .map_err(|error| PackDownloadError::Write(error.error))?;
    }
    Ok(path)
}

/// Streams a response into `out`, failing past vanilla's size cap whether or
/// not the server sent a Content-Length. Returns the bytes' SHA-1.
fn fetch_into(url: &str, out: &mut File) -> Result<String, PackDownloadError> {
    let mut response = crate::net::http::blocking_builder()
        .build()
        .and_then(|client| client.get(url).send())
        .map_err(|error| PackDownloadError::Download(error.without_url()))?;
    // `HttpURLConnection.getInputStream` throws on an error status.
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        return Err(PackDownloadError::UnexpectedStatus(status));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_DOWNLOAD_BYTES)
    {
        return Err(PackDownloadError::DownloadTooLarge);
    }
    copy_hashed(&mut response, out, MAX_DOWNLOAD_BYTES)
}

/// Vanilla `HttpUtil.downloadAndHash`.
fn copy_hashed(
    input: &mut impl Read,
    out: &mut impl Write,
    max_bytes: u64,
) -> Result<String, PackDownloadError> {
    let mut hasher = sha1_smol::Sha1::new();
    let mut buffer = [0; 16 * 1024];
    let mut read_so_far = 0u64;
    loop {
        let count = input.read(&mut buffer).map_err(PackDownloadError::Read)?;
        if count == 0 {
            return Ok(hasher.digest().to_string());
        }
        read_so_far += count as u64;
        if read_so_far > max_bytes {
            return Err(PackDownloadError::DownloadTooLarge);
        }
        hasher.update(&buffer[..count]);
        out.write_all(&buffer[..count])
            .map_err(PackDownloadError::Write)?;
    }
}

/// Vanilla `HttpUtil.checkExistingFile`.
fn file_has_hash(path: &Path, hash: &str) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let actual = copy_hashed(&mut file, &mut io::sink(), u64::MAX);
    match actual {
        Ok(actual) if actual == hash => true,
        Ok(actual) => {
            tracing::warn!(
                "Mismatched hash of file {}, expected {hash} but found {actual}",
                path.display()
            );
            false
        }
        Err(error) => {
            tracing::warn!("Failed to check cached file {}: {error}", path.display());
            false
        }
    }
}

fn parse_sha1(hash: &str) -> Option<String> {
    (hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PackDownloadError {
    #[error("download failed: {0}")]
    Download(#[source] reqwest::Error),
    #[error("download returned HTTP {0}")]
    UnexpectedStatus(reqwest::StatusCode),
    #[error("reading the download failed: {0}")]
    Read(#[source] io::Error),
    #[error("download exceeds the {MAX_DOWNLOAD_BYTES}-byte limit")]
    DownloadTooLarge,
    #[error("SHA-1 hash mismatch (expected {expected}, got {actual})")]
    HashMismatch { expected: String, actual: String },
    #[error("writing the download failed: {0}")]
    Write(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a valid SHA-1 can select an existing cache file.
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

    /// The size cap applies to the stream itself, past the last byte allowed.
    #[test]
    fn copy_limits_bytes_and_hashes_them() {
        let mut out = Vec::new();
        let hash = copy_hashed(&mut &b"pack"[..], &mut out, 4).unwrap();
        assert_eq!(out, b"pack");
        assert_eq!(hash, sha1_smol::Sha1::from(b"pack").digest().to_string());
        assert!(matches!(
            copy_hashed(&mut &b"pack!"[..], &mut Vec::new(), 4),
            Err(PackDownloadError::DownloadTooLarge)
        ));
    }

    /// A cached file is reused only when its contents still match the hash.
    #[test]
    fn a_cached_file_must_match_its_hash() {
        let cache = tempfile::tempdir().unwrap();
        let hash = sha1_smol::Sha1::from(b"pack").digest().to_string();
        let path = cache.path().join(&hash);
        assert!(!file_has_hash(&path, &hash));
        std::fs::write(&path, b"pack").unwrap();
        assert!(file_has_hash(&path, &hash));
        std::fs::write(&path, b"tampered").unwrap();
        assert!(!file_has_hash(&path, &hash));
    }
}
