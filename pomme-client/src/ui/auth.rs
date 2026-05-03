// TODO: Move to launcher - authentication will be handled by mc-launcher when it exists.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_uuid(s: &str) -> uuid::Uuid {
    uuid::Uuid::parse_str(s)
        .or_else(|_| {
            let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            uuid::Uuid::parse_str(&hex)
        })
        .unwrap_or(uuid::Uuid::nil())
}

pub struct AuthAccount {
    pub username: String,
    pub uuid: uuid::Uuid,
    pub access_token: String,
}

pub enum AuthStatus {
    Idle,
    OpeningBrowser,
    WaitingForBrowser,
    Exchanging,
    Success(AuthAccount),
    Failed(String),
}

const CLIENT_ID: &str = "00000000441cc96b";
const SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";

pub fn spawn_auth(
    rt: &tokio::runtime::Runtime,
    status: Arc<Mutex<AuthStatus>>,
    cache_file: PathBuf,
) {
    *status.lock() = AuthStatus::OpeningBrowser;
    rt.spawn(async move {
        match run_auth_flow(cache_file, Arc::clone(&status)).await {
            Ok(account) => {
                *status.lock() = AuthStatus::Success(account);
            }
            Err(e) => {
                *status.lock() = AuthStatus::Failed(e.to_string());
            }
        }
    });
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    user_code: String,
    device_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct MsaTokenResponse {
    access_token: String,
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

#[derive(Serialize, Deserialize)]
struct CachedAuth {
    username: String,
    uuid: String,
    access_token: String,
    expires_at: u64,
}

async fn run_auth_flow(
    cache_file: PathBuf,
    status: Arc<Mutex<AuthStatus>>,
) -> Result<AuthAccount, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();

    let device_code: DeviceCodeResponse = client
        .post("https://login.live.com/oauth20_connect.srf")
        .form(&[
            ("scope", SCOPE),
            ("client_id", CLIENT_ID),
            ("response_type", "device_code"),
        ])
        .send()
        .await?
        .json()
        .await?;

    let login_url = format!(
        "{}?otc={}",
        device_code.verification_uri, device_code.user_code
    );
    let _ = open::that(&login_url);
    *status.lock() = AuthStatus::WaitingForBrowser;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(device_code.expires_in);
    let interval = Duration::from_secs(device_code.interval);

    let msa = loop {
        tokio::time::sleep(interval).await;
        if tokio::time::Instant::now() > deadline {
            return Err("Authentication timed out".into());
        }
        let resp = client
            .post(format!(
                "https://login.live.com/oauth20_token.srf?client_id={CLIENT_ID}"
            ))
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", &device_code.device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?;
        if let Ok(token) = resp.json::<MsaTokenResponse>().await {
            break token;
        }
    };

    *status.lock() = AuthStatus::Exchanging;

    let xbl: XblResponse = client
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": &msa.access_token,
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await?
        .json()
        .await?;

    let user_hash = &xbl.display_claims.xui[0].uhs;

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
        .await?
        .json()
        .await?;

    let mc_auth: McAuthResponse = client
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .json(&serde_json::json!({
            "identityToken": format!("XBL3.0 x={};{}", user_hash, xsts.token),
        }))
        .send()
        .await?
        .json()
        .await?;

    let profile: McProfileResponse = client
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(&mc_auth.access_token)
        .send()
        .await?
        .json()
        .await?;

    let uuid = parse_uuid(&profile.id);

    let cached = CachedAuth {
        username: profile.name.clone(),
        uuid: uuid.to_string(),
        access_token: mc_auth.access_token.clone(),
        expires_at: unix_now() + 86400,
    };
    if let Ok(json) = serde_json::to_string_pretty(&cached) {
        let _ = std::fs::write(&cache_file, json);
    }

    Ok(AuthAccount {
        username: profile.name,
        uuid,
        access_token: mc_auth.access_token,
    })
}

pub fn try_restore_cached(cache_file: &Path) -> Option<AuthAccount> {
    let contents = std::fs::read_to_string(cache_file).ok()?;
    let cached: CachedAuth = serde_json::from_str(&contents).ok()?;
    if cached.expires_at > unix_now() {
        Some(AuthAccount {
            username: cached.username,
            uuid: parse_uuid(&cached.uuid),
            access_token: cached.access_token,
        })
    } else {
        None
    }
}
