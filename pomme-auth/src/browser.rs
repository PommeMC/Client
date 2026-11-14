//! The browser sign-in desktop uses (RFC 8252): OAuth authorization code with
//! PKCE, returned to a listener on a free localhost port. The registered
//! redirect is plain `http://localhost`, whose port Microsoft ignores. The
//! browser is held on the callback until the account is checked, then sent
//! on to pomme.rs to say how it went.

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{
    AuthError, CLIENT_ID, MsaTokenResponse, SCOPE, TOKEN_URL, TokenError, finish_msa_exchange,
};
use crate::{AuthAccount, SecretStore};

const AUTHORIZE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const SIGNED_IN_URL: &str = "https://pomme.rs/signed-in";
const SIGN_IN_FAILED_URL: &str = "https://pomme.rs/sign-in-failed";
const TIMEOUT: Duration = Duration::from_secs(600);

/// `base` with `pairs` as its query.
fn url_with(base: &str, pairs: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse(base).expect("a valid URL");
    url.query_pairs_mut().extend_pairs(pairs);
    url.into()
}

/// Answers a request with an empty body.
async fn reply(stream: &mut TcpStream, head: &str) {
    let response = format!("HTTP/1.1 {head}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(response.as_bytes()).await;
}

/// `bytes` random bytes, base64url without padding.
fn random_token(bytes: usize) -> String {
    URL_SAFE_NO_PAD.encode((0..bytes).map(|_| rand::random::<u8>()).collect::<Vec<_>>())
}

/// Signs in through the browser: `on_url` gets the sign-in page to open.
pub async fn browser_sign_in(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    on_url: impl FnOnce(&str),
) -> Result<AuthAccount, String> {
    let listen_error = |e| format!("Couldn't listen for the sign-in: {}", crate::error_chain(e));
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(listen_error)?;
    let port = listener.local_addr().map_err(listen_error)?.port();
    let redirect_uri = format!("http://localhost:{port}");
    let verifier = random_token(32);
    let challenge = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let state = random_token(16);

    on_url(&url_with(
        AUTHORIZE_URL,
        &[
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPE),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ],
    ));

    let (mut stream, params) = tokio::time::timeout(TIMEOUT, callback(&listener))
        .await
        .map_err(|_| "The sign-in timed out; try again".to_string())??;
    let result = match callback_code(&params, &state) {
        Ok(code) => exchange_code(http, store, code, &redirect_uri, &verifier).await,
        Err(reason) => Err(reason),
    };
    let target = match &result {
        Ok(_) => SIGNED_IN_URL.to_owned(),
        Err(e) => url_with(
            SIGN_IN_FAILED_URL,
            &[("code", e.code), ("reason", &e.message)],
        ),
    };
    reply(&mut stream, &format!("302 Found\r\nLocation: {target}")).await;
    result.map_err(String::from)
}

/// The first request that carries the sign-in's answer, and the connection to
/// answer it on; anything else (a favicon, an empty speculative connection)
/// gets a 404.
async fn callback(listener: &TcpListener) -> Result<(TcpStream, HashMap<String, String>), String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("Couldn't take the sign-in: {}", crate::error_chain(e)))?;
        let mut buf = [0u8; 8192];
        let Ok(n) = stream.read(&mut buf).await else {
            continue;
        };
        if let Some(params) = query(&String::from_utf8_lossy(&buf[..n]))
            && (params.contains_key("code") || params.contains_key("error"))
        {
            return Ok((stream, params));
        }
        reply(&mut stream, "404 Not Found").await;
    }
}

/// The query of an HTTP request's target.
fn query(request: &str) -> Option<HashMap<String, String>> {
    let target = request.lines().next()?.split_whitespace().nth(1)?;
    let url = reqwest::Url::parse(&format!("http://localhost{target}")).ok()?;
    Some(url.query_pairs().into_owned().collect())
}

/// The authorization code, once the callback's state matches the one sent.
fn callback_code<'a>(
    params: &'a HashMap<String, String>,
    state: &str,
) -> Result<&'a str, AuthError> {
    if let Some(error) = params.get("error") {
        return Err(if error == "access_denied" {
            AuthError::new("cancelled", "Sign-in was cancelled")
        } else {
            params
                .get("error_description")
                .unwrap_or(error)
                .clone()
                .into()
        });
    }
    if params.get("state").map(String::as_str) != Some(state) {
        return Err(AuthError::new(
            "mismatch",
            "The sign-in didn't match; try again",
        ));
    }
    params
        .get("code")
        .map(String::as_str)
        .ok_or_else(|| "Microsoft sent no sign-in code".into())
}

async fn exchange_code(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<AuthAccount, AuthError> {
    let resp = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|e| format!("Sign-in request failed: {}", crate::error_chain(e)))?;
    let ok = resp.status().is_success();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Sign-in read failed: {}", crate::error_chain(e)))?;
    if !ok {
        return Err(serde_json::from_str::<TokenError>(&body)
            .map_or_else(|_| format!("Sign-in failed: {body}"), TokenError::reason)
            .into());
    }
    let msa: MsaTokenResponse = serde_json::from_str(&body)
        .map_err(|e| format!("Token parse failed: {}", crate::error_chain(e)))?;
    finish_msa_exchange(http, store, &msa).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(request: &str) -> HashMap<String, String> {
        query(request).expect("a request with a target")
    }

    #[test]
    fn the_callback_gives_its_code_only_with_the_state_sent() {
        let ok = params("GET /?code=M.C5%2Fabc&state=s1 HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(callback_code(&ok, "s1").ok(), Some("M.C5/abc"));
        assert_eq!(
            callback_code(&ok, "s2").err().map(|e| e.code),
            Some("mismatch")
        );
        let denied = params("GET /?error=access_denied&state=s1 HTTP/1.1\r\n\r\n");
        assert_eq!(
            callback_code(&denied, "s1").err().map(|e| e.code),
            Some("cancelled")
        );
        assert!(query("").is_none());
    }

    #[tokio::test]
    async fn the_listener_skips_requests_without_an_answer() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let browser = tokio::spawn(async move {
            drop(TcpStream::connect(addr).await.unwrap());
            for request in [
                "GET /favicon.ico HTTP/1.1\r\n\r\n",
                "GET /?code=c&state=s HTTP/1.1\r\n\r\n",
            ] {
                let mut stream = TcpStream::connect(addr).await.unwrap();
                stream.write_all(request.as_bytes()).await.unwrap();
                let mut reply = String::new();
                if request.contains("favicon") {
                    stream.read_to_string(&mut reply).await.unwrap();
                    assert!(reply.starts_with("HTTP/1.1 404"));
                }
            }
        });
        let (_, params) = callback(&listener).await.unwrap();
        assert_eq!(params.get("code").map(String::as_str), Some("c"));
        browser.await.unwrap();
    }
}
