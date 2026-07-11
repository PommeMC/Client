//! Client for the Minecraft friends API (read + write).
//!
//! Ported from the launcher's `friends.rs`. Reads the friends list + incoming/
//! outgoing requests plus presence, and performs add/remove/accept/decline/
//! cancel mutations. Results land in shared state via [`refresh_friends`] /
//! [`friend_action`], mirroring the `ping_all_servers` / `PingResults` pattern.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const FRIENDS_URL: &str = "https://api.minecraftservices.com/friends";
const PRESENCE_URL: &str = "https://api.minecraftservices.com/presence";

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

#[derive(Deserialize)]
struct Friend {
    #[serde(rename = "profileId")]
    profile_id: String,
    name: String,
}

#[derive(Deserialize, Default)]
struct FriendsList {
    #[serde(default)]
    friends: Vec<Friend>,
    #[serde(default, rename = "incomingRequests")]
    incoming_requests: Vec<Friend>,
    #[serde(default, rename = "outgoingRequests")]
    outgoing_requests: Vec<Friend>,
}

#[derive(Deserialize)]
struct PresenceEntry {
    #[serde(rename = "profileId")]
    profile_id: String,
    status: String,
}

#[derive(Deserialize, Default)]
struct PresenceResponse {
    #[serde(default)]
    presence: Vec<PresenceEntry>,
}

/// A friend's online status, resolved from the presence response. Mirrors the
/// vanilla `gui.friends.presence.status.*` states.
#[derive(Clone)]
pub enum FriendStatus {
    Offline,
    Online,
    PlayingOffline,
    PlayingServer,
}

impl FriendStatus {
    pub fn is_online(&self) -> bool {
        !matches!(self, Self::Offline)
    }
}

/// A list entry ready to display: name, UUID (for the skin face) and presence
/// (only meaningful/rendered for confirmed friends, not requests).
#[derive(Clone)]
pub struct FriendView {
    pub name: String,
    pub uuid: String,
    pub status: FriendStatus,
}

/// The three friends lists, ready to render.
#[derive(Clone, Default)]
pub struct FriendLists {
    pub friends: Vec<FriendView>,
    pub incoming: Vec<FriendView>,
    pub outgoing: Vec<FriendView>,
}

/// Shared cache of fetched 8x8 faces, keyed by undashed UUID. Mirrors the
/// `PingResults` favicon pattern; uploaded to the renderer face atlas.
pub type FaceCache = Arc<RwLock<HashMap<String, Vec<u8>>>>;

/// Last friend-mutation error, shown inline on the screen (vanilla uses
/// toasts).
pub type ActionError = Arc<RwLock<Option<String>>>;

#[derive(Clone, Default)]
pub enum FriendsState {
    /// Never fetched (or no signed-in account).
    #[default]
    Idle,
    Loading,
    Loaded(FriendLists),
    Failed(String),
}

pub type FriendsData = Arc<RwLock<FriendsState>>;

#[derive(Clone, Copy)]
pub enum UpdateType {
    Add,
    Remove,
}

impl UpdateType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "ADD",
            Self::Remove => "REMOVE",
        }
    }
}

/// A friend mutation requested by the UI.
pub enum FriendAction {
    AddByName(String),
    ById(String, UpdateType),
}

/// Set the shared state to `Loading` and spawn a fetch that resolves it to
/// `Loaded`/`Failed`. Once loaded, spawn a skin fetch per uuid (friends and
/// requests) that fills `faces`. Cheap to call again to refresh.
pub fn refresh_friends(
    rt: Arc<tokio::runtime::Runtime>,
    access_token: String,
    data: &FriendsData,
    faces: &FaceCache,
) {
    *data.write() = FriendsState::Loading;
    let data = Arc::clone(data);
    let faces = Arc::clone(faces);
    let spawn_rt = Arc::clone(&rt);
    spawn_rt.spawn(async move {
        match fetch(&access_token).await {
            Ok(lists) => {
                let uuids = lists
                    .friends
                    .iter()
                    .chain(&lists.incoming)
                    .chain(&lists.outgoing)
                    .map(|f| f.uuid.clone());
                for uuid in uuids {
                    if uuid.is_empty() || faces.read().contains_key(&uuid) {
                        continue;
                    }
                    let faces = Arc::clone(&faces);
                    rt.spawn(async move {
                        if let Ok(skin) = crate::renderer::fetch_skin_texture(&uuid).await
                            && let Some(face) =
                                crate::renderer::pipelines::menu_overlay::extract_face_8x8(
                                    &skin.pixels,
                                    skin.width,
                                    skin.height,
                                )
                        {
                            faces.write().insert(uuid, face);
                        }
                    });
                }
                *data.write() = FriendsState::Loaded(lists);
            }
            Err(e) => *data.write() = FriendsState::Failed(e),
        }
    });
}

/// Perform a friend mutation, then re-fetch on success (so presence + new faces
/// update). On failure, write the message into `err`.
pub fn friend_action(
    rt: Arc<tokio::runtime::Runtime>,
    access_token: String,
    action: FriendAction,
    data: &FriendsData,
    faces: &FaceCache,
    err: &ActionError,
) {
    *err.write() = None;
    let data = Arc::clone(data);
    let faces = Arc::clone(faces);
    let err = Arc::clone(err);
    let spawn_rt = Arc::clone(&rt);
    spawn_rt.spawn(async move {
        let result = match &action {
            FriendAction::AddByName(name) => {
                action_by_name(&access_token, name, UpdateType::Add).await
            }
            FriendAction::ById(uuid, update) => action_by_id(&access_token, uuid, *update).await,
        };
        match result {
            Ok(()) => refresh_friends(rt, access_token, &data, &faces),
            Err(msg) => *err.write() = Some(msg),
        }
    });
}

async fn fetch(access_token: &str) -> Result<FriendLists, String> {
    let list = get_friends(access_token).await?;
    // Presence requires announcing our own status; the API returns friends'
    // presence in the response. A failure here is non-fatal: show everyone
    // offline rather than erroring out.
    let presence = get_presence(access_token).await.unwrap_or_default();

    let mut friends: Vec<FriendView> = list
        .friends
        .into_iter()
        .map(|f| {
            let status = presence
                .get(&f.profile_id)
                .cloned()
                .unwrap_or(FriendStatus::Offline);
            FriendView {
                name: f.name,
                uuid: f.profile_id,
                status,
            }
        })
        .collect();
    // Online first, then alphabetical.
    friends.sort_by(|a, b| {
        b.status
            .is_online()
            .cmp(&a.status.is_online())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(FriendLists {
        friends,
        incoming: list
            .incoming_requests
            .into_iter()
            .map(request_view)
            .collect(),
        outgoing: list
            .outgoing_requests
            .into_iter()
            .map(request_view)
            .collect(),
    })
}

/// Requests carry no presence; render as offline (status isn't shown for them).
fn request_view(f: Friend) -> FriendView {
    FriendView {
        name: f.name,
        uuid: f.profile_id,
        status: FriendStatus::Offline,
    }
}

async fn get_friends(access_token: &str) -> Result<FriendsList, String> {
    let resp = http()
        .get(FRIENDS_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("Friends fetch failed: {e}"))?;
    check_status(&resp, false)?;
    resp.json()
        .await
        .map_err(|e| format!("Friends response parse failed: {e}"))
}

/// Posts our presence ("ONLINE", idle) and reads back friends' presence,
/// returned keyed by undashed profile id.
async fn get_presence(access_token: &str) -> Result<HashMap<String, FriendStatus>, String> {
    let resp = http()
        .post(PRESENCE_URL)
        .bearer_auth(access_token)
        .json(&serde_json::json!({ "status": "ONLINE" }))
        .send()
        .await
        .map_err(|e| format!("Presence post failed: {e}"))?;
    check_status(&resp, false)?;
    let parsed: PresenceResponse = resp
        .json()
        .await
        .map_err(|e| format!("Presence parse failed: {e}"))?;
    Ok(parsed
        .presence
        .into_iter()
        .map(|mut e| {
            // /presence returns dashed UUIDs; /friends returns undashed.
            e.profile_id.retain(|c| c != '-');
            let status = match e.status.as_str() {
                "ONLINE" => FriendStatus::Online,
                "PLAYING_OFFLINE" => FriendStatus::PlayingOffline,
                // PLAYING_SERVER / PLAYING_HOSTED_SERVER / PLAYING_REALMS
                s if s.starts_with("PLAYING") => FriendStatus::PlayingServer,
                _ => FriendStatus::Offline,
            };
            (e.profile_id, status)
        })
        .collect())
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

async fn action_by_id(
    access_token: &str,
    profile_id: &str,
    action: UpdateType,
) -> Result<(), String> {
    put_action(
        access_token,
        FriendActionRequest {
            name: None,
            profile_id: Some(profile_id),
            update_type: action.as_str(),
        },
        false,
    )
    .await
}

async fn action_by_name(access_token: &str, name: &str, action: UpdateType) -> Result<(), String> {
    put_action(
        access_token,
        FriendActionRequest {
            name: Some(name),
            profile_id: None,
            update_type: action.as_str(),
        },
        true,
    )
    .await
}

async fn put_action(
    access_token: &str,
    body: FriendActionRequest<'_>,
    by_name: bool,
) -> Result<(), String> {
    let resp = http()
        .put(FRIENDS_URL)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Friend action failed: {e}"))?;
    check_status(&resp, by_name)
}

fn check_status(resp: &reqwest::Response, by_name: bool) -> Result<(), String> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    Err(match status.as_u16() {
        400 if by_name => "Unknown player name".to_string(),
        401 => "Session expired — sign in again".to_string(),
        403 => "Account has no active Java profile".to_string(),
        429 => "Rate limited — try again shortly".to_string(),
        s if s >= 500 => "Friends service unavailable, try again later".to_string(),
        s => format!("Friends service returned HTTP {s}"),
    })
}
