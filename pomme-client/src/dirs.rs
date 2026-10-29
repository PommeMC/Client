use std::path::PathBuf;

#[derive(Clone)]
pub struct DataDirs {
    pub assets_dir: PathBuf,
    pub versions_dir: PathBuf,
    pub indexes_dir: PathBuf,
    pub objects_dir: PathBuf,
    pub pomme_assets_dir: PathBuf,
    pub bedrock_assets_dir: PathBuf,
    pub client_jar: PathBuf,
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

        let client_jar = versions_dir.join(version).join(format!("{version}.jar"));
        let jar_assets_dir = versions_dir.join(version).join("extracted/assets");

        let indexes_dir = assets_dir.join("indexes");
        let objects_dir = assets_dir.join("objects");

        Self {
            assets_dir,
            versions_dir,
            indexes_dir,
            objects_dir,
            client_jar,
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

    pub fn asset_index_file(&self, version: &str) -> PathBuf {
        self.indexes_dir.join(format!("{version}.json"))
    }

    pub(crate) fn installation_location(
        &self,
        version: &str,
    ) -> crate::resources::packs::InstallationLocation {
        crate::resources::packs::InstallationLocation {
            version: version.to_owned(),
            client_jar: self.client_jar.clone(),
            index_file: self.asset_index_file(version),
            objects_dir: self.objects_dir.clone(),
        }
    }

    /// `version`'s assets are downloaded, so the client can start on them.
    pub fn is_installed(&self, version: &str) -> bool {
        pomme_install::is_installed(&self.assets_dir, &self.versions_dir, version)
    }

    /// The downloaded Bedrock UI textures the Bedrock theme draws.
    pub fn bedrock_ui_dir(&self) -> PathBuf {
        pomme_install::bedrock::dir(&self.bedrock_assets_dir).join(pomme_install::bedrock::UI_DIR)
    }
}
