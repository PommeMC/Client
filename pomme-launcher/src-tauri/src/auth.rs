//! The launcher's side of `pomme-auth`: accounts in the OS keyring, and the
//! browser sign-in announced to the UI.

pub use pomme_auth::AuthAccount;
use pomme_auth::KeyringStore;
use serde::Serialize;

#[derive(Serialize, Clone, specta::Type, tauri_specta::Event)]
pub struct AuthUrlEvent {
    pub url: String,
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

pub fn get_all_accounts() -> Vec<AuthAccount> {
    pomme_auth::get_all_accounts(&KeyringStore)
}

pub fn try_restore(uuid: &str) -> Option<AuthAccount> {
    pomme_auth::try_restore(&KeyringStore, uuid)
}

pub async fn try_restore_or_refresh(uuid: &str) -> Option<AuthAccount> {
    pomme_auth::try_restore_or_refresh(http(), &KeyringStore, uuid).await
}

pub fn remove_account(uuid: &str) {
    pomme_auth::remove_account(&KeyringStore, uuid);
}

pub async fn oauth_sign_in(app: tauri::AppHandle) -> Result<AuthAccount, String> {
    use tauri_specta::Event;

    pomme_auth::pkce_sign_in(http(), &KeyringStore, |url| {
        let _ = AuthUrlEvent {
            url: url.to_owned(),
        }
        .emit(&app);
        if let Err(e) = open::that_detached(url) {
            log::warn!("failed to open browser for sign-in: {e}");
        }
    })
    .await
}
