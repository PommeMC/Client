//! A pack's `shaders/` tree, read from a folder or straight out of its zip.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

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
                let at = name.find("shaders/")?;
                (at == 0 || name[..at].ends_with('/'))
                    .then(|| name[..at + "shaders/".len()].to_owned())
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
            let mut bytes = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut bytes)?;
            files.insert(rel, bytes);
        }
        Ok(Self::Zip(files))
    }

    /// The file at `path`, relative to `shaders/` with `/` separators.
    pub fn read(&self, path: &str) -> Option<String> {
        let bytes = match self {
            Self::Dir(root) => std::fs::read(root.join(path)).ok()?,
            Self::Zip(files) => files.get(path)?.clone(),
        };
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Every file's path, relative to `shaders/`, sorted.
    pub fn files(&self) -> Vec<String> {
        let mut files = match self {
            Self::Dir(root) => {
                let mut files = Vec::new();
                let mut dirs = vec![root.clone()];
                while let Some(dir) = dirs.pop() {
                    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            dirs.push(path);
                        } else if let Ok(rel) = path.strip_prefix(root) {
                            files.push(rel.to_string_lossy().replace('\\', "/"));
                        }
                    }
                }
                files
            }
            Self::Zip(files) => files.keys().cloned().collect(),
        };
        files.sort_unstable();
        files
    }

    pub fn exists(&self, path: &str) -> bool {
        match self {
            Self::Dir(root) => root.join(path).is_file(),
            Self::Zip(files) => files.contains_key(path),
        }
    }
}
