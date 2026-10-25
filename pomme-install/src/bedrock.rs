//! The parts of Bedrock's vanilla resource pack the Bedrock theme draws,
//! from Mojang's `bedrock-samples` repo pinned to one release.
//!
//! Each folder's file list comes from GitHub's git tree API by tree sha, so it
//! can't change under us, and every file is checked against its git blob hash.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::{Fetch, Hash, Progress};

const REPO: &str = "Mojang/bedrock-samples";
const TAG: &str = "v1.26.50.4";
const COMMIT: &str = "46ba6ea985fb5a92d79a9419198f10dda14c199d";
/// Bedrock's UI textures, relative to `dir`.
pub const UI_DIR: &str = "resource_pack/textures/ui";
/// The repo folders downloaded, with their tree shas at `COMMIT`.
const FOLDERS: &[(&str, &str)] = &[(UI_DIR, "06a2b4631b9f8ef76579ed793b49852691de8aff")];
const MARKER: &str = ".installed";

#[derive(Deserialize)]
struct Tree {
    tree: Vec<Entry>,
    truncated: bool,
}

#[derive(Deserialize)]
struct Entry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    #[serde(default)]
    size: u64,
}

/// Where the pinned release lands under `root`, laid out as in the repo.
pub fn dir(root: &Path) -> PathBuf {
    root.join(TAG)
}

pub fn is_installed(root: &Path) -> bool {
    dir(root).join(MARKER).exists()
}

/// Downloads whatever of the pinned release is missing into `dir(root)`,
/// reporting through `on_progress`.
pub async fn install(
    http: &reqwest::Client,
    root: &Path,
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<(), String> {
    let report = crate::reporter(on_progress);
    report(0, 1, "Fetching the file list...");
    let out = dir(root);
    let mut files = Vec::new();
    for &(folder, tree_sha) in FOLDERS {
        let url = format!("https://api.github.com/repos/{REPO}/git/trees/{tree_sha}?recursive=1");
        files.extend(fetches(crate::get_json(http, &url).await?, folder, &out)?);
    }
    crate::download_all(http, files, &report).await?;
    // After the files, so a marker on disk means they're all there.
    crate::write_atomic(&out.join(MARKER), TAG.as_bytes()).await
}

/// The downloads for `tree`, the listing of repo folder `folder`.
fn fetches(tree: Tree, folder: &str, out: &Path) -> Result<Vec<Fetch>, String> {
    if tree.truncated {
        return Err(format!("{folder}: GitHub cut the file list short"));
    }
    tree.tree
        .into_iter()
        .filter(|entry| entry.kind == "blob")
        .map(|entry| {
            let path = Path::new(&entry.path);
            if !path.components().all(|c| matches!(c, Component::Normal(_))) {
                return Err(format!("{folder}: refusing the path {}", entry.path));
            }
            Ok(Fetch {
                url: format!(
                    "https://raw.githubusercontent.com/{REPO}/{COMMIT}/{folder}/{}",
                    entry.path
                ),
                path: out.join(folder).join(path),
                size: entry.size,
                expected: entry.sha,
                hash: Hash::GitBlob,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(truncated: bool, entries: &[(&str, &str)]) -> Tree {
        let entries: Vec<_> = entries
            .iter()
            .map(|&(path, kind)| {
                serde_json::json!({ "path": path, "type": kind, "sha": "ab", "size": 3 })
            })
            .collect();
        serde_json::from_value(serde_json::json!({ "tree": entries, "truncated": truncated }))
            .unwrap()
    }

    #[test]
    fn blobs_map_to_raw_urls_inside_the_folder() {
        let out = Path::new("out");
        let files = fetches(
            tree(false, &[("bubble", "tree"), ("bubble/bg.png", "blob")]),
            "resource_pack/textures/ui",
            out,
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].url,
            format!(
                "https://raw.githubusercontent.com/{REPO}/{COMMIT}/resource_pack/textures/ui/bubble/bg.png"
            )
        );
        assert_eq!(
            files[0].path,
            out.join("resource_pack/textures/ui").join("bubble/bg.png")
        );
        assert_eq!((files[0].size, files[0].expected.as_str()), (3, "ab"));
    }

    #[test]
    fn truncated_lists_and_escaping_paths_are_refused() {
        let out = Path::new("out");
        assert!(fetches(tree(true, &[]), "ui", out).is_err());
        for path in ["../escape.png", "/abs.png", "./a.png"] {
            assert!(fetches(tree(false, &[(path, "blob")]), "ui", out).is_err());
        }
    }

    #[test]
    fn installed_once_the_marker_lands() {
        let root = crate::tests::temp_dir("bedrock");
        assert!(!is_installed(&root));
        std::fs::create_dir_all(dir(&root)).unwrap();
        assert!(!is_installed(&root));
        std::fs::write(dir(&root).join(MARKER), TAG).unwrap();
        assert!(is_installed(&root));
        std::fs::remove_dir_all(root).unwrap();
    }
}
