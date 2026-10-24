//! Microsoft account sign-in for Minecraft: the token from either sign-in
//! flow is exchanged through Xbox Live for a Minecraft access token, and the
//! account and its refresh token are kept in a [`SecretStore`].
//!
//! Every call takes the HTTP client to use, so the client can pass one that
//! trusts Android's CA store.

mod device_code;
mod pkce;
mod store;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub use device_code::{DeviceCode, device_code_sign_in};
pub use pkce::pkce_sign_in;
use serde::{Deserialize, Serialize};
#[cfg(feature = "keyring")]
pub use store::KeyringStore;
pub use store::{FileStore, SecretStore};

const CLIENT_ID: &str = "b2dd2c0f-8a09-4549-9d76-ab43b8572695";
const SCOPE: &str = "XboxLive.signin offline_access";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";

const ACCOUNTS_KEY: &str = "minecraft-accounts";
const REFRESH_KEY: &str = "minecraft-refresh-tokens";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct AuthAccount {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub expires_at: u64,
}

#[derive(Deserialize)]
struct MsaTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct XblResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: XblDisplayClaims,
}

#[derive(Deserialize)]
struct XblDisplayClaims {
    xui: Vec<XblXui>,
}

#[derive(Deserialize)]
struct XblXui {
    uhs: String,
}

#[derive(Deserialize)]
struct McAuthResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct McProfileResponse {
    id: String,
    name: String,
}

/// Error message including the source chain (`reqwest` hides the detail there).
pub fn error_chain(e: impl std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !msg.contains(&text) {
            msg.push_str(": ");
            msg.push_str(&text);
        }
        source = s.source();
    }
    msg
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_json<T: serde::de::DeserializeOwned + Default>(store: &dyn SecretStore, key: &str) -> T {
    store
        .read(key)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn write_json(store: &dyn SecretStore, key: &str, value: &impl Serialize) {
    if let Ok(json) = serde_json::to_string(value) {
        store.write(key, &json);
    }
}

pub fn get_all_accounts(store: &dyn SecretStore) -> Vec<AuthAccount> {
    read_json(store, ACCOUNTS_KEY)
}

fn refresh_tokens(store: &dyn SecretStore) -> HashMap<String, String> {
    read_json(store, REFRESH_KEY)
}

/// The stored account, while its access token is still valid.
pub fn try_restore(store: &dyn SecretStore, uuid: &str) -> Option<AuthAccount> {
    get_all_accounts(store)
        .into_iter()
        .find(|a| a.uuid == uuid && a.expires_at > unix_now())
}

pub async fn try_refresh(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    uuid: &str,
) -> Option<AuthAccount> {
    let refresh_token = refresh_tokens(store).remove(uuid)?;
    refresh_msa_token(http, store, &refresh_token).await.ok()
}

pub async fn try_restore_or_refresh(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    uuid: &str,
) -> Option<AuthAccount> {
    match try_restore(store, uuid) {
        Some(account) => Some(account),
        None => try_refresh(http, store, uuid).await,
    }
}

pub fn remove_account(store: &dyn SecretStore, uuid: &str) {
    let mut accounts = get_all_accounts(store);
    accounts.retain(|a| a.uuid != uuid);
    write_json(store, ACCOUNTS_KEY, &accounts);
    let mut tokens = refresh_tokens(store);
    tokens.remove(uuid);
    write_json(store, REFRESH_KEY, &tokens);
}

fn save_account(store: &dyn SecretStore, account: &AuthAccount, refresh_token: Option<&str>) {
    let mut accounts = get_all_accounts(store);
    accounts.retain(|a| a.uuid != account.uuid);
    accounts.push(account.clone());
    write_json(store, ACCOUNTS_KEY, &accounts);
    if let Some(token) = refresh_token {
        let mut tokens = refresh_tokens(store);
        tokens.insert(account.uuid.clone(), token.to_owned());
        write_json(store, REFRESH_KEY, &tokens);
    }
}

async fn refresh_msa_token(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    refresh_token: &str,
) -> Result<AuthAccount, String> {
    let msa: MsaTokenResponse = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|e| format!("Refresh failed: {}", crate::error_chain(e)))?
        .json()
        .await
        .map_err(|e| format!("Refresh parse failed: {}", crate::error_chain(e)))?;
    finish_msa_exchange(http, store, &msa).await
}

/// Exchanges a fresh Microsoft token for a Minecraft account and stores it.
async fn finish_msa_exchange(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    msa: &MsaTokenResponse,
) -> Result<AuthAccount, String> {
    let account = exchange_msa_to_minecraft(http, &msa.access_token).await?;
    save_account(store, &account, msa.refresh_token.as_deref());
    Ok(account)
}

async fn exchange_msa_to_minecraft(
    http: &reqwest::Client,
    msa_token: &str,
) -> Result<AuthAccount, String> {
    let xbl: XblResponse = http
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={msa_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await
        .map_err(|e| format!("Xbox Live auth failed: {}", crate::error_chain(e)))?
        .json()
        .await
        .map_err(|e| format!("Xbox Live parse failed: {}", crate::error_chain(e)))?;

    let user_hash = xbl
        .display_claims
        .xui
        .first()
        .map(|x| x.uhs.clone())
        .ok_or("No user hash in XBL response")?;

    let xsts = http
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .json(&serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [&xbl.token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }))
        .send()
        .await
        .map_err(|e| format!("XSTS auth failed: {}", crate::error_chain(e)))?;
    let xsts_ok = xsts.status().is_success();
    let xsts_body = xsts
        .text()
        .await
        .map_err(|e| format!("XSTS read failed: {}", crate::error_chain(e)))?;
    if !xsts_ok {
        return Err(xsts_error(&xsts_body));
    }
    let xsts: XblResponse = serde_json::from_str(&xsts_body)
        .map_err(|e| format!("XSTS parse failed: {}", crate::error_chain(e)))?;

    let mc: McAuthResponse = http
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({
            "identityToken": format!("XBL3.0 x={user_hash};{}", xsts.token),
        }))
        .send()
        .await
        .map_err(|e| format!("MC auth failed: {}", crate::error_chain(e)))?
        .json()
        .await
        .map_err(|e| format!("MC auth parse failed: {}", crate::error_chain(e)))?;

    let profile = http
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(&mc.access_token)
        .send()
        .await
        .map_err(|e| format!("Profile fetch failed: {}", crate::error_chain(e)))?;
    if profile.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("This Microsoft account doesn't own Minecraft".into());
    }
    let profile: McProfileResponse = profile
        .json()
        .await
        .map_err(|e| format!("Profile parse failed: {}", crate::error_chain(e)))?;

    Ok(AuthAccount {
        username: profile.name,
        uuid: profile.id,
        access_token: mc.access_token,
        // Minecraft tokens last a day.
        expires_at: unix_now() + 86400,
    })
}

/// Why XSTS refused the account, from its `XErr` code.
fn xsts_error(body: &str) -> String {
    #[derive(Deserialize)]
    struct XstsError {
        #[serde(rename = "XErr")]
        code: u64,
    }
    let reason = match serde_json::from_str::<XstsError>(body).map(|e| e.code) {
        Ok(2148916233) => "This Microsoft account has no Xbox profile; create one at xbox.com",
        Ok(2148916235) => "Xbox Live isn't available in this account's country",
        Ok(2148916236 | 2148916237) => "This account needs adult verification on xbox.com",
        Ok(2148916238) => "This is a child account; an adult must add it to a Microsoft family",
        _ => return format!("XSTS auth refused: {body}"),
    };
    reason.into()
}

#[cfg(test)]
mod tests {
    use super::{
        AuthAccount, FileStore, get_all_accounts, remove_account, save_account, xsts_error,
    };

    #[test]
    fn xsts_errors_read_as_reasons() {
        assert!(xsts_error(r#"{"XErr":2148916233,"Message":""}"#).contains("no Xbox profile"));
        assert!(xsts_error(r#"{"XErr":2148916238}"#).contains("child account"));
        assert!(xsts_error("not json").starts_with("XSTS auth refused"));
    }

    #[test]
    fn accounts_save_replace_and_remove() {
        let dir = std::env::temp_dir().join(format!("pomme-auth-accounts-{}", std::process::id()));
        let store = FileStore {
            path: dir.join("accounts.json"),
        };
        let account = |token: &str| AuthAccount {
            username: "Tester".into(),
            uuid: "abc".into(),
            access_token: token.into(),
            expires_at: 0,
        };
        save_account(&store, &account("one"), Some("refresh"));
        save_account(&store, &account("two"), None);
        assert_eq!(get_all_accounts(&store), [account("two")]);
        remove_account(&store, "abc");
        assert!(get_all_accounts(&store).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
