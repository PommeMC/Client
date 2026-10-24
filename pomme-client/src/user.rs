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

    /// Who a launch plays as, and whether it must sign in first. A release
    /// build only plays as a signed-in Microsoft account that owns Java
    /// Edition, so it ignores account flags (even the old launcher's) and
    /// restores the stored account or signs in. A dev build takes them, to
    /// play offline; without any, a phone signs in and a desktop plays
    /// offline, never reading the keychain unasked.
    pub fn for_launch(
        username: Option<String>,
        uuid: Option<String>,
        access_token: Option<String>,
        release: bool,
        mobile: bool,
    ) -> (Self, bool) {
        if release {
            return (Self::from_args(None, None, None), true);
        }
        let flagged = username.is_some() || uuid.is_some() || access_token.is_some();
        (
            Self::from_args(username, uuid, access_token),
            mobile && !flagged,
        )
    }

    pub fn from_account(account: &pomme_auth::AuthAccount) -> Self {
        Self::from_args(
            Some(account.username.clone()),
            Some(account.uuid.clone()),
            Some(account.access_token.clone()),
        )
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

    #[test]
    fn only_dev_builds_play_from_account_flags() {
        let launch = |release, mobile| {
            UserData::for_launch(
                Some("Steve".to_owned()),
                Some("5627dd98-e6be-3c21-b8a8-e92344183641".to_owned()),
                Some("token".to_owned()),
                release,
                mobile,
            )
        };
        for mobile in [false, true] {
            let (user, required) = launch(true, mobile);
            assert!(required);
            assert!(user.access_token.is_none() && !user.has_profile);

            let (user, required) = launch(false, mobile);
            assert!(!required);
            assert_eq!(user.access_token.as_deref(), Some("token"));
        }
        let bare = |mobile| UserData::for_launch(None, None, None, false, mobile).1;
        assert!(!bare(false), "a desktop dev build plays offline");
        assert!(bare(true), "a phone dev build signs in");
    }
}
