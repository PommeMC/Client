use std::sync::Arc;
use std::time::Duration;

use azalea_protocol::packets::game::s_chat_session_update::RemoteChatSessionData;
use base64::Engine;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::Pkcs1v15Sign;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::Deserialize;
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::net::commands::CommandTree;
use crate::net::sender::ChatMark;

const SERVICES_PUBLIC_KEYS_URL: &str = "https://api.minecraftservices.com/publickeys";
const PLAYER_CERTIFICATES_URL: &str = "https://api.minecraftservices.com/player/certificates";
const PROFILE_KEY_EXPIRY_GRACE_MS: u64 = 8 * 60 * 60 * 1000;
/// Vanilla `AccountProfileKeyPairManager.MINIMUM_PROFILE_KEY_REFRESH_INTERVAL`.
const MINIMUM_KEY_REFRESH_INTERVAL_MS: u64 = 60 * 60 * 1000;
const LAST_SEEN_CAPACITY: usize = 20;
const MAX_CHAT_LENGTH: usize = 256;
const MAX_ARGUMENT_NAME_LENGTH: usize = 16;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("could not build HTTP client: {e}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastSeenUpdate {
    pub offset: u32,
    pub acknowledged: [u8; 3],
    pub checksum: u8,
    pub last_seen: Vec<[u8; 256]>,
}

#[derive(Clone, Copy, Debug)]
struct TrackedMessage {
    signature: [u8; 256],
    pending: bool,
}

/// Vanilla `LastSeenMessagesTracker`.
#[derive(Clone, Debug)]
struct LastSeenTracker {
    tracked: [Option<TrackedMessage>; LAST_SEEN_CAPACITY],
    tail: usize,
    offset: u32,
    last_tracked: Option<[u8; 256]>,
}

impl Default for LastSeenTracker {
    fn default() -> Self {
        Self {
            tracked: [None; LAST_SEEN_CAPACITY],
            tail: 0,
            offset: 0,
            last_tracked: None,
        }
    }
}

impl LastSeenTracker {
    fn mark_processed(&mut self, signature: [u8; 256], shown: bool) -> Option<u32> {
        if self.last_tracked.as_ref() == Some(&signature) {
            return None;
        }
        self.last_tracked = Some(signature);
        let index = self.tail;
        self.tail = (self.tail + 1) % LAST_SEEN_CAPACITY;
        self.offset = self.offset.saturating_add(1);
        self.tracked[index] = shown.then_some(TrackedMessage {
            signature,
            pending: true,
        });
        (self.offset > 64).then(|| self.take_offset())
    }

    fn ignore_pending(&mut self, signature: &[u8; 256]) {
        for entry in &mut self.tracked {
            if entry
                .as_ref()
                .is_some_and(|entry| entry.pending && &entry.signature == signature)
            {
                *entry = None;
                break;
            }
        }
    }

    fn generate_update(&mut self) -> LastSeenUpdate {
        let offset = self.take_offset();
        let mut acknowledged = [0u8; 3];
        let mut last_seen = Vec::with_capacity(LAST_SEEN_CAPACITY);
        for i in 0..LAST_SEEN_CAPACITY {
            let index = (self.tail + i) % LAST_SEEN_CAPACITY;
            let Some(entry) = self.tracked[index].as_mut() else {
                continue;
            };
            acknowledged[i / 8] |= 1 << (i % 8);
            last_seen.push(entry.signature);
            entry.pending = false;
        }
        LastSeenUpdate {
            offset,
            acknowledged,
            checksum: last_seen_checksum(&last_seen),
            last_seen,
        }
    }

    fn take_offset(&mut self) -> u32 {
        std::mem::take(&mut self.offset)
    }
}

/// Vanilla `ProfileKeyPair`: the account's chat signing key and Mojang's
/// signature over its public half.
#[derive(Debug)]
pub struct ProfileKeyPair {
    private_key: RsaPrivateKey,
    public_key_der: Vec<u8>,
    key_signature: Vec<u8>,
    expires_at_ms: u64,
    refreshed_after_ms: u64,
}

impl PartialEq for ProfileKeyPair {
    fn eq(&self, other: &Self) -> bool {
        self.public_key_der == other.public_key_der
            && self.key_signature == other.key_signature
            && self.expires_at_ms == other.expires_at_ms
            && self.refreshed_after_ms == other.refreshed_after_ms
    }
}

#[derive(Debug, Deserialize)]
struct CertificatesResponse {
    #[serde(rename = "keyPair")]
    key_pair: KeyPairResponse,
    #[serde(rename = "publicKeySignatureV2")]
    public_key_signature_v2: String,
    #[serde(rename = "expiresAt")]
    expires_at: DateTime<Utc>,
    #[serde(rename = "refreshedAfter")]
    refreshed_after: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct KeyPairResponse {
    #[serde(rename = "privateKey")]
    private_key: String,
    #[serde(rename = "publicKey")]
    public_key: String,
}

impl ProfileKeyPair {
    async fn fetch(access_token: &str) -> Result<Self, String> {
        let response = http_client()?
            .post(PLAYER_CERTIFICATES_URL)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| format!("could not request player chat certificate: {e}"))?
            .error_for_status()
            .map_err(|e| format!("player certificate request failed: {e}"))?
            .json::<CertificatesResponse>()
            .await
            .map_err(|e| format!("player certificate response was malformed: {e}"))?;

        let private_der = decode_pem_body(&response.key_pair.private_key)?;
        let private_key = RsaPrivateKey::from_pkcs8_der(&private_der)
            .or_else(|_| RsaPrivateKey::from_pkcs1_der(&private_der))
            .map_err(|e| format!("player chat private key is malformed: {e}"))?;
        let public_key_der = decode_pem_body(&response.key_pair.public_key)?;
        RsaPublicKey::from_public_key_der(&public_key_der)
            .map_err(|e| format!("player chat public key is malformed: {e}"))?;
        let key_signature = base64::engine::general_purpose::STANDARD
            .decode(response.public_key_signature_v2.as_bytes())
            .map_err(|e| format!("player certificate signature is invalid base64: {e}"))?;
        Ok(Self {
            private_key,
            public_key_der,
            key_signature,
            expires_at_ms: response.expires_at.timestamp_millis().max(0) as u64,
            refreshed_after_ms: response.refreshed_after.timestamp_millis().max(0) as u64,
        })
    }

    fn due_refresh(&self, now_ms: u64) -> bool {
        now_ms > self.refreshed_after_ms
    }
}

/// Vanilla `AccountProfileKeyPairManager`: the key pair belongs to the
/// account, so it outlives connections.
struct KeyPairManager {
    account: Uuid,
    key_pair: Option<Arc<ProfileKeyPair>>,
    next_refresh_ms: u64,
    in_flight: usize,
}

static KEY_PAIRS: Mutex<KeyPairManager> = Mutex::new(KeyPairManager {
    account: Uuid::nil(),
    key_pair: None,
    next_refresh_ms: 0,
    in_flight: 0,
});

/// Runs fetches one after another, like vanilla's chained key pair future.
static KEY_PAIR_FETCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn key_pairs(account: Uuid) -> parking_lot::MutexGuard<'static, KeyPairManager> {
    let mut manager = KEY_PAIRS.lock();
    if manager.account != account {
        manager.account = account;
        manager.key_pair = None;
        manager.next_refresh_ms = 0;
    }
    manager
}

/// Vanilla `prepareKeyPair`: the cached pair unless it is due a refresh, else
/// a fetch that keeps the cached pair on failure. The result goes to `done`.
fn prepare_key_pair(
    account: Uuid,
    access_token: String,
    done: mpsc::UnboundedSender<Option<Arc<ProfileKeyPair>>>,
) {
    {
        let mut manager = key_pairs(account);
        manager.next_refresh_ms = now_ms() + MINIMUM_KEY_REFRESH_INTERVAL_MS;
        manager.in_flight += 1;
    }
    tokio::spawn(async move {
        let _chain = KEY_PAIR_FETCH.lock().await;
        let cached = key_pairs(account).key_pair.clone();
        let key_pair = match cached {
            Some(key_pair) if !key_pair.due_refresh(now_ms()) => Some(key_pair),
            cached => match ProfileKeyPair::fetch(&access_token).await {
                Ok(key_pair) => {
                    let key_pair = Arc::new(key_pair);
                    key_pairs(account).key_pair = Some(key_pair.clone());
                    Some(key_pair)
                }
                Err(error) => {
                    tracing::error!("Failed to retrieve profile key pair: {error}");
                    cached
                }
            },
        };
        key_pairs(account).in_flight -= 1;
        let _ = done.send(key_pair);
    });
}

/// Vanilla `shouldRefreshKeyPair`.
fn should_refresh_key_pair(account: Uuid, now_ms: u64) -> bool {
    let manager = key_pairs(account);
    manager.in_flight == 0
        && now_ms > manager.next_refresh_ms
        && manager
            .key_pair
            .as_ref()
            .is_none_or(|key_pair| key_pair.due_refresh(now_ms))
}

/// Vanilla `LocalChatSession` with its `SignedMessageChain` encoder.
struct LocalChatSession {
    session_id: Uuid,
    key_pair: Arc<ProfileKeyPair>,
    message_index: i32,
}

impl LocalChatSession {
    fn sign(
        &mut self,
        profile_id: Uuid,
        content: &str,
        timestamp_ms: i64,
        salt: i64,
        last_seen: &[[u8; 256]],
    ) -> Result<[u8; 256], String> {
        let index = self.message_index;
        self.message_index = self
            .message_index
            .checked_add(1)
            .ok_or("signed-chat message index overflowed")?;
        let payload = signed_payload(
            profile_id,
            self.session_id,
            index,
            &SignedBody {
                content,
                timestamp_ms,
                salt,
                last_seen,
            },
        )
        .ok_or("signed chat content is too large")?;
        let digest = Sha256::digest(&payload);
        let signature = self
            .key_pair
            .private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), digest.as_ref())
            .map_err(|e| format!("could not sign chat message: {e}"))?;
        signature.try_into().map_err(|signature: Vec<u8>| {
            format!("chat signature had {} bytes, expected 256", signature.len())
        })
    }
}

/// Vanilla `SignedMessageBody`'s signed fields.
struct SignedBody<'a> {
    content: &'a str,
    timestamp_ms: i64,
    salt: i64,
    last_seen: &'a [[u8; 256]],
}

/// The bytes vanilla `SignedMessageLink.updateSignature` and
/// `SignedMessageBody.updateSignature` feed the signature.
fn signed_payload(
    sender: Uuid,
    session_id: Uuid,
    index: i32,
    body: &SignedBody<'_>,
) -> Option<Vec<u8>> {
    let content = body.content.as_bytes();
    let mut payload = Vec::with_capacity(64 + content.len() + body.last_seen.len() * 256);
    payload.extend_from_slice(&1_i32.to_be_bytes());
    payload.extend_from_slice(sender.as_bytes());
    payload.extend_from_slice(session_id.as_bytes());
    payload.extend_from_slice(&index.to_be_bytes());
    payload.extend_from_slice(&body.salt.to_be_bytes());
    payload.extend_from_slice(&body.timestamp_ms.div_euclid(1000).to_be_bytes());
    payload.extend_from_slice(&i32::try_from(content.len()).ok()?.to_be_bytes());
    payload.extend_from_slice(content);
    payload.extend_from_slice(&i32::try_from(body.last_seen.len()).ok()?.to_be_bytes());
    for signature in body.last_seen {
        payload.extend_from_slice(signature);
    }
    Some(payload)
}

/// The outbound half of vanilla's secure chat, owned by the network loop so
/// chat input, processed and deleted marks apply in game-thread order like
/// `ClientPacketListener`'s single thread.
pub struct ChatSender {
    profile_id: Uuid,
    account_id: Uuid,
    access_token: Option<String>,
    session: Option<LocalChatSession>,
    last_seen: LastSeenTracker,
    key_pair_tx: mpsc::UnboundedSender<Option<Arc<ProfileKeyPair>>>,
}

impl ChatSender {
    pub fn new(
        profile_id: Uuid,
        account_id: Uuid,
        access_token: Option<String>,
        key_pair_tx: mpsc::UnboundedSender<Option<Arc<ProfileKeyPair>>>,
    ) -> Self {
        Self {
            profile_id,
            account_id,
            access_token,
            session: None,
            last_seen: LastSeenTracker::default(),
            key_pair_tx,
        }
    }

    /// Vanilla `handleLogin`: a fresh tracker, and a key pair when online.
    pub fn login(&mut self, online_mode: bool) {
        self.last_seen = LastSeenTracker::default();
        self.session = None;
        if online_mode {
            self.prepare_key_pair();
        }
    }

    /// Vanilla `ClientPacketListener.tick`'s key refresh.
    pub fn tick(&mut self) {
        if self.session.is_some() && should_refresh_key_pair(self.account_id, now_ms()) {
            self.prepare_key_pair();
        }
    }

    fn prepare_key_pair(&self) {
        if let Some(token) = self.access_token.clone() {
            prepare_key_pair(self.account_id, token, self.key_pair_tx.clone());
        }
    }

    /// Vanilla `setKeyPair`; returns the `chat_session_update` to send.
    pub fn key_pair_ready(&mut self, key_pair: Option<Arc<ProfileKeyPair>>) -> Option<Vec<u8>> {
        let key_pair = key_pair?;
        if self.profile_id != self.account_id
            || self
                .session
                .as_ref()
                .is_some_and(|session| *session.key_pair == *key_pair)
        {
            return None;
        }
        let session = LocalChatSession {
            session_id: Uuid::new_v4(),
            key_pair,
            message_index: 0,
        };
        let frame = super::chat::encode_chat_session_update(
            session.session_id,
            session.key_pair.expires_at_ms,
            &session.key_pair.public_key_der,
            &session.key_pair.key_signature,
        );
        self.session = Some(session);
        Some(frame)
    }

    /// Vanilla `markMessageAsProcessed` / `ignorePending`; returns a standalone
    /// ack when one is due.
    pub fn mark(&mut self, mark: ChatMark) -> Option<Vec<u8>> {
        match mark {
            ChatMark::Processed { signature, shown } => self
                .last_seen
                .mark_processed(signature, shown)
                .map(super::chat::encode_chat_ack),
            ChatMark::Deleted { signature } => {
                self.last_seen.ignore_pending(&signature);
                None
            }
        }
    }

    /// Vanilla `sendChatAcknowledgement`.
    pub fn flush_ack(&mut self) -> Option<Vec<u8>> {
        let offset = self.last_seen.take_offset();
        (offset > 0).then(|| super::chat::encode_chat_ack(offset))
    }

    /// Vanilla `sendChat` / `sendCommand`: one frame for typed chat input.
    pub fn encode_input(
        &mut self,
        input: &str,
        tree: Option<&CommandTree>,
    ) -> Result<Vec<u8>, String> {
        let timestamp = now_ms();
        let salt = rand::random::<i64>();
        let Some(command) = input.strip_prefix('/') else {
            if input.encode_utf16().count() > MAX_CHAT_LENGTH {
                return Err("chat message exceeds 256 UTF-16 code units".into());
            }
            let update = self.last_seen.generate_update();
            let signature = self.sign(input, timestamp, salt, &update.last_seen)?;
            return Ok(super::chat::encode_outbound_message(
                input,
                timestamp,
                salt,
                signature.as_ref(),
                &update,
            ));
        };
        let arguments = tree
            .map(|tree| tree.signable_arguments(command))
            .unwrap_or_default();
        if arguments.is_empty() {
            return Ok(super::chat::encode_outbound_command(command));
        }
        if arguments
            .iter()
            .any(|(name, _)| name.chars().count() > MAX_ARGUMENT_NAME_LENGTH)
        {
            return Err("signed command argument name exceeds 16 characters".into());
        }
        let update = self.last_seen.generate_update();
        let mut signatures = Vec::with_capacity(arguments.len());
        for (name, value) in arguments {
            if let Some(signature) = self.sign(&value, timestamp, salt, &update.last_seen)? {
                signatures.push((name, signature));
            }
        }
        Ok(super::chat::encode_outbound_signed_command(
            command,
            timestamp,
            salt,
            &signatures,
            &update,
        ))
    }

    /// Vanilla `SignedMessageChain.Encoder`: nothing without a session.
    fn sign(
        &mut self,
        content: &str,
        timestamp_ms: u64,
        salt: i64,
        last_seen: &[[u8; 256]],
    ) -> Result<Option<[u8; 256]>, String> {
        let profile_id = self.profile_id;
        self.session
            .as_mut()
            .map(|session| session.sign(profile_id, content, timestamp_ms as i64, salt, last_seen))
            .transpose()
    }
}

#[derive(Clone, Debug)]
pub struct ValidatedChatSession {
    pub session_id: Uuid,
    pub expires_at_ms: u64,
    pub public_key: RsaPublicKey,
}

impl ValidatedChatSession {
    pub fn expired_with_grace(&self, now_ms: u64) -> bool {
        now_ms
            > self
                .expires_at_ms
                .saturating_add(PROFILE_KEY_EXPIRY_GRACE_MS)
    }
}

#[derive(Clone, Debug)]
pub struct SignedChatBody {
    pub content: String,
    pub timestamp_ms: i64,
    pub salt: i64,
    pub last_seen: Vec<[u8; 256]>,
    pub message_index: i32,
    pub modified: bool,
    /// Vanilla trust level when `onlyShowSecureChat` removes unsigned text.
    /// A non-default font on unsigned content still marks the message modified,
    /// but a text-only unsigned replacement no longer does.
    pub modified_when_unsigned_hidden: bool,
    pub fully_filtered: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileKeyServices {
    keys: Vec<RsaPublicKey>,
}

enum ServicesState {
    NotStarted,
    Loading,
    Loaded(Option<Arc<ProfileKeyServices>>),
}

/// Vanilla fetches the services keys once, at startup.
static SERVICES: Mutex<ServicesState> = Mutex::new(ServicesState::NotStarted);

impl ProfileKeyServices {
    /// Starts the one fetch this process makes.
    pub fn prefetch() {
        {
            let mut state = SERVICES.lock();
            if !matches!(*state, ServicesState::NotStarted) {
                return;
            }
            *state = ServicesState::Loading;
        }
        tokio::spawn(async {
            let services = match Self::fetch().await {
                Ok(services) => Some(Arc::new(services)),
                Err(error) => {
                    tracing::warn!("Could not load Mojang profile-key services: {error}");
                    None
                }
            };
            *SERVICES.lock() = ServicesState::Loaded(services);
        });
    }

    /// The services keys, once loaded (vanilla
    /// `getProfileKeySignatureValidator`).
    pub fn get() -> Option<Arc<Self>> {
        match &*SERVICES.lock() {
            ServicesState::Loaded(services) => services.clone(),
            _ => None,
        }
    }

    async fn fetch() -> Result<Self, String> {
        let value: Value = http_client()?
            .get(SERVICES_PUBLIC_KEYS_URL)
            .send()
            .await
            .map_err(|e| format!("could not request Mojang services public keys: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Mojang services public-key request failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Mojang services public-key response was malformed: {e}"))?;
        Self::from_response_json(&value)
    }

    fn from_response_json(value: &Value) -> Result<Self, String> {
        let entries = value
            .get("playerCertificateKeys")
            .and_then(Value::as_array)
            .ok_or_else(|| "Mojang public-key response had no playerCertificateKeys".to_owned())?;
        let keys: Vec<_> = entries
            .iter()
            .filter_map(|entry| entry.get("publicKey")?.as_str())
            .filter_map(|pem| decode_rsa_public_key_pem(pem).ok())
            .collect();
        if keys.is_empty() {
            return Err(
                "Mojang public-key response contained no usable player certificate keys".to_owned(),
            );
        }
        Ok(Self { keys })
    }

    pub fn validate_session(
        &self,
        profile_id: Uuid,
        data: &RemoteChatSessionData,
    ) -> Result<ValidatedChatSession, String> {
        let key = RsaPublicKey::from_public_key_der(&data.profile_public_key.key)
            .map_err(|e| format!("player profile public key is malformed: {e}"))?;

        let mut payload = Vec::with_capacity(24 + data.profile_public_key.key.len());
        payload.extend_from_slice(profile_id.as_bytes());
        payload.extend_from_slice(&data.profile_public_key.expires_at.to_be_bytes());
        payload.extend_from_slice(&data.profile_public_key.key);

        let digest = Sha1::digest(&payload);
        let valid = self.keys.iter().any(|service_key| {
            service_key
                .verify(
                    Pkcs1v15Sign::new::<Sha1>(),
                    &digest,
                    &data.profile_public_key.key_signature,
                )
                .is_ok()
        });
        if !valid {
            return Err(
                "profile public key signature did not validate against Mojang services keys"
                    .to_owned(),
            );
        }

        Ok(ValidatedChatSession {
            session_id: data.session_id,
            expires_at_ms: data.profile_public_key.expires_at,
            public_key: key,
        })
    }
}

pub fn verify_player_message(
    session: &ValidatedChatSession,
    sender: Uuid,
    body: &SignedChatBody,
    signature: &[u8; 256],
) -> bool {
    let Some(payload) = signed_payload(
        sender,
        session.session_id,
        body.message_index,
        &SignedBody {
            content: &body.content,
            timestamp_ms: body.timestamp_ms,
            salt: body.salt,
            last_seen: &body.last_seen,
        },
    ) else {
        return false;
    };
    let digest = Sha256::digest(&payload);
    session
        .public_key
        .verify(Pkcs1v15Sign::new::<Sha256>(), digest.as_ref(), signature)
        .is_ok()
}

fn last_seen_checksum(last_seen: &[[u8; 256]]) -> u8 {
    let mut checksum: i32 = 1;
    for signature in last_seen {
        let mut signature_hash: i32 = 1;
        for byte in signature {
            signature_hash = signature_hash
                .wrapping_mul(31)
                .wrapping_add(i32::from(*byte as i8));
        }
        checksum = checksum.wrapping_mul(31).wrapping_add(signature_hash);
    }
    let value = checksum as u8;
    if value == 0 { 1 } else { value }
}

fn decode_pem_body(pem: &str) -> Result<Vec<u8>, String> {
    let encoded: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|e| format!("invalid PEM base64: {e}"))
}

fn decode_rsa_public_key_pem(pem: &str) -> Result<RsaPublicKey, String> {
    let der = decode_pem_body(pem)?;
    RsaPublicKey::from_public_key_der(&der)
        .map_err(|e| format!("invalid services RSA public key: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_seen_checksum_matches_java_arrays_hash_code_fold() {
        let zero = [0u8; 256];
        let range = std::array::from_fn::<u8, 256, _>(|i| i as u8);
        assert_eq!(last_seen_checksum(&[zero]), 32);
        assert_eq!(last_seen_checksum(&[range]), 160);
    }

    #[test]
    fn last_seen_tracker_matches_vanilla_ring_and_bitset_order() {
        let shown = [1u8; 256];
        let hidden = [2u8; 256];
        let mut tracker = LastSeenTracker::default();
        assert_eq!(tracker.mark_processed(shown, true), None);
        assert_eq!(tracker.mark_processed(hidden, false), None);

        let update = tracker.generate_update();
        assert_eq!(update.offset, 2);
        // tail == 2, so slot 0 appears at logical bit 18; slot 1 was hidden
        // and therefore contributes no acknowledgement bit or last-seen entry.
        assert_eq!(update.acknowledged, [0, 0, 0b0000_0100]);
        assert_eq!(update.last_seen, vec![shown]);
        assert_eq!(update.checksum, 32);
    }

    #[test]
    fn last_seen_tracker_suppresses_duplicates_and_pending_deletes() {
        let signature = [3u8; 256];
        let mut tracker = LastSeenTracker::default();
        assert_eq!(tracker.mark_processed(signature, true), None);
        assert_eq!(tracker.mark_processed(signature, true), None);
        tracker.ignore_pending(&signature);
        let update = tracker.generate_update();
        assert_eq!(update.offset, 1);
        assert_eq!(update.acknowledged, [0, 0, 0]);
        assert!(update.last_seen.is_empty());
        assert_eq!(update.checksum, 1);
    }

    #[test]
    fn last_seen_tracker_requests_standalone_ack_after_sixty_four() {
        let mut tracker = LastSeenTracker::default();
        for i in 0..64u8 {
            assert_eq!(tracker.mark_processed([i; 256], false), None);
        }
        assert_eq!(tracker.mark_processed([64; 256], false), Some(65));
        // The standalone ack clears only the offset; the ring state remains.
        assert_eq!(tracker.generate_update().offset, 0);
    }

    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDTf07dTJsDSTk0
eV80dVE1I6g+20qbXNLaqmH2nszYkLt2p9lpwyhs4I8QayJ0fYtKXgH5ayFxy/79
y0zcE4HrRvAM2CvNYnG/j3V17syQHfRWXKF+PshuIpI/lTNYSME0VWymGK66Yeya
ooUSjytn1Bu5k+zmOMZ5v1FJ6SJtJ3PCPbRGrHPzvAphGplvwel3UqZ9UtulDkib
DlBC1qwPp+U6sNZpYHEw7YxN0xseldtESiqwzUL9/6PJ17SQwi08K0GwDbpORZT4
3WhRSyzwbe3bV00gUqZEuTPWQ38gnpJz0/rO9/khtBPjNun0EYzDhtzKgxs69CI3
V5NNedhBAgMBAAECggEAAg+rBmMF8uS1QQ61Qog/AJJJREU5b1/wAfei4THNQTI7
kiWYNndeJvMoex0bg/s0vtvsGkDkvayNVBkgVcKUE7BNwYb0+f6S0/kl6Ik0IcFE
UDf9JBiLIMVSEPqnOlMphzUVbDLQTMqh8Q8IK7pXjpPy6g6NYqAsnANlBgrBU4MS
6ovMwgtoA/4x9lEIoxTD10iSl1jmAl2ibXhicu+M1sEoRC3ScwyNPb6BvGPs/1pY
KbJqVATwj//x4iLfZhUGaoTLtLUpFlaBu8dpxx+Wid6whkWS9CnK6QpneHjKLqBx
Ys+d3MY0dywT02LziJHupDOeJ1RN1gpTNs8R1hfGKQKBgQD4IFlGPiT3l3Zfo9ny
wxOsD61uqI+iqyXUJ09b0Ff0TMwi10KhcNo59D1Ne/wp1ifzRI/s36TP27SbwTGb
ssNvSU7yT1IZdhjzJHnLDRDg7zIXvK4L6zzhagw+uaVu4MyUJ8yRWA2W/Lx1bBC9
moUr9wvu7BMKGUT++3ygTvnu+QKBgQDaNWdShfu/EZZMMZsNVfYh/y+QaJkx9NLY
C2kUngzHK58UGiWxSjjX8WuyxzheCnv2LX0TG2JM6hOiC6cI+EdSkH520qIGe8ia
CG97c0a9lY7603QKWgteWOvYuILhXxVXQh/kWy7CQRLO3GA9NWfi9Tm0l1PtY2tD
+ss1mMzdiQKBgB7AE5BK/1XX5Ymwyr/1QSjfwISoSzTDtSp3vLQKO/xA0EO5Hb7Y
N5NbG4XQyc19hvH1G0kl5k0EU3vCE53SJ7pRAYGyJuCU7D6l1Jo/gkn+Gt0qOv+r
JZ5iACZ952y4W2I5FHcmzHhb1hdPTzvQPJTYRxhTFYD45L4c+LL9VqgxAoGAZKRq
8knvsdGXw67Bd+Yk7ss3EeDcf4kO0ix5G9RFynsZFPl2Vw4Hp7mm1b9DBUTKpeGX
JX/k19rCkWPUd7OjmbYhTgaaSmk/PaQUXxjtELXxS0jJ5ZhgU/SpWrzHSNFFE4jh
Er7nkxrWZOiJztFaB/jY061UPVI0gBclMKQ4IRkCgYEA9VJQLRiyx+14WrAI8kM9
WWxbR6MxwAVFIRvKKmrRsn1foQA6zzhEmW7ITrNZBF6IxtRsklmmj/LWdQxUjEVg
vnqdH6NeFCrbjgv1jBEiJNUeGFyDgTKM9xHqreA755FxIRe7Xha6eeIKR8b8JKfj
L59jqlQpPBBT3EAbN66KEao=
-----END PRIVATE KEY-----";

    fn test_key_pair() -> Arc<ProfileKeyPair> {
        let der = decode_pem_body(TEST_KEY).unwrap();
        Arc::new(ProfileKeyPair {
            private_key: RsaPrivateKey::from_pkcs8_der(&der).unwrap(),
            public_key_der: vec![1, 2, 3],
            key_signature: vec![4, 5],
            expires_at_ms: 1_000,
            refreshed_after_ms: 500,
        })
    }

    #[test]
    fn signed_message_verifies_against_its_public_key() {
        let key_pair = test_key_pair();
        let mut session = LocalChatSession {
            session_id: Uuid::from_u128(7),
            key_pair: key_pair.clone(),
            message_index: 3,
        };
        let sender = Uuid::from_u128(9);
        let last_seen = [[5u8; 256]];
        let signature = session.sign(sender, "hi", 12_345, -4, &last_seen).unwrap();
        assert_eq!(session.message_index, 4);

        let remote = ValidatedChatSession {
            session_id: session.session_id,
            expires_at_ms: 0,
            public_key: RsaPublicKey::from(&key_pair.private_key),
        };
        let mut body = SignedChatBody {
            content: "hi".into(),
            timestamp_ms: 12_345,
            salt: -4,
            last_seen: last_seen.to_vec(),
            message_index: 3,
            modified: false,
            modified_when_unsigned_hidden: false,
            fully_filtered: false,
        };
        assert!(verify_player_message(&remote, sender, &body, &signature));
        body.message_index = 4;
        assert!(!verify_player_message(&remote, sender, &body, &signature));
    }

    #[test]
    fn chat_sender_signs_only_with_a_session_and_checks_length_first() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let account = Uuid::from_u128(1);
        let mut sender = ChatSender::new(account, account, None, tx);
        assert!(sender.encode_input(&"😀".repeat(129), None).is_err());
        assert!(sender.key_pair_ready(Some(test_key_pair())).is_some());
        assert!(sender.encode_input(&"😀".repeat(129), None).is_err());
        assert_eq!(sender.session.as_ref().unwrap().message_index, 0);
        sender.encode_input("hello", None).unwrap();
        assert_eq!(sender.session.as_ref().unwrap().message_index, 1);
    }

    #[test]
    fn same_key_pair_keeps_the_session_and_foreign_profiles_never_sign() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let key_pair = test_key_pair();
        let mut sender = ChatSender::new(Uuid::from_u128(1), Uuid::from_u128(1), None, tx.clone());
        assert!(sender.key_pair_ready(Some(key_pair.clone())).is_some());
        assert!(sender.key_pair_ready(Some(key_pair)).is_none());

        let mut foreign = ChatSender::new(Uuid::from_u128(2), Uuid::from_u128(1), None, tx);
        assert!(foreign.key_pair_ready(Some(test_key_pair())).is_none());
    }
}
