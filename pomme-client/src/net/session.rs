//! Telling Mojang's session server we're joining an online-mode server, as
//! vanilla `ClientHandshakePacketListenerImpl.authenticateServer` does.

use sha1::{Digest, Sha1};

/// Vanilla `new BigInteger(Crypt.digestData(serverId, publicKey, secretKey))
/// .toString(16)`: SHA-1 over the server id, shared secret and public key,
/// printed as a signed hex number.
pub fn server_hash(server_id: &str, shared_secret: &[u8], public_key: &[u8]) -> String {
    let mut digest: [u8; 20] = Sha1::new()
        .chain_update(server_id.as_bytes())
        .chain_update(shared_secret)
        .chain_update(public_key)
        .finalize()
        .into();
    let negative = digest[0] & 0x80 != 0;
    if negative {
        // Two's complement negation, so the magnitude prints after the sign.
        let mut carry = true;
        for byte in digest.iter_mut().rev() {
            *byte = !*byte;
            if carry {
                (*byte, carry) = byte.overflowing_add(1);
            }
        }
    }
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let hex = hex.trim_start_matches('0');
    match (negative, hex.is_empty()) {
        (_, true) => "0".into(),
        (true, false) => format!("-{hex}"),
        (false, false) => hex.into(),
    }
}

/// POSTs the join to the session server; `Err` holds a readable reason.
pub async fn join(access_token: &str, uuid: &uuid::Uuid, server_hash: &str) -> Result<(), String> {
    let response = super::http::client()
        .post("https://sessionserver.mojang.com/session/minecraft/join")
        .json(&serde_json::json!({
            "accessToken": access_token,
            "selectedProfile": uuid.simple().to_string(),
            "serverId": server_hash,
        }))
        .send()
        .await
        .map_err(|e| format!("session server unreachable: {e}"))?;
    match response.status() {
        reqwest::StatusCode::NO_CONTENT | reqwest::StatusCode::OK => Ok(()),
        reqwest::StatusCode::FORBIDDEN => {
            #[derive(serde::Deserialize)]
            struct Forbidden {
                error: String,
            }
            let error = response
                .json::<Forbidden>()
                .await
                .map(|f| f.error)
                .unwrap_or_default();
            Err(match error.as_str() {
                "InsufficientPrivilegesException" => "multiplayer is disabled for this account",
                "UserBannedException" => "this account is banned from multiplayer",
                "InvalidCredentialsException" => "the session is invalid; sign in again",
                _ => return Err(format!("session server refused the join: {error}")),
            }
            .to_owned())
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            Err("session server rate limit; try again".into())
        }
        status => Err(format!("session server answered {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::server_hash;

    /// The examples from the protocol documentation, hashing only the name
    /// (the other inputs empty).
    #[test]
    fn hashes_print_as_signed_hex() {
        assert_eq!(
            server_hash("Notch", &[], &[]),
            "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48"
        );
        assert_eq!(
            server_hash("jeb_", &[], &[]),
            "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
        );
        assert_eq!(
            server_hash("simon", &[], &[]),
            "88e16a1019277b15d58faf0541e11910eb756f6"
        );
    }
}
