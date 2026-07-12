pub struct UserData {
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: Option<String>,
    /// True when `uuid` is a launcher-supplied profile UUID rather than a
    /// synthesized offline one (which has no Mojang profile to fetch).
    pub has_profile: bool,
}

impl UserData {
    pub fn from_args(
        username: Option<String>,
        uuid: Option<String>,
        access_token: Option<String>,
    ) -> Self {
        let username = username.unwrap_or_else(|| "Steve".to_string());

        let uuid = uuid.and_then(|s| uuid::Uuid::parse_str(&s).ok());
        let has_profile = uuid.is_some();
        let uuid = uuid.unwrap_or_else(|| Self::offline_uuid(&username));

        Self {
            username,
            uuid,
            access_token,
            has_profile,
        }
    }

    fn offline_uuid(username: &str) -> uuid::Uuid {
        uuid::Uuid::new_v3(
            &uuid::Uuid::NAMESPACE_DNS,
            format!("OfflinePlayer:{username}").as_bytes(),
        )
    }
}
