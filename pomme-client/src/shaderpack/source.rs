//! A pack's `shaders/` tree, read from a folder or straight out of its zip.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub enum PackSource {
    /// The pack folder's `shaders/` directory.
    Dir(PathBuf),
    /// Every file under the zip's `shaders/`, keyed by its path below it.
    Zip(HashMap<String, Vec<u8>>),
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("no shaders/ folder")]
    NoShaders,
}

impl PackSource {
    pub fn open(path: &Path) -> Result<Self, SourceError> {
        if path.is_dir() {
            let shaders = path.join("shaders");
            return if shaders.is_dir() {
                Ok(Self::Dir(shaders))
            } else {
                Err(SourceError::NoShaders)
            };
        }
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
        // Some zips nest the pack in a folder: take the shallowest `shaders/`.
        let prefix = (0..archive.len())
            .filter_map(|i| {
                let name = archive.name_for_index(i)?.ok()?;
                // A whole `shaders` segment, not the end of `sildurs_shaders/`.
                let at = name
                    .match_indices("shaders/")
                    .map(|(at, _)| at)
                    .find(|&at| at == 0 || name[..at].ends_with('/'))?;
                Some(name[..at + "shaders/".len()].to_owned())
            })
            .min_by_key(|p| p.len())
            .ok_or(SourceError::NoShaders)?;
        let mut files = HashMap::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let name = file.name()?;
            let Some(rel) = name.strip_prefix(&prefix).map(str::to_owned) else {
                continue;
            };
            if file.is_dir() || rel.is_empty() {
                continue;
            }
            // The entry's declared size isn't trusted for the allocation.
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            files.insert(rel, bytes);
        }
        Ok(Self::Zip(files))
    }

    /// The file at `path`, relative to `shaders/` with `/` separators.
    pub fn read(&self, path: &str) -> Option<String> {
        let bytes = match self {
            Self::Dir(root) => std::fs::read(root.join(inside(path)?)).ok()?,
            Self::Zip(files) => files.get(path)?.clone(),
        };
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn exists(&self, path: &str) -> bool {
        match self {
            Self::Dir(root) => inside(path).is_some_and(|p| root.join(p).is_file()),
            Self::Zip(files) => files.contains_key(path),
        }
    }

    pub fn has_folder(&self, path: &str) -> bool {
        match self {
            Self::Dir(root) => inside(path).is_some_and(|p| root.join(p).is_dir()),
            Self::Zip(files) => {
                let prefix = format!("{path}/");
                files.keys().any(|f| f.starts_with(&prefix))
            }
        }
    }
}

/// `path` if it stays inside the pack: plain names only, so no `..`, root
/// or drive (`C:`) can take a join outside it.
fn inside(path: &str) -> Option<&Path> {
    let path = Path::new(path);
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then_some(path)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn nested_zip_shaders_folder() {
        let path = std::env::temp_dir().join(format!("pomme_pack_{}.zip", std::process::id()));
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for name in [
            "sildurs_shaders/readme.txt",
            "sildurs_shaders/shaders/final.fsh",
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();
        let source = PackSource::open(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(source.unwrap().exists("final.fsh"));
    }

    #[test]
    fn paths_stay_inside_the_pack() {
        assert!(inside("world0/composite.fsh").is_some());
        assert!(inside("../outside.glsl").is_none());
        assert!(inside("/etc/passwd").is_none());
        // Only a drive on Windows; elsewhere `C:` is a plain folder name.
        #[cfg(windows)]
        assert!(inside("C:/Windows/win.ini").is_none());
    }
}
