use std::time::Duration;

use azalea_protocol::packets::game::s_chat_session_update::RemoteChatSessionData;
use base64::Engine;
use chrono::{DateTime, Utc};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::Pkcs1v15Sign;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::Deserialize;
use serde_json::Value;
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use uuid::Uuid;

const SERVICES_PUBLIC_KEYS_URL: &str = "https://api.minecraftservices.com/publickeys";
const PLAYER_CERTIFICATES_URL: &str = "https://api.minecraftservices.com/player/certificates";
const PROFILE_KEY_EXPIRY_GRACE_MS: u64 = 8 * 60 * 60 * 1000;
const LAST_SEEN_CAPACITY: usize = 20;

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

#[derive(Clone, Debug)]
pub struct LastSeenTracker {
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
    pub fn mark_processed(&mut self, signature: [u8; 256], shown: bool) -> Option<u32> {
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

    pub fn ignore_pending(&mut self, signature: &[u8; 256]) {
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

    pub fn generate_update(&mut self) -> LastSeenUpdate {
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
        let offset = self.offset;
        self.offset = 0;
        offset
    }
}

#[derive(Clone, Debug)]
pub struct LocalChatSession {
    pub session_id: Uuid,
    pub expires_at_ms: u64,
    pub refresh_after_ms: u64,
    pub public_key_der: Vec<u8>,
    pub key_signature: Vec<u8>,
    private_key: RsaPrivateKey,
    message_index: u32,
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

impl LocalChatSession {
    pub async fn fetch(access_token: &str) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| format!("could not build certificate HTTP client: {e}"))?;
        let response = client
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
        // Validate that the returned public key is structurally valid. The
        // private key still owns the signing operation, matching Vanilla's
        // ProfileKeyPair.
        RsaPublicKey::from_public_key_der(&public_key_der)
            .map_err(|e| format!("player chat public key is malformed: {e}"))?;
        let key_signature = base64::engine::general_purpose::STANDARD
            .decode(response.public_key_signature_v2.as_bytes())
            .map_err(|e| format!("player certificate signature is invalid base64: {e}"))?;

        let expires_at_ms = response.expires_at.timestamp_millis().max(0) as u64;
        let refresh_after_ms = response.refreshed_after.timestamp_millis().max(0) as u64;
        Ok(Self {
            session_id: Uuid::new_v4(),
            expires_at_ms,
            refresh_after_ms,
            public_key_der,
            key_signature,
            private_key,
            message_index: 0,
        })
    }

    pub fn should_refresh(&self, now_ms: u64) -> bool {
        now_ms >= self.refresh_after_ms
    }

    pub fn renew_session(&mut self) {
        self.session_id = Uuid::new_v4();
        self.message_index = 0;
    }

    pub fn sign_body(
        &mut self,
        profile_id: Uuid,
        content: &str,
        timestamp_ms: i64,
        salt: i64,
        last_seen: &[[u8; 256]],
    ) -> Result<[u8; 256], String> {
        let message_index = i32::try_from(self.message_index)
            .map_err(|_| "signed-chat message index overflowed i32".to_owned())?;
        self.message_index = self.message_index.wrapping_add(1);

        let mut payload = Vec::with_capacity(64 + content.len() + last_seen.len() * 256);
        payload.extend_from_slice(&1_i32.to_be_bytes());
        payload.extend_from_slice(profile_id.as_bytes());
        payload.extend_from_slice(self.session_id.as_bytes());
        payload.extend_from_slice(&message_index.to_be_bytes());
        payload.extend_from_slice(&salt.to_be_bytes());
        payload.extend_from_slice(&timestamp_ms.div_euclid(1000).to_be_bytes());
        let content_bytes = content.as_bytes();
        let content_len = i32::try_from(content_bytes.len())
            .map_err(|_| "signed chat content is too large".to_owned())?;
        payload.extend_from_slice(&content_len.to_be_bytes());
        payload.extend_from_slice(content_bytes);
        let last_seen_len = i32::try_from(last_seen.len())
            .map_err(|_| "too many last-seen signatures".to_owned())?;
        payload.extend_from_slice(&last_seen_len.to_be_bytes());
        for signature in last_seen {
            payload.extend_from_slice(signature);
        }

        let digest = Sha256::digest(&payload);
        let signature = self
            .private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), digest.as_ref())
            .map_err(|e| format!("could not sign chat message: {e}"))?;
        signature.try_into().map_err(|signature: Vec<u8>| {
            format!("chat signature had {} bytes, expected 256", signature.len())
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChatOutboundState {
    pub session: Option<LocalChatSession>,
    pub last_seen: LastSeenTracker,
    pub signing_enabled: bool,
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

impl ProfileKeyServices {
    pub async fn fetch() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| format!("could not build profile-key HTTP client: {e}"))?;
        let response = client
            .get(SERVICES_PUBLIC_KEYS_URL)
            .send()
            .await
            .map_err(|e| format!("could not request Mojang services public keys: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Mojang services public-key request failed: {e}"))?;
        let value: Value = response
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
        let mut keys = Vec::new();
        for entry in entries {
            let Some(pem) = entry.get("publicKey").and_then(Value::as_str) else {
                continue;
            };
            if let Ok(key) = decode_rsa_public_key_pem(pem) {
                keys.push(key);
            }
        }
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
                    digest.as_ref(),
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
    let mut payload = Vec::with_capacity(64 + body.content.len() + body.last_seen.len() * 256);
    payload.extend_from_slice(&1_i32.to_be_bytes());
    payload.extend_from_slice(sender.as_bytes());
    payload.extend_from_slice(session.session_id.as_bytes());
    payload.extend_from_slice(&body.message_index.to_be_bytes());
    payload.extend_from_slice(&body.salt.to_be_bytes());
    payload.extend_from_slice(&body.timestamp_ms.div_euclid(1000).to_be_bytes());
    let content = body.content.as_bytes();
    let Ok(content_len) = i32::try_from(content.len()) else {
        return false;
    };
    payload.extend_from_slice(&content_len.to_be_bytes());
    payload.extend_from_slice(content);
    let Ok(last_seen_len) = i32::try_from(body.last_seen.len()) else {
        return false;
    };
    payload.extend_from_slice(&last_seen_len.to_be_bytes());
    for entry in &body.last_seen {
        payload.extend_from_slice(entry);
    }

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
    fn expiry_grace_matches_vanilla_eight_hours() {
        assert_eq!(PROFILE_KEY_EXPIRY_GRACE_MS, 28_800_000);
    }

    #[test]
    fn signed_body_timestamp_uses_epoch_seconds() {
        let body = SignedChatBody {
            content: "hi".into(),
            timestamp_ms: 1_234_567,
            salt: 0x0102_0304_0506_0708,
            last_seen: Vec::new(),
            message_index: 7,
            modified: false,
            modified_when_unsigned_hidden: false,
            fully_filtered: false,
        };
        assert_eq!(body.timestamp_ms.div_euclid(1000), 1_234);
    }

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
}
