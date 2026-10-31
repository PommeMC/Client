use std::path::PathBuf;

#[derive(Clone)]
pub struct DataDirs {
    pub assets_dir: PathBuf,
    pub versions_dir: PathBuf,
    pub indexes_dir: PathBuf,
    pub objects_dir: PathBuf,
    pub pomme_assets_dir: PathBuf,
    pub bedrock_assets_dir: PathBuf,
    pub jar_assets_dir: PathBuf,
    pub game_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl DataDirs {
    pub fn resolve(
        version: &str,
        assets_dir: Option<&str>,
        versions_dir: Option<&str>,
        game_dir: Option<&str>,
    ) -> Self {
        let layout = pomme_dirs::layout();

        let assets_dir = assets_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| layout.minecraft_assets());

        let game_dir = game_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| layout.default_profile());

        let versions_dir = versions_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| layout.minecraft_versions());

        let jar_assets_dir = versions_dir.join(version).join("extracted/assets");

        let indexes_dir = assets_dir.join("indexes");
        let objects_dir = assets_dir.join("objects");

        Self {
            assets_dir,
            versions_dir,
            indexes_dir,
            objects_dir,
            jar_assets_dir,
            pomme_assets_dir: layout.pomme_assets(),
            bedrock_assets_dir: layout.bedrock_assets(),
            game_dir,
            cache_dir: layout.cache(version),
        }
    }

    pub fn ensure_game_dir(&self) -> std::io::Result<()> {
        pomme_dirs::create_profile(&self.game_dir)
    }

    /// The client also needs the cached JAR for installed format metadata.
    /// Older extracted-only installs get one normal installer repair pass.
    pub fn is_installed(&self, version: &str) -> bool {
        pomme_install::is_installed(&self.assets_dir, &self.versions_dir, version)
            && pomme_install::client_jar_path(&self.versions_dir, version).is_file()
    }

    pub(crate) fn installation_location(
        &self,
        version: &str,
    ) -> crate::resources::packs::InstallationLocation {
        crate::resources::packs::InstallationLocation {
            version: version.to_owned(),
            client_jar: pomme_install::client_jar_path(&self.versions_dir, version),
            extracted_root: self.versions_dir.join(version).join("extracted"),
            index_file: self.indexes_dir.join(format!("{version}.json")),
            objects_dir: self.objects_dir.clone(),
        }
    }

    /// The downloaded Bedrock UI textures the Bedrock theme draws.
    pub fn bedrock_ui_dir(&self) -> PathBuf {
        pomme_install::bedrock::dir(&self.bedrock_assets_dir).join(pomme_install::bedrock::UI_DIR)
    }
}
