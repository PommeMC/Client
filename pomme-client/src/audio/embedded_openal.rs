//! The OpenAL Soft library a release build carries (see `build.rs`), unpacked
//! into Pomme's data folder to load, as the vanilla launcher unpacks LWJGL's
//! copy from its jar. Other builds carry none.

use std::fs;
use std::path::{Path, PathBuf};

const LIBRARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/openal-embedded"));
const LIBRARY_NAME: &str = env!("POMME_OPENAL_NAME");
/// Each build of the library unpacks into its own folder, this plus its key.
const FOLDER_PREFIX: &str = "openal-";
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

/// The unpacked library.
pub(super) fn unpack() -> Option<Result<PathBuf, String>> {
    if LIBRARY.is_empty() {
        return None;
    }
    let natives = pomme_dirs::layout().natives();
    let folder = format!("{FOLDER_PREFIX}{}", env!("POMME_OPENAL_KEY"));
    Some(
        unpack_to(&natives, &folder, LIBRARY_NAME, LIBRARY)
            .map_err(|e| format!("unpacking to {}: {e}", natives.display())),
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

/// Unpacks `library` into `natives/folder` unless it's there, then removes
/// the folders older builds of it unpacked into.
fn unpack_to(natives: &Path, folder: &str, name: &str, library: &[u8]) -> std::io::Result<PathBuf> {
    let dir = natives.join(folder);
    let path = dir.join(name);
    if !fs::metadata(&path).is_ok_and(|meta| meta.len() == library.len() as u64) {
        // The library goes last, so a present one means the notices are too.
        fs::create_dir_all(&dir)?;
        for (notice, text) in NOTICES {
            crate::util::write_atomic(&dir.join(notice), text.as_bytes())?;
        }
        crate::util::write_atomic(&path, library)?;
    }
    // A folder an older Pomme still has loaded stays until a later launch.
    for entry in fs::read_dir(natives)?.flatten() {
        let old = entry.file_name();
        if old != folder && old.to_string_lossy().starts_with(FOLDER_PREFIX) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(path)
}

fn remove_copy(dir: &Path, name: &str, library: &[u8]) {
    let path = dir.join(name);
    if fs::read(&path).is_ok_and(|bytes| bytes == library) {
        let _ = fs::remove_file(path);
        for (notice, _) in NOTICES {
            let _ = fs::remove_file(dir.join(notice));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_temp_dir;

    #[test]
    fn unpacks_the_library_with_its_notices_once() {
        let natives = test_temp_dir("openal_unpack");
        let dir = natives.join("openal-1");
        let path = unpack_to(&natives, "openal-1", "OpenAL.dll", b"library").unwrap();
        assert_eq!(path, dir.join("OpenAL.dll"));
        assert_eq!(fs::read(&path).unwrap(), b"library");
        for (notice, text) in NOTICES {
            assert_eq!(fs::read_to_string(dir.join(notice)).unwrap(), text);
        }
        // An unpacked library is left as it is.
        fs::remove_file(dir.join(NOTICES[0].0)).unwrap();
        unpack_to(&natives, "openal-1", "OpenAL.dll", b"library").unwrap();
        assert!(!dir.join(NOTICES[0].0).exists());
        fs::remove_dir_all(&natives).unwrap();
    }

    #[test]
    fn a_new_build_removes_older_ones() {
        let natives = test_temp_dir("openal_replace");
        fs::create_dir_all(natives.join("other")).unwrap();
        unpack_to(&natives, "openal-1", "OpenAL.dll", b"old").unwrap();
        unpack_to(&natives, "openal-2", "OpenAL.dll", b"new").unwrap();
        assert!(!natives.join("openal-1").exists());
        assert!(natives.join("openal-2/OpenAL.dll").exists());
        assert!(natives.join("other").exists());
        fs::remove_dir_all(&natives).unwrap();
    }

    #[test]
    fn removes_only_its_own_bundled_copy() {
        let dir = test_temp_dir("openal_bundled");
        fs::create_dir_all(&dir).unwrap();
        let write = |library: &[u8]| {
            fs::write(dir.join("OpenAL.dll"), library).unwrap();
            for (notice, text) in NOTICES {
                fs::write(dir.join(notice), text).unwrap();
            }
        };

        write(b"a player's build");
        remove_copy(&dir, "OpenAL.dll", b"library");
        assert!(dir.join("OpenAL.dll").exists());
        assert!(dir.join(NOTICES[1].0).exists());

        write(b"library");
        remove_copy(&dir, "OpenAL.dll", b"library");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }
}
