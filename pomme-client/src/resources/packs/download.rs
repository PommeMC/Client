use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Replaced when cache admission rules change. Older extractions may be
/// partial.
const CACHE_VERSION: &str = "v2";
/// Vanilla's server resource pack download cap.
const MAX_DOWNLOAD_BYTES: u64 = 250 * 1024 * 1024;
const PACK_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PACK_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 65_536;
const MAX_ARCHIVE_DEPTH: usize = 16;

/// Fetches and verifies a pack, then returns its completely extracted directory
/// from the content-addressed server cache.
pub(crate) fn download_server_pack(
    cache_dir: &Path,
    url: &str,
    hash: &str,
) -> Result<PathBuf, PackDownloadError> {
    let cache_dir = cache_dir.join(CACHE_VERSION);
    let hash = parse_sha1(hash);
    if let Some(hash) = &hash {
        let dir = cache_dir.join(hash);
        if dir.is_dir() {
            return Ok(dir);
        }
    }

    let data = fetch_zip(url)?;
    let actual = sha1_smol::Sha1::from(&data).digest().to_string();
    if let Some(expected) = hash
        && actual != expected
    {
        return Err(PackDownloadError::HashMismatch { expected, actual });
    }
    cache_pack(&cache_dir, &actual, &data).map_err(PackDownloadError::Extract)
}

/// Fetches only a successful response and bounds the bytes read even without
/// a Content-Length or when the server sends data continuously.
fn fetch_zip(url: &str) -> Result<Vec<u8>, PackDownloadError> {
    let started = Instant::now();
    let mut response = crate::net::http::blocking_builder()
        .timeout(PACK_REQUEST_TIMEOUT)
        .build()
        .and_then(|client| client.get(url).send())
        .map_err(|error| PackDownloadError::Download(error.without_url()))?;
    if !response.status().is_success() {
        return Err(PackDownloadError::UnexpectedStatus(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_DOWNLOAD_BYTES)
    {
        return Err(PackDownloadError::DownloadTooLarge);
    }
    read_zip(&mut response, MAX_DOWNLOAD_BYTES, started)
}

/// Reads a streaming response up to the byte cap and wall-clock deadline.
fn read_zip(
    response: &mut impl Read,
    max_bytes: u64,
    started: Instant,
) -> Result<Vec<u8>, PackDownloadError> {
    let mut data = Vec::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        if started.elapsed() >= PACK_DOWNLOAD_TIMEOUT {
            return Err(PackDownloadError::DownloadTimedOut);
        }
        let count = response
            .read(&mut buffer)
            .map_err(PackDownloadError::Read)?;
        if count == 0 {
            return Ok(data);
        }
        if (data.len() + count) as u64 > max_bytes {
            return Err(PackDownloadError::DownloadTooLarge);
        }
        data.extend_from_slice(&buffer[..count]);
    }
}

/// Normalizes a vanilla SHA-1 string. The server can omit it or supply an
/// invalid value, in which case only the downloaded bytes select a cache key.
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
    #[error("download exceeded the time limit")]
    DownloadTimedOut,
    #[error("SHA-1 hash mismatch (expected {expected}, got {actual})")]
    HashMismatch { expected: String, actual: String },
    #[error("extraction failed: {0}")]
    Extract(#[source] io::Error),
}

/// Extracts into a private sibling directory. Only a complete extraction is
/// published, and a concurrent successful publisher can win the rename.
fn cache_pack(cache_dir: &Path, hash: &str, data: &[u8]) -> io::Result<PathBuf> {
    let dir = cache_dir.join(hash);
    if !dir.is_dir() {
        fs::create_dir_all(cache_dir)?;
        let staging = tempfile::Builder::new()
            .prefix(".extract-")
            .tempdir_in(cache_dir)?;
        extract_zip(data, staging.path(), MAX_UNPACKED_BYTES)?;
        if !staging.path().join("pack.mcmeta").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "resource pack is missing pack.mcmeta",
            ));
        }
        if let Err(error) = fs::rename(staging.path(), &dir)
            && !dir.is_dir()
        {
            return Err(error);
        }
    }
    Ok(dir)
}

/// Extracts regular files into a fresh staging directory within the limits.
/// Rejects unsafe paths and links and never overwrites an existing file.
fn extract_zip(data: &[u8], dest: &Path, max_bytes: u64) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(io::Cursor::new(data))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "resource pack has too many ZIP entries",
        ));
    }

    let mut remaining = max_bytes;
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let enclosed = file
            .enclosed_name()
            .filter(|path| {
                !path.as_os_str().is_empty() && path.components().count() <= MAX_ARCHIVE_DEPTH
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid ZIP entry path"))?;
        if file.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "resource pack contains a ZIP symlink",
            ));
        }
        let out_path = dest.join(enclosed);
        if file.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        let size = file.size();
        if size > remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "resource pack exceeds the extracted size limit",
            ));
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = File::create_new(&out_path)?;
        let written = io::copy(&mut file.take(size + 1), &mut out)?;
        if written != size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ZIP entry's actual size differs from its declared size",
            ));
        }
        remaining -= written;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;

    /// Only a valid SHA-1 can select an existing cache directory.
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

    /// The download limit applies to the stream independently of its headers.
    #[test]
    fn read_zip_limits_bytes_and_time() {
        assert_eq!(
            read_zip(&mut &b"pack"[..], 4, Instant::now()).unwrap(),
            b"pack"
        );
        assert!(matches!(
            read_zip(&mut &b"pack!"[..], 4, Instant::now()),
            Err(PackDownloadError::DownloadTooLarge)
        ));
        assert!(matches!(
            read_zip(&mut &b"pack"[..], 4, Instant::now() - PACK_DOWNLOAD_TIMEOUT),
            Err(PackDownloadError::DownloadTimedOut)
        ));
    }

    /// Creates a small regular ZIP for testing extraction and cache admission.
    fn fixture_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for &(name, data) in entries {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// An incomplete cache entry is never visible and a later download can
    /// publish successfully. The unpacked budget counts all regular files.
    #[test]
    fn cache_only_publishes_complete_packs() {
        let cache = tempfile::tempdir().unwrap();
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let texture = "assets/minecraft/textures/fixture.png";
        let no_metadata = fixture_zip(&[(texture, b"fixture")]);
        assert!(cache_pack(cache.path(), hash, &no_metadata).is_err());
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);

        let complete = fixture_zip(&[("pack.mcmeta", b"{}"), (texture, b"fixture")]);
        let unpack = tempfile::tempdir().unwrap();
        assert!(extract_zip(&complete, unpack.path(), 8).is_err());

        let published = cache_pack(cache.path(), hash, &complete).unwrap();
        assert_eq!(fs::read(published.join(texture)).unwrap(), b"fixture");
        assert_eq!(
            cache_pack(cache.path(), hash, &complete).unwrap(),
            published
        );
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 1);
    }
}
