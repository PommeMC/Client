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

/// The most recently signed-in account, refreshed if its token expired.
// TODO: refresh again when a long session outlives the day-long token.
pub fn restore(rt: &tokio::runtime::Runtime, store: &dyn SecretStore) -> Option<AuthAccount> {
    let last = pomme_auth::get_all_accounts(store).pop()?;
    rt.block_on(pomme_auth::try_restore_or_refresh(
        crate::net::http::client(),
        store,
        &last.uuid,
    ))
}
