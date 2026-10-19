//! The browser sign-in the launcher uses: OAuth authorization code with PKCE,
//! redirected back to a listener on localhost.

use std::time::Duration;

use super::{CLIENT_ID, MsaTokenResponse, SCOPE, TOKEN_URL, finish_msa_exchange};
use crate::{AuthAccount, SecretStore};

const REDIRECT_PORT: u16 = 25585;
const REDIRECT_URI: &str = "http://localhost:25585";

fn generate_pkce() -> (String, String) {
    use base64::Engine;
    use sha2::Digest;

    let verifier_bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&verifier_bytes);

    let digest = sha2::Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);

    (verifier, challenge)
}

/// Signs in through the browser: `on_url` gets the sign-in page to open.
pub async fn pkce_sign_in(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    on_url: impl FnOnce(&str),
) -> Result<AuthAccount, String> {
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

    // Bind before opening the browser so a taken port fails immediately.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
        .await
        .map_err(|e| {
            format!(
                "Failed to bind callback listener: {}",
                crate::error_chain(e)
            )
        })?;

    on_url(&auth_url);

    let code = listen_for_callback(listener, &state).await?;

    let resp = http
        .post(TOKEN_URL)
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
        .map_err(|e| format!("Token exchange failed: {}", crate::error_chain(e)))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Token exchange read failed: {}", crate::error_chain(e)))?;

    if !status.is_success() {
        return Err(format!("Auth failed ({status}): {body}"));
    }

    let msa: MsaTokenResponse = serde_json::from_str(&body)
        .map_err(|e| format!("Token parse failed: {}", crate::error_chain(e)))?;

    finish_msa_exchange(http, store, &msa).await
}
async fn listen_for_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let timeout = Duration::from_secs(300);
    let (mut stream, _) = tokio::time::timeout(timeout, listener.accept())
        .await
        .map_err(|_| "Authentication timed out".to_string())?
        .map_err(|e| format!("Accept failed: {}", crate::error_chain(e)))?;

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| format!("Read failed: {}", crate::error_chain(e)))?;
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
        reject(&mut stream, desc).await;
        return Err(format!("OAuth error: {desc}"));
    }

    if params.get("state").copied() != Some(expected_state) {
        reject(&mut stream, "state mismatch").await;
        return Err("State mismatch".to_string());
    }

    let Some(raw_code) = params.get("code") else {
        reject(&mut stream, "missing auth code").await;
        return Err("Missing auth code".to_string());
    };
    let code = urlencoding::decode(raw_code)
        .map_err(|e| format!("Failed to decode auth code: {}", crate::error_chain(e)))?
        .into_owned();

    send_http_response(
        &mut stream,
        200,
        "Signed in successfully! You can close this tab.",
    )
    .await;

    Ok(code)
}

async fn reject(stream: &mut tokio::net::TcpStream, desc: &str) {
    send_http_response(stream, 400, &format!("Authentication failed: {desc}")).await;
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
