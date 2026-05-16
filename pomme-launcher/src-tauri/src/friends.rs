use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const FRIENDS_URL: &str = "https://api.minecraftservices.com/friends";
const PRESENCE_URL: &str = "https://api.minecraftservices.com/presence";
const ATTRIBUTES_URL: &str = "https://api.minecraftservices.com/player/attributes";
const DEFAULT_RETRY_AFTER_SECS: u32 = 60;

#[derive(Clone, Copy)]
enum FriendsApi {
    FriendByName,
    FriendById,
    Presence,
    Attributes,
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

#[derive(Serialize, Clone, Debug, specta::Type)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FriendsApiError {
    RateLimited { retry_after_secs: u32 },
    Message { message: String },
}

impl FriendsApiError {
    fn msg(s: impl Into<String>) -> Self {
        Self::Message { message: s.into() }
    }
}

impl From<String> for FriendsApiError {
    fn from(message: String) -> Self {
        Self::Message { message }
    }
}

impl std::fmt::Display for FriendsApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited { retry_after_secs } => {
                write!(f, "Rate limited, try again in {retry_after_secs}s")
            }
            Self::Message { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for FriendsApiError {}

#[derive(Serialize, Deserialize, Clone, specta::Type)]
pub struct Friend {
    #[serde(rename = "profileId")]
    pub profile_id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Default, specta::Type)]
pub struct FriendsList {
    #[serde(default)]
    pub friends: Vec<Friend>,
    #[serde(default, rename = "incomingRequests")]
    pub incoming_requests: Vec<Friend>,
    #[serde(default, rename = "outgoingRequests")]
    pub outgoing_requests: Vec<Friend>,
}

pub enum UpdateType {
    Add,
    Remove,
}

impl UpdateType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Add => "ADD",
            Self::Remove => "REMOVE",
        }
    }
}

#[derive(Serialize)]
struct FriendActionRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(rename = "profileId", skip_serializing_if = "Option::is_none")]
    profile_id: Option<&'a str>,
    #[serde(rename = "updateType")]
    update_type: &'static str,
}

pub async fn get_friends(access_token: &str) -> Result<FriendsList, FriendsApiError> {
    let resp = http()
        .get(FRIENDS_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| FriendsApiError::msg(format!("Friends fetch failed: {e}")))?;
    handle_response(resp, FriendsApi::FriendById).await
}

pub async fn action_by_id(
    access_token: &str,
    profile_id: &str,
    action: UpdateType,
) -> Result<FriendsList, FriendsApiError> {
    put_action(
        access_token,
        FriendActionRequest {
            name: None,
            profile_id: Some(profile_id),
            update_type: action.as_str(),
        },
        FriendsApi::FriendById,
    )
    .await
}

pub async fn action_by_name(
    access_token: &str,
    name: &str,
    action: UpdateType,
) -> Result<FriendsList, FriendsApiError> {
    put_action(
        access_token,
        FriendActionRequest {
            name: Some(name),
            profile_id: None,
            update_type: action.as_str(),
        },
        FriendsApi::FriendByName,
    )
    .await
}

async fn put_action(
    access_token: &str,
    body: FriendActionRequest<'_>,
    api: FriendsApi,
) -> Result<FriendsList, FriendsApiError> {
    let resp = http()
        .put(FRIENDS_URL)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| FriendsApiError::msg(format!("Friend action failed: {e}")))?;
    handle_response(resp, api).await
}

fn check_status(
    resp: reqwest::Response,
    api: FriendsApi,
) -> Result<reqwest::Response, FriendsApiError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry_after = parse_retry_after(resp.headers());
    Err(map_error(api, status.as_u16(), retry_after))
}

async fn handle_response(
    resp: reqwest::Response,
    api: FriendsApi,
) -> Result<FriendsList, FriendsApiError> {
    check_status(resp, api)?
        .json()
        .await
        .map_err(|e| FriendsApiError::msg(format!("Friends response parse failed: {e}")))
}

#[derive(Serialize, Deserialize, Clone, specta::Type)]
pub struct PresenceJoinInfo {
    pub value: String,
    pub invited: bool,
}

#[derive(Serialize, Deserialize, Clone, specta::Type)]
pub struct PresenceEntry {
    #[serde(rename = "profileId")]
    pub profile_id: String,
    pub status: String,
    #[serde(default, rename = "joinInfo")]
    pub join_info: Option<PresenceJoinInfo>,
    #[serde(default, rename = "lastUpdated")]
    pub last_updated: Option<String>,
}

#[derive(Deserialize, Default)]
struct PresenceResponse {
    #[serde(default)]
    presence: Vec<PresenceEntry>,
}

#[derive(Serialize)]
struct PresenceRequest<'a> {
    status: &'a str,
    #[serde(rename = "joinInfo", skip_serializing_if = "Option::is_none")]
    join_info: Option<&'a PresenceJoinInfo>,
}

pub async fn update_presence(
    access_token: &str,
    status: &str,
    join_info: Option<&PresenceJoinInfo>,
) -> Result<Vec<PresenceEntry>, FriendsApiError> {
    let resp = http()
        .post(PRESENCE_URL)
        .bearer_auth(access_token)
        .json(&PresenceRequest { status, join_info })
        .send()
        .await
        .map_err(|e| FriendsApiError::msg(format!("Presence post failed: {e}")))?;
    let mut parsed: PresenceResponse = check_status(resp, FriendsApi::Presence)?
        .json()
        .await
        .map_err(|e| FriendsApiError::msg(format!("Presence parse failed: {e}")))?;
    // Mojang's /presence returns dashed UUIDs; /friends returns undashed.
    // Normalize.
    for entry in &mut parsed.presence {
        entry.profile_id.retain(|c| c != '-');
    }
    Ok(parsed.presence)
}

#[derive(Serialize, Deserialize, Clone, specta::Type)]
pub struct FriendSettings {
    pub show_in_list: bool,
    pub accept_invites: bool,
}

#[derive(Deserialize, Default)]
struct FriendsPreferencesDto {
    #[serde(default)]
    friends: Option<String>,
    #[serde(default, rename = "acceptInvites")]
    accept_invites: Option<String>,
}

#[derive(Deserialize, Default)]
struct UserAttributesResponseDto {
    #[serde(default, rename = "friendsPreferences")]
    friends_preferences: Option<FriendsPreferencesDto>,
}

#[derive(Serialize)]
struct FriendsPreferencesUpdate {
    friends: &'static str,
    #[serde(rename = "acceptInvites")]
    accept_invites: &'static str,
}

#[derive(Serialize)]
struct UserAttributesUpdate {
    #[serde(rename = "friendsPreferences")]
    friends_preferences: FriendsPreferencesUpdate,
}

fn toggle_str(value: bool) -> &'static str {
    if value { "ENABLED" } else { "DISABLED" }
}

async fn parse_attributes(resp: reqwest::Response) -> Result<FriendSettings, FriendsApiError> {
    let dto: UserAttributesResponseDto =
        check_status(resp, FriendsApi::Attributes)?
            .json()
            .await
            .map_err(|e| FriendsApiError::msg(format!("Settings parse failed: {e}")))?;
    let prefs = dto.friends_preferences.unwrap_or_default();
    // Mojang omits the field when unset; treat anything other than explicit
    // DISABLED as enabled.
    Ok(FriendSettings {
        show_in_list: prefs.friends.as_deref() != Some("DISABLED"),
        accept_invites: prefs.accept_invites.as_deref() != Some("DISABLED"),
    })
}

pub async fn get_friend_settings(access_token: &str) -> Result<FriendSettings, FriendsApiError> {
    let resp = http()
        .get(ATTRIBUTES_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| FriendsApiError::msg(format!("Settings fetch failed: {e}")))?;
    parse_attributes(resp).await
}

pub async fn update_friend_settings(
    access_token: &str,
    show: bool,
    accept: bool,
) -> Result<FriendSettings, FriendsApiError> {
    let resp = http()
        .post(ATTRIBUTES_URL)
        .bearer_auth(access_token)
        .json(&UserAttributesUpdate {
            friends_preferences: FriendsPreferencesUpdate {
                friends: toggle_str(show),
                accept_invites: toggle_str(accept),
            },
        })
        .send()
        .await
        .map_err(|e| FriendsApiError::msg(format!("Settings update failed: {e}")))?;
    parse_attributes(resp).await
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> u32 {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS)
}

fn map_error(api: FriendsApi, status: u16, retry_after_secs: u32) -> FriendsApiError {
    match (api, status) {
        (_, 429) => FriendsApiError::RateLimited { retry_after_secs },
        (FriendsApi::FriendByName, 400) => FriendsApiError::msg("Unknown profile name"),
        (_, 400) => FriendsApiError::msg("Invalid request"),
        (_, 403) => FriendsApiError::msg("Account does not have an active Java profile"),
        (_, s) if s >= 500 => FriendsApiError::msg("Friends service unavailable, try again later"),
        (_, s) => FriendsApiError::msg(format!("Friends service returned HTTP {s}")),
    }
}
