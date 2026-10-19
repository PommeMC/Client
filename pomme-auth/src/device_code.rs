//! The device-code sign-in the client uses: show a code, the user enters it
//! at microsoft.com/link in any browser, and the client polls until they do.
//! There is no redirect to catch, so it works where a local listener can't,
//! such as an Android app frozen while the browser is up.

use std::time::Duration;

use serde::Deserialize;

use super::{CLIENT_ID, MsaTokenResponse, SCOPE, TOKEN_URL, finish_msa_exchange};
use crate::{AuthAccount, SecretStore};

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The `devicecode` response; `user_code` and `verification_uri` are what
/// to show the user.
#[derive(Deserialize, Clone, Debug)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    device_code: String,
    expires_in: u64,
    interval: u64,
}

/// One poll's answer.
#[derive(Debug, PartialEq)]
enum Poll {
    Pending,
    SlowDown,
    Failed(String),
}

fn poll_error(body: &str) -> Poll {
    #[derive(Deserialize)]
    struct TokenError {
        error: String,
        error_description: Option<String>,
    }
    match serde_json::from_str::<TokenError>(body) {
        Ok(e) if e.error == "authorization_pending" => Poll::Pending,
        Ok(e) if e.error == "slow_down" => Poll::SlowDown,
        Ok(e) if e.error == "authorization_declined" => Poll::Failed("Sign-in was declined".into()),
        Ok(e) if e.error == "expired_token" => Poll::Failed("The code expired; try again".into()),
        Ok(e) => Poll::Failed(e.error_description.unwrap_or(e.error)),
        Err(_) => Poll::Failed(format!("Sign-in failed: {body}")),
    }
}

/// Signs in with a code: `on_code` gets the code and page to show, then this
/// polls until the user finishes, declines or the code expires.
pub async fn device_code_sign_in(
    http: &reqwest::Client,
    store: &dyn SecretStore,
    on_code: impl FnOnce(&DeviceCode),
) -> Result<AuthAccount, String> {
    let resp = http
        .post(DEVICE_CODE_URL)
        .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .send()
        .await
        .map_err(|e| format!("Sign-in request failed: {}", crate::error_chain(e)))?;
    // Microsoft's own error reads better than a missing `user_code`.
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(match poll_error(&body) {
            Poll::Failed(reason) => reason,
            _ => format!("Sign-in failed: {body}"),
        });
    }
    let code: DeviceCode = resp
        .json()
        .await
        .map_err(|e| format!("Sign-in response parse failed: {}", crate::error_chain(e)))?;
    on_code(&code);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval.max(1));
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(interval).await;
        let sent = http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", DEVICE_CODE_GRANT),
                ("device_code", &code.device_code),
            ])
            .send()
            .await;
        // A poll can fail without the sign-in failing, e.g. while Android has
        // frozen the app behind the browser; the next one retries.
        let Ok(resp) = sent else {
            continue;
        };
        let ok = resp.status().is_success();
        let Ok(body) = resp.text().await else {
            continue;
        };
        if ok {
            let msa: MsaTokenResponse = serde_json::from_str(&body)
                .map_err(|e| format!("Token parse failed: {}", crate::error_chain(e)))?;
            return finish_msa_exchange(http, store, &msa).await;
        }
        match poll_error(&body) {
            Poll::Pending => {}
            // RFC 8628 §3.5: back off by five seconds.
            Poll::SlowDown => interval += Duration::from_secs(5),
            Poll::Failed(reason) => return Err(reason),
        }
    }
    Err("The code expired; try again".into())
}

#[cfg(test)]
mod tests {
    use super::{Poll, poll_error};

    #[test]
    fn poll_errors_map_to_their_outcome() {
        assert_eq!(
            poll_error(r#"{"error":"authorization_pending"}"#),
            Poll::Pending
        );
        assert_eq!(poll_error(r#"{"error":"slow_down"}"#), Poll::SlowDown);
        assert_eq!(
            poll_error(r#"{"error":"expired_token"}"#),
            Poll::Failed("The code expired; try again".into())
        );
        assert_eq!(
            poll_error(r#"{"error":"invalid_grant","error_description":"bad"}"#),
            Poll::Failed("bad".into())
        );
    }
}
