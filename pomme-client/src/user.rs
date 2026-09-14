use md5::Digest as _;

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

    /// Vanilla `UUIDUtil.createOfflinePlayerUUID`: a bare MD5 of the prefixed
    /// name, not a namespaced v3 UUID.
    fn offline_uuid(username: &str) -> uuid::Uuid {
        let digest = md5::Md5::digest(format!("OfflinePlayer:{username}").as_bytes());
        uuid::Builder::from_md5_bytes(digest.into()).into_uuid()
    }
}

#[cfg(test)]
mod tests {
    use super::UserData;

    #[test]
    fn offline_uuid_matches_vanilla() {
        for (name, expected) in [
            ("Notch", "b50ad385-829d-3141-a216-7e7d7539ba7f"),
            ("Steve", "5627dd98-e6be-3c21-b8a8-e92344183641"),
            ("Alex", "36532b5e-c442-3dbb-a24c-c7e55d0f979a"),
        ] {
            assert_eq!(UserData::offline_uuid(name).to_string(), expected);
        }
    }
}
