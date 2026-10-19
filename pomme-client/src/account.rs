//! The Microsoft account the client plays as when no launcher passed one:
//! restored from `pomme-auth`'s store at startup, or signed in from the menu
//! with a device code.

use std::path::Path;

use pomme_auth::{AuthAccount, SecretStore};

/// The OS credential store on desktop, shared with the launcher; Android has
/// none, so a file in the app's private storage.
// TODO: wrap the Android file in the Android Keystore.
pub fn store(game_dir: &Path) -> Box<dyn SecretStore> {
    #[cfg(target_os = "android")]
    return Box::new(pomme_auth::FileStore {
        path: game_dir.join("accounts.json"),
    });
    #[cfg(not(target_os = "android"))]
    {
        let _ = game_dir;
        Box::new(pomme_auth::KeyringStore)
    }
}

/// Removes `uuid`'s stored account, whichever way its uuid was written.
pub fn forget(store: &dyn SecretStore, uuid: &uuid::Uuid) {
    if let Some(account) = pomme_auth::get_all_accounts(store)
        .into_iter()
        .find(|a| uuid::Uuid::parse_str(&a.uuid).is_ok_and(|stored| stored == *uuid))
    {
        pomme_auth::remove_account(store, &account.uuid);
    }
}

/// The most recently signed-in account, refreshed if its token expired.
// TODO: refresh again when a long session outlives the day-long token.
pub async fn restore(store: &dyn SecretStore) -> Option<AuthAccount> {
    let last = pomme_auth::get_all_accounts(store).pop()?;
    pomme_auth::try_restore_or_refresh(crate::net::http::client(), store, &last.uuid).await
}
