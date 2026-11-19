//! `/relay/<host>/<path>`: the page's requests to Mojang and Microsoft, which
//! don't allow cross-origin ones.

use std::net::SocketAddr;
use std::time::Duration;

use axum::BoxError;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;

use crate::limits::Spend;
use crate::{AppState, guard};

/// Everything the client talks to, and what a request to it spends: the
/// version manifests and game files, the skin textures, the account and
/// session services, and the Microsoft and Xbox sign-in steps.
pub const ALLOWED_HOSTS: &[(&str, Spend)] = &[
    ("piston-meta.mojang.com", Spend::Download),
    ("piston-data.mojang.com", Spend::Download),
    ("libraries.minecraft.net", Spend::Download),
    ("resources.download.minecraft.net", Spend::Download),
    ("textures.minecraft.net", Spend::Download),
    ("api.mojang.com", Spend::Api),
    ("api.minecraftservices.com", Spend::Api),
    ("sessionserver.mojang.com", Spend::Api),
    ("login.microsoftonline.com", Spend::Api),
    ("user.auth.xboxlive.com", Spend::Api),
    ("xsts.auth.xboxlive.com", Spend::Api),
];

/// Request headers that describe the page, the browser's connection or the
/// proxies in front of the relay rather than the request, and so stay here.
const DROPPED_REQUEST_HEADERS: &[&str] = &[
    "host",
    "origin",
    "referer",
    "cookie",
    "connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "accept-encoding",
    "forwarded",
    "via",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "x-forwarded-server",
    "x-original-forwarded-for",
    "x-real-ip",
    "x-client-ip",
    "x-request-id",
    "true-client-ip",
    "fly-client-ip",
    "cdn-loop",
    "cf-connecting-ip",
    "cf-ipcountry",
    "cf-ray",
    "cf-visitor",
];

/// Response headers passed back; the rest belong to the relay's own hop.
const KEPT_RESPONSE_HEADERS: &[header::HeaderName] = &[
    header::CONTENT_TYPE,
    header::CACHE_CONTROL,
    header::ETAG,
    header::LAST_MODIFIED,
    header::ACCEPT_RANGES,
    header::CONTENT_RANGE,
];

/// The content types the client reads. Anything else is relayed as bytes, so
/// no relayed response can be a page running as this relay's origin.
const DATA_TYPES: &[&str] = &[
    "application/json",
    "application/octet-stream",
    "image/png",
    "text/plain",
];

/// Gives up on a host that won't answer, or a download that stalls, rather
/// than holding the relay's connections open. A redirect isn't followed: its
/// target was never checked against [`ALLOWED_HOSTS`].
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("the HTTP client builds")
}

pub async fn relay(
    State(state): State<AppState>,
    peer: ConnectInfo<SocketAddr>,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some((host, rest)) = target(&uri) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if guard::cross_site(&headers) {
        tracing::warn!("Refused a cross-site request for {host}");
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(&(_, spend)) = ALLOWED_HOSTS.iter().find(|(allowed, _)| *allowed == host) else {
        tracing::warn!("Refused a request for {host}");
        return StatusCode::FORBIDDEN.into_response();
    };
    let client = guard::client(&state.config, peer, &headers);
    if !state.limits.allow(spend, client) {
        tracing::warn!("Rate limited {client}'s requests for {host}");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let url = format!("https://{host}/{rest}");
    let mut forwarded = headers;
    for name in DROPPED_REQUEST_HEADERS {
        forwarded.remove(*name);
    }
    let upstream = match state
        .http
        .request(method, &url)
        .headers(forwarded)
        .body(body)
        .send()
        .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            tracing::warn!("Couldn't reach {url}: {error}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    // The page couldn't follow one either: its target isn't under `/relay`.
    if upstream.status().is_redirection() && upstream.headers().contains_key(header::LOCATION) {
        tracing::warn!("{url} redirects, which the relay doesn't follow");
        return StatusCode::BAD_GATEWAY.into_response();
    }
    let mut response = Response::builder()
        .status(upstream.status())
        .header(header::CONTENT_SECURITY_POLICY, "sandbox");
    for name in KEPT_RESPONSE_HEADERS {
        if let Some(value) = upstream.headers().get(name) {
            let value = if name == header::CONTENT_TYPE && !data(value) {
                &HeaderValue::from_static("application/octet-stream")
            } else {
                value
            };
            response = response.header(name, value);
        }
    }
    let limits = state.limits;
    let relayed = upstream.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(BoxError::from)?;
        if spend == Spend::Download && !limits.downloaded(client, chunk.len()) {
            tracing::warn!("Cut {client}'s download short at what it may download");
            return Err(BoxError::from("over the download limit"));
        }
        Ok(chunk)
    });
    response
        .body(Body::from_stream(relayed))
        .expect("the relayed response builds")
}

/// The host, and the path and query after it, of a request under `/relay`,
/// as the page wrote them: decoding the path would turn an encoded `?` or `/`
/// into a real one.
fn target(uri: &Uri) -> Option<(&str, &str)> {
    uri.path_and_query()?
        .as_str()
        .strip_prefix('/')?
        .split_once('/')
}

/// Whether `content_type` is one of [`DATA_TYPES`].
fn data(content_type: &HeaderValue) -> bool {
    content_type
        .to_str()
        .ok()
        .and_then(|value| value.split(';').next())
        .is_some_and(|essence| {
            DATA_TYPES
                .iter()
                .any(|data| essence.trim().eq_ignore_ascii_case(data))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_and_query_pass_through_as_written() {
        let uri = Uri::from_static("/api.mojang.com/a%3Fb%2Fc?d=e%20f");
        assert_eq!(target(&uri), Some(("api.mojang.com", "a%3Fb%2Fc?d=e%20f")));
        let uri = Uri::from_static("/api.mojang.com/");
        assert_eq!(target(&uri), Some(("api.mojang.com", "")));
        let uri = Uri::from_static("/api.mojang.com");
        assert_eq!(target(&uri), None);
    }

    #[test]
    fn only_data_types_pass() {
        for value in ["application/json; charset=utf-8", "IMAGE/PNG", "text/plain"] {
            assert!(data(&HeaderValue::from_static(value)), "{value}");
        }
        for value in ["text/html", "image/svg+xml", "application/xhtml+xml"] {
            assert!(!data(&HeaderValue::from_static(value)), "{value}");
        }
    }
}
