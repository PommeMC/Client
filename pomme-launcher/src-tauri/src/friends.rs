use serde::{Deserialize, Serialize};

const FRIENDS_URL: &str = "https://api.minecraftservices.com/friends";
const PRESENCE_URL: &str = "https://api.minecraftservices.com/presence";
const ATTRIBUTES_URL: &str = "https://api.minecraftservices.com/player/attributes";

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

pub async fn get_friends(access_token: &str) -> Result<FriendsList, String> {
    let resp = reqwest::Client::new()
        .get(FRIENDS_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("Friends fetch failed: {e}"))?;
    handle_response(resp).await
}

pub async fn action_by_id(
    access_token: &str,
    profile_id: &str,
    action: UpdateType,
) -> Result<FriendsList, String> {
    put_action(
        access_token,
        FriendActionRequest {
            name: None,
            profile_id: Some(profile_id),
            update_type: action.as_str(),
        },
    )
    .await
}

pub async fn action_by_name(
    access_token: &str,
    name: &str,
    action: UpdateType,
) -> Result<FriendsList, String> {
    put_action(
        access_token,
        FriendActionRequest {
            name: Some(name),
            profile_id: None,
            update_type: action.as_str(),
        },
    )
    .await
}

async fn put_action(
    access_token: &str,
    body: FriendActionRequest<'_>,
) -> Result<FriendsList, String> {
    let resp = reqwest::Client::new()
        .put(FRIENDS_URL)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Friend action failed: {e}"))?;
    handle_response(resp).await
}

async fn handle_response(resp: reqwest::Response) -> Result<FriendsList, String> {
    let status = resp.status();
    if status.is_success() {
        return resp
            .json()
            .await
            .map_err(|e| format!("Friends response parse failed: {e}"));
    }
    Err(map_error(status.as_u16()))
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
struct PresenceRequest {
    status: &'static str,
    #[serde(rename = "joinInfo")]
    join_info: Option<()>,
}

pub async fn update_presence(access_token: &str) -> Result<Vec<PresenceEntry>, String> {
    let resp = reqwest::Client::new()
        .post(PRESENCE_URL)
        .bearer_auth(access_token)
        .json(&PresenceRequest {
            status: "ONLINE",
            join_info: None,
        })
        .send()
        .await
        .map_err(|e| format!("Presence post failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(map_error(status.as_u16()));
    }
    let mut parsed: PresenceResponse = resp
        .json()
        .await
        .map_err(|e| format!("Presence parse failed: {e}"))?;
    // Mojang's /presence returns dashed UUIDs; /friends returns undashed — normalize.
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

async fn parse_attributes(resp: reqwest::Response) -> Result<FriendSettings, String> {
    let status = resp.status();
    if !status.is_success() {
        return Err(map_error(status.as_u16()));
    }
    let dto: UserAttributesResponseDto = resp
        .json()
        .await
        .map_err(|e| format!("Settings parse failed: {e}"))?;
    let prefs = dto.friends_preferences.unwrap_or_default();
    Ok(FriendSettings {
        show_in_list: prefs.friends.as_deref() != Some("DISABLED"),
        accept_invites: prefs.accept_invites.as_deref() != Some("DISABLED"),
    })
}

pub async fn get_friend_settings(access_token: &str) -> Result<FriendSettings, String> {
    let resp = reqwest::Client::new()
        .get(ATTRIBUTES_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("Settings fetch failed: {e}"))?;
    parse_attributes(resp).await
}

pub async fn update_friend_settings(
    access_token: &str,
    show: bool,
    accept: bool,
) -> Result<FriendSettings, String> {
    let resp = reqwest::Client::new()
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
        .map_err(|e| format!("Settings update failed: {e}"))?;
    parse_attributes(resp).await
}

fn map_error(status: u16) -> String {
    match status {
        400 => "Unknown profile name".to_string(),
        403 => "Account does not have an active Java profile".to_string(),
        429 => "Rate limited — try again in a moment".to_string(),
        s if s >= 500 => "Friends service unavailable — try again later".to_string(),
        s => format!("Friends service returned HTTP {s}"),
    }
}
