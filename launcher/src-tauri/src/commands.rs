use crate::installations::{Installation, InstallationDraft, InstallationError};
use crate::settings::LauncherSettings;
use crate::{AppState, installations, storage};

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

#[derive(Deserialize)]
struct MojangPatchNotes {
    entries: Vec<MojangEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MojangEntry {
    title: String,
    version: String,
    date: String,
    short_text: String,
    image: MojangImage,
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(rename = "contentPath")]
    content_path: String,
}

#[derive(Deserialize)]
struct MojangImage {
    url: String,
}

#[derive(Deserialize)]
struct MojangContent {
    body: String,
}

#[derive(Serialize)]
pub struct PatchNote {
    pub title: String,
    pub version: String,
    pub date: String,
    pub summary: String,
    pub image_url: String,
    pub entry_type: String,
    pub content_path: String,
}

const PATCH_NOTES_URL: &str = "https://launchercontent.mojang.com/v2/javaPatchNotes.json";
const IMAGE_BASE: &str = "https://launchercontent.mojang.com";

#[derive(Clone, Serialize)]
enum ConsoleEventType {
    #[serde(rename = "message")]
    Message,
    #[serde(rename = "reset")]
    Reset,
}

#[derive(Clone, Serialize)]
struct ConsoleEvent {
    #[serde(rename = "type")]
    pub message_type: ConsoleEventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub val: Option<String>,
}

#[tauri::command]
pub async fn get_patch_notes(count: Option<usize>) -> Result<Vec<PatchNote>, String> {
    let limit = count.unwrap_or(20);
    let resp: MojangPatchNotes = reqwest::get(PATCH_NOTES_URL)
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let mut entries = resp.entries;
    entries.sort_unstable_by(|a, b| b.date.cmp(&a.date));

    Ok(entries
        .into_iter()
        .take(limit)
        .map(|e| PatchNote {
            title: e.title,
            date: e.date.chars().take(10).collect(),
            summary: e.short_text,
            image_url: format!("{IMAGE_BASE}{}", e.image.url),
            entry_type: e.entry_type,
            content_path: e.content_path,
            version: e.version,
        })
        .collect())
}

#[tauri::command]
pub async fn get_patch_content(content_path: String) -> Result<String, String> {
    let url = format!("{IMAGE_BASE}/v2/{content_path}");
    let content: MojangContent = reqwest::get(&url)
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(content.body)
}

#[derive(Deserialize)]
struct SessionProfile {
    properties: Vec<SessionProperty>,
}

#[derive(Deserialize)]
struct SessionProperty {
    value: String,
}

#[derive(Deserialize)]
struct TexturesPayload {
    textures: Textures,
}

#[derive(Deserialize)]
struct Textures {
    #[serde(rename = "SKIN")]
    skin: Option<SkinTexture>,
}

#[derive(Deserialize)]
struct SkinTexture {
    url: String,
}

#[tauri::command]
pub async fn get_skin_url(uuid: String) -> Result<String, String> {
    let clean_uuid = uuid.replace('-', "");
    let url = format!("https://sessionserver.mojang.com/session/minecraft/profile/{clean_uuid}");
    let profile: SessionProfile = reqwest::get(&url)
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let value = &profile.properties.first().ok_or("No properties")?.value;

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|e| e.to_string())?;
    let payload: TexturesPayload = serde_json::from_slice(&decoded).map_err(|e| e.to_string())?;

    payload
        .textures
        .skin
        .map(|s| s.url)
        .ok_or_else(|| "No skin texture".to_string())
}

#[tauri::command]
pub fn get_all_accounts() -> Vec<crate::auth::AuthAccount> {
    crate::auth::get_all_accounts()
}

#[tauri::command]
pub async fn add_account() -> Result<crate::auth::AuthAccount, String> {
    crate::auth::oauth_sign_in().await
}

#[tauri::command]
pub fn remove_account(uuid: String) {
    crate::auth::remove_account(&uuid);
}

#[derive(Deserialize)]
struct VersionManifest {
    latest: LatestVersions,
    versions: Vec<VersionEntry>,
}

#[derive(Deserialize)]
struct VersionEntry {
    id: String,
    #[serde(rename = "type")]
    version_type: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Serialize)]
pub struct Versions {
    pub latest: LatestVersions,
    pub versions: Vec<GameVersion>,
}

#[derive(Serialize, Clone)]
pub struct GameVersion {
    pub id: String,
    pub version_type: String,
}

static VERSION_CACHE: std::sync::OnceLock<Versions> = std::sync::OnceLock::new();

pub async fn fetch_versions() -> Result<&'static Versions, String> {
    if let Some(cached) = VERSION_CACHE.get() {
        return Ok(cached);
    }

    let manifest: VersionManifest =
        reqwest::get("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

    let versions: Vec<GameVersion> = manifest
        .versions
        .into_iter()
        .map(|v| GameVersion {
            id: v.id,
            version_type: v.version_type,
        })
        .collect();

    let latest: LatestVersions = manifest.latest;

    Ok(VERSION_CACHE.get_or_init(|| Versions { latest, versions }))
}

#[tauri::command]
pub async fn get_versions(show_snapshots: Option<bool>) -> Result<Vec<GameVersion>, String> {
    let all = fetch_versions().await?;
    let include_snapshots = show_snapshots.unwrap_or(false);
    Ok(all
        .versions
        .iter()
        .filter(|v| include_snapshots || v.version_type == "release")
        .cloned()
        .collect())
}

#[tauri::command]
pub async fn refresh_account(uuid: String) -> Result<crate::auth::AuthAccount, String> {
    crate::auth::try_restore_or_refresh(&uuid)
        .await
        .ok_or_else(|| "Failed to refresh account".to_string())
}

#[tauri::command]
pub async fn ensure_assets(app: AppHandle, version: String) -> Result<(), String> {
    if crate::downloader::needs_download(&version) {
        crate::downloader::download(&app, &version).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_downloaded_versions() -> Vec<String> {
    crate::downloader::get_downloaded_versions().await
}

#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    version: String,
    uuid: Option<String>,
    server: Option<String>,
    debug_enabled: Option<bool>,
    install_path: Option<String>,
) -> Result<String, String> {
    let exe = find_client_binary()?;
    let account = uuid.as_deref().and_then(crate::auth::try_restore);
    let username = account
        .as_ref()
        .map(|a| a.username.clone())
        .unwrap_or_else(|| "Steve".into());

    let token: String = (0..32)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    let token_path = std::env::temp_dir().join("pomme_launch_token");
    std::fs::write(&token_path, &token).map_err(|e| e.to_string())?;

    let mut cmd = tokio::process::Command::new(&exe);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if debug_enabled.unwrap_or(false) {
        cmd.env("RUST_LOG", "debug");
        cmd.env("RUST_BACKTRACE", "full");
        match app.webview_windows().get("console") {
            None => {
                WebviewWindowBuilder::new(&app, "console", WebviewUrl::App("console".into()))
                    .title("Pomme Debugger")
                    .decorations(false)
                    .build()
                    .unwrap();
            }
            Some(window) => {
                let _ = app
                    .emit(
                        "console_message",
                        ConsoleEvent {
                            message_type: ConsoleEventType::Reset,
                            val: None,
                        },
                    )
                    .map_err(|e| e.to_string());
                window.set_focus().expect("failed to focus window");
            }
        }
    }

    cmd.arg("--version")
        .arg(&version)
        .arg("--username")
        .arg(&username)
        .arg("--assets-dir")
        .arg(storage::assets_dir().to_string_lossy().as_ref())
        .arg("--versions-dir")
        .arg(storage::versions_dir().to_string_lossy().as_ref())
        .arg("--launch-token")
        .arg(token_path.to_string_lossy().as_ref())
        .arg("--game-dir")
        .arg(install_path.unwrap_or_else(|| {
            storage::installations_dir()
                .join("default")
                .to_string_lossy()
                .into_owned()
        }));

    if let Some(acc) = &account {
        cmd.arg("--uuid")
            .arg(&acc.uuid)
            .arg("--access-token")
            .arg(&acc.access_token);
    }
    if let Some(server) = &server {
        cmd.arg("--quick-access-server").arg(server);
    }

    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    let stdout = child.stdout.take().expect("couldn't take stdout");
    let stderr = child.stderr.take().expect("couldn't take stderr");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let tx2 = tx.clone();

    let last_error_line = Arc::new(Mutex::new(None::<String>));

    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx.send(line);
        }
    });

    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx2.send(line);
        }
    });

    let app_emitter = app.clone();
    let app_handle = app.clone();
    let last_error_writer = last_error_line.clone();
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            let _ = app_emitter.emit(
                "console_message",
                ConsoleEvent {
                    message_type: ConsoleEventType::Message,
                    val: Some(line.clone()),
                },
            );
            let state = app_handle.state::<AppState>();
            let mut logs = state.client_logs.lock().await;
            logs.push_back(line.clone());
            if logs.len() > 10_000 {
                logs.pop_front();
            }
            if line.contains(" ERROR ") {
                *last_error_writer.lock().await = Some(line);
            }
        }
    });

    let app_handle = app.clone();
    tokio::spawn(async move {
        let status = child
            .wait()
            .await
            .expect("client process encountered an error");

        if !status.success() {
            let last = last_error_line.lock().await.clone();

            #[cfg(unix)]
            let signal = status.signal();
            #[cfg(not(unix))]
            let signal: Option<i32> = None;

            let _ = app_handle.emit(
                "game_exited",
                serde_json::json!({
                    "code": status.code(),
                    "signal": signal,
                    "last_line": last,
                }),
            );
        }

        println!("client status was: {}", status);
    });

    Ok(format!("Launched as {username}"))
}

#[tauri::command]
pub async fn get_client_logs(state: State<'_, AppState>) -> Result<VecDeque<String>, ()> {
    let logs = state.client_logs.lock().await;
    Ok(logs.clone())
}

fn find_client_binary() -> Result<std::path::PathBuf, String> {
    #[cfg(target_family = "windows")]
    const EXENAME: &str = "pomme.exe";

    #[cfg(target_family = "unix")]
    const EXENAME: &str = "pomme";

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let same_dir = dir.join(EXENAME);
        if same_dir.exists() {
            return Ok(same_dir);
        }

        let mut ancestor = dir.to_path_buf();
        for _ in 0..6 {
            if !ancestor.pop() {
                break;
            }

            #[cfg(debug_assertions)]
            let profiles = ["debug", "release"];
            #[cfg(not(debug_assertions))]
            let profiles = ["release", "debug"];

            for profile in profiles {
                let candidate = ancestor.join("target").join(profile).join(EXENAME);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err("Pomme client not found. It will be bundled in future releases.".into())
}

#[tauri::command]
pub async fn load_launcher_settings() -> LauncherSettings {
    let settings = LauncherSettings::get().await;
    settings.clone()
}

#[tauri::command]
pub async fn set_launcher_language(language: String) -> Result<(), String> {
    LauncherSettings::update(|s| s.language = language).await
}

#[tauri::command]
pub async fn set_keep_launcher_open(keep: bool) -> Result<(), String> {
    LauncherSettings::update(|s| s.keep_launcher_open = keep).await
}

#[tauri::command]
pub async fn set_launch_with_console(launch: bool) -> Result<(), String> {
    LauncherSettings::update(|s| s.launch_with_console = launch).await
}

#[tauri::command]
pub async fn ping_server(address: String) -> crate::ping::ServerStatus {
    crate::ping::ping_server(&address).await
}

#[tauri::command]
pub async fn load_servers() -> Vec<crate::ping::SavedServer> {
    let path = servers_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub async fn save_servers(servers: Vec<crate::ping::SavedServer>) -> Result<(), String> {
    let path = servers_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let json = serde_json::to_string_pretty(&servers).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

fn servers_path() -> std::path::PathBuf {
    storage::installations_dir()
        .join("default")
        .join("servers.json")
}

#[tauri::command]
pub async fn load_installations(
    state: State<'_, AppState>,
) -> Result<Vec<Installation>, InstallationError> {
    let _guard = state.installations_lock.lock().await;
    installations::load_installations().await
}

#[tauri::command]
pub async fn create_installation(
    state: State<'_, AppState>,
    payload: InstallationDraft,
) -> Result<Installation, InstallationError> {
    let _guard = state.installations_lock.lock().await;
    installations::create_installation(payload).await
}

#[tauri::command]
pub async fn delete_installation(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), InstallationError> {
    let _guard = state.installations_lock.lock().await;
    installations::delete_installation(id).await
}

#[tauri::command]
pub async fn duplicate_installation(
    state: State<'_, AppState>,
    old_id: String,
    payload: InstallationDraft,
) -> Result<Installation, InstallationError> {
    let _guard = state.installations_lock.lock().await;
    installations::duplicate_installation(old_id, payload).await
}

#[tauri::command]
pub async fn edit_installation(
    state: State<'_, AppState>,
    id: String,
    payload: InstallationDraft,
) -> Result<Installation, InstallationError> {
    let _guard = state.installations_lock.lock().await;
    installations::edit_installation(id, payload).await
}
