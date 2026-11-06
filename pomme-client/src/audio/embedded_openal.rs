//! The OpenAL Soft library a release build carries (see `build.rs`), unpacked
//! into Pomme's data folder to load, as the vanilla launcher unpacks LWJGL's
//! copy from its jar. Other builds carry none.

use std::path::{Path, PathBuf};

const LIBRARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/openal-embedded"));
const LIBRARY_NAME: &str = env!("POMME_OPENAL_NAME");
/// The licence and notice OpenAL Soft is distributed under, unpacked with it.
const NOTICES: [(&str, &str); 2] = [
    (
        "OpenAL-Soft-LICENSE.txt",
        include_str!("../../third-party/OpenAL-Soft-LICENSE.txt"),
    ),
    (
        "OpenAL-Soft-NOTICE.txt",
        include_str!("../../third-party/OpenAL-Soft-NOTICE.txt"),
    ),
];

/// The unpacked library, written on the first launch of each build of it.
pub(super) fn unpack() -> Option<Result<PathBuf, String>> {
    if LIBRARY.is_empty() {
        return None;
    }
    let dir = pomme_dirs::layout()
        .natives()
        .join(concat!("openal-", env!("POMME_OPENAL_KEY")));
    Some(
        unpack_to(&dir, LIBRARY_NAME, LIBRARY)
            .map_err(|e| format!("unpacking to {}: {e}", dir.display())),
    )
}

/// Removes the copy and notices that releases from before embedding shipped
/// beside the binary in `dir`. A different build of the library there is a
/// player's own, which loads first, and stays.
pub fn remove_bundled_copy(dir: &Path) {
    if !LIBRARY.is_empty() {
        remove_copy(dir, LIBRARY_NAME, LIBRARY);
    }
}

fn unpack_to(dir: &Path, name: &str, library: &[u8]) -> std::io::Result<PathBuf> {
    let path = dir.join(name);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() == library.len() as u64) {
        return Ok(path);
    }
    // The library goes last, so a present one means the notices are too.
    std::fs::create_dir_all(dir)?;
    for (notice, text) in NOTICES {
        crate::util::write_atomic(&dir.join(notice), text.as_bytes())?;
    }
    crate::util::write_atomic(&path, library)?;
    Ok(path)
}

fn remove_copy(dir: &Path, name: &str, library: &[u8]) {
    let path = dir.join(name);
    if std::fs::read(&path).is_ok_and(|bytes| bytes == library) {
        let _ = std::fs::remove_file(path);
        for (notice, _) in NOTICES {
            let _ = std::fs::remove_file(dir.join(notice));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_temp_dir;

    #[test]
    fn unpacks_the_library_with_its_notices_once() {
        let dir = test_temp_dir("openal_unpack");
        let path = unpack_to(&dir, "OpenAL.dll", b"library").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"library");
        for (notice, text) in NOTICES {
            assert_eq!(std::fs::read_to_string(dir.join(notice)).unwrap(), text);
        }
        // An unpacked library is left as it is.
        std::fs::remove_file(dir.join(NOTICES[0].0)).unwrap();
        unpack_to(&dir, "OpenAL.dll", b"library").unwrap();
        assert!(!dir.join(NOTICES[0].0).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn removes_only_its_own_bundled_copy() {
        let dir = test_temp_dir("openal_bundled");
        std::fs::create_dir_all(&dir).unwrap();
        let write = |library: &[u8]| {
            std::fs::write(dir.join("OpenAL.dll"), library).unwrap();
            for (notice, text) in NOTICES {
                std::fs::write(dir.join(notice), text).unwrap();
            }
        };

        write(b"a player's build");
        remove_copy(&dir, "OpenAL.dll", b"library");
        assert!(dir.join("OpenAL.dll").exists());
        assert!(dir.join(NOTICES[1].0).exists());

        write(b"library");
        remove_copy(&dir, "OpenAL.dll", b"library");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
