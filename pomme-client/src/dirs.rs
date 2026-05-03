use std::path::{Path, PathBuf};

use directories::ProjectDirs;

pub struct DataDirs {
    pub indexes_dir: PathBuf,
    pub objects_dir: PathBuf,
    pub pomme_assets_dir: PathBuf,
    pub jar_assets_dir: PathBuf,
    pub game_dir: PathBuf,
}

impl DataDirs {
    pub fn resolve(
        version: &str,
        assets_dir: Option<&str>,
        versions_dir: Option<&str>,
        game_dir: Option<&str>,
    ) -> Self {
        let root_dir = data_dir();

        let assets_dir = assets_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| root_dir.join("assets"));

        let pomme_assets_dir = root_dir.join("pomme-assets");

        let game_dir = game_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| root_dir.join("installations").join("default"));

        let versions_dir = versions_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| root_dir.join("versions"));

        let jar_assets_dir = versions_dir.join(version).join("extracted/assets");

        let indexes_dir = assets_dir.join("indexes");
        let objects_dir = assets_dir.join("objects");

        Self {
            indexes_dir,
            objects_dir,
            jar_assets_dir,
            pomme_assets_dir,
            game_dir,
        }
    }

    pub fn ensure_game_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.game_dir)
    }

    pub fn verify(&self) -> Result<(), String> {
        let DataDirs {
            indexes_dir,
            objects_dir,
            pomme_assets_dir: _,
            jar_assets_dir,
            game_dir: _,
        } = self;

        for dir in [indexes_dir, objects_dir, jar_assets_dir] {
            if !dir.exists() {
                return Err(format!(
                    "{} not found, please use the launcher or specify all arguments.",
                    dir.display()
                ));
            }
        }
        Ok(())
    }
}

fn data_dir() -> PathBuf {
    ProjectDirs::from("", "", ".pomme")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| Path::new(".pomme").to_path_buf())
}
