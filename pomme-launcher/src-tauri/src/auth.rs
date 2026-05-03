use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLIENT_ID: &str = "b2dd2c0f-8a09-4549-9d76-ab43b8572695";
const REDIRECT_URI: &str = "http://localhost:25585";
const SCOPE: &str = "XboxLive.signin offline_access";

#[derive(Serialize, Deserialize, Clone, specta::Type)]
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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

const KEYRING_SERVICE: &str = "pomme-launcher";
const KEYRING_ACCOUNTS: &str = "minecraft-accounts";
const KEYRING_REFRESH: &str = "minecraft-refresh-tokens";

fn keyring_read(key: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, key)
        .ok()?
        .get_password()
        .ok()
}

fn keyring_write(key: &str, value: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, key) {
        let _ = entry.set_password(value);
    }
}

pub fn get_all_accounts() -> Vec<AuthAccount> {
    keyring_read(KEYRING_ACCOUNTS)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn get_refresh_tokens() -> std::collections::HashMap<String, String> {
    keyring_read(KEYRING_REFRESH)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn try_restore(uuid: &str) -> Option<AuthAccount> {
    get_all_accounts()
        .into_iter()
        .find(|a| a.uuid == uuid && a.expires_at > unix_now())
}

pub async fn try_refresh(uuid: &str) -> Option<AuthAccount> {
    let tokens = get_refresh_tokens();
    let refresh_token = tokens.get(uuid)?;
    refresh_msa_token(refresh_token).await.ok()
}

pub async fn try_restore_or_refresh(uuid: &str) -> Option<AuthAccount> {
    if let Some(account) = try_restore(uuid) {
        return Some(account);
    }
    try_refresh(uuid).await
}

async fn refresh_msa_token(refresh_token: &str) -> Result<AuthAccount, String> {
    let client = reqwest::Client::new();

    let msa: MsaTokenResponse = client
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
        .form(&[
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|e| format!("Refresh failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Refresh parse failed: {e}"))?;

    finish_msa_exchange(&client, &msa).await
}

fn save_account(account: &AuthAccount) {
    let mut accounts = get_all_accounts();
    accounts.retain(|a| a.uuid != account.uuid);
    accounts.push(account.clone());
    if let Ok(json) = serde_json::to_string(&accounts) {
        keyring_write(KEYRING_ACCOUNTS, &json);
    }
}

fn save_refresh_token(uuid: &str, token: &str) {
    let mut tokens = get_refresh_tokens();
    tokens.insert(uuid.to_string(), token.to_string());
    if let Ok(json) = serde_json::to_string(&tokens) {
        keyring_write(KEYRING_REFRESH, &json);
    }
}

pub fn remove_account(uuid: &str) {
    let mut accounts = get_all_accounts();
    accounts.retain(|a| a.uuid != uuid);
    if let Ok(json) = serde_json::to_string(&accounts) {
        keyring_write(KEYRING_ACCOUNTS, &json);
    }
    let mut tokens = get_refresh_tokens();
    tokens.remove(uuid);
    if let Ok(json) = serde_json::to_string(&tokens) {
        keyring_write(KEYRING_REFRESH, &json);
    }
}

fn generate_pkce() -> (String, String) {
    use base64::Engine;
    use sha2::Digest;

    let verifier_bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&verifier_bytes);

    let digest = sha2::Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);

    (verifier, challenge)
}

pub async fn oauth_sign_in() -> Result<AuthAccount, String> {
    let (verifier, challenge) = generate_pkce();

    let state: String = (0..16)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();

    let auth_url = format!(
        "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?\
         client_id={CLIENT_ID}\
         &response_type=code\
         &redirect_uri={REDIRECT_URI}\
         &scope={}\
         &state={state}\
         &code_challenge={challenge}\
         &code_challenge_method=S256",
        urlencoding::encode(SCOPE),
    );

    let _ = open::that(&auth_url);

    let code = listen_for_callback(&state).await?;

    let client = reqwest::Client::new();
    let resp = client
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
        .form(&[
            ("client_id", CLIENT_ID),
            ("code", &code),
            ("redirect_uri", REDIRECT_URI),
            ("grant_type", "authorization_code"),
            ("code_verifier", &verifier),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|e| format!("Token exchange failed: {e}"))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Token exchange read failed: {e}"))?;

    if !status.is_success() {
        return Err(format!("Auth failed ({status}): {body}"));
    }

    let msa: MsaTokenResponse =
        serde_json::from_str(&body).map_err(|e| format!("Token parse failed: {e}"))?;

    finish_msa_exchange(&client, &msa).await
}

async fn finish_msa_exchange(
    client: &reqwest::Client,
    msa: &MsaTokenResponse,
) -> Result<AuthAccount, String> {
    let account = exchange_msa_to_minecraft(client, &msa.access_token).await?;
    if let Some(refresh) = &msa.refresh_token {
        save_refresh_token(&account.uuid, refresh);
    }
    Ok(account)
}

async fn listen_for_callback(expected_state: &str) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:25585")
        .await
        .map_err(|e| format!("Failed to bind callback listener: {e}"))?;

    let timeout = Duration::from_secs(300);
    let (mut stream, _) = tokio::time::timeout(timeout, listener.accept())
        .await
        .map_err(|_| "Authentication timed out".to_string())?
        .map_err(|e| format!("Accept failed: {e}"))?;

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| format!("Read failed: {e}"))?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("Invalid HTTP request")?;

    let query = path.split('?').nth(1).ok_or("No query params")?;
    let params: std::collections::HashMap<&str, &str> =
        query.split('&').filter_map(|p| p.split_once('=')).collect();

    if let Some(error) = params.get("error") {
        let desc = params.get("error_description").unwrap_or(error);
        let body = format!("Authentication failed: {desc}");
        send_http_response(&mut stream, 400, &body).await;
        return Err(format!("OAuth error: {desc}"));
    }

    let state = params.get("state").ok_or("Missing state")?;
    if *state != expected_state {
        return Err("State mismatch".to_string());
    }

    let raw_code = params.get("code").ok_or("Missing auth code")?;
    let code = urlencoding::decode(raw_code)
        .map_err(|e| format!("Failed to decode auth code: {e}"))?
        .into_owned();

    send_http_response(
        &mut stream,
        200,
        "Signed in successfully! You can close this tab.",
    )
    .await;

    Ok(code)
}

async fn send_http_response(stream: &mut tokio::net::TcpStream, status: u16, body: &str) {
    use tokio::io::AsyncWriteExt;

    let status_text = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

async fn exchange_msa_to_minecraft(
    client: &reqwest::Client,
    msa_token: &str,
) -> Result<AuthAccount, String> {
    let xbl: XblResponse = client
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
        .map_err(|e| format!("Xbox Live auth failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Xbox Live parse failed: {e}"))?;

    let user_hash = xbl
        .display_claims
        .xui
        .first()
        .map(|x| x.uhs.clone())
        .ok_or("No user hash in XBL response")?;

    let xsts: XblResponse = client
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
        .map_err(|e| format!("XSTS auth failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("XSTS parse failed: {e}"))?;

    let mc: McAuthResponse = client
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({
            "identityToken": format!("XBL3.0 x={user_hash};{}", xsts.token),
        }))
        .send()
        .await
        .map_err(|e| format!("MC auth failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("MC auth parse failed: {e}"))?;

    let profile: McProfileResponse = client
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(&mc.access_token)
        .send()
        .await
        .map_err(|e| format!("Profile fetch failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Profile parse failed: {e}"))?;

    let account = AuthAccount {
        username: profile.name,
        uuid: profile.id,
        access_token: mc.access_token,
        expires_at: unix_now() + 86400,
    };

    save_account(&account);
    Ok(account)
}
