//! `/relay/<host>/<path>`: the page's requests to Mojang and Microsoft, which
//! don't allow cross-origin ones.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

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
    "content-length",
    "accept-encoding",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
];

/// Response headers passed back; the rest belong to the relay's own hop.
const KEPT_RESPONSE_HEADERS: &[header::HeaderName] = &[
    header::CONTENT_TYPE,
    header::CACHE_CONTROL,
    header::ETAG,
    header::LAST_MODIFIED,
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
/// than holding the relay's connections open.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("the HTTP client builds")
}

pub async fn relay(
    State(state): State<AppState>,
    Path((host, path)): Path<(String, String)>,
    peer: ConnectInfo<SocketAddr>,
    RawQuery(query): RawQuery,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
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
    let mut url = format!("https://{host}/{path}");
    if let Some(query) = query {
        url.push('?');
        url.push_str(&query);
    }
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
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .expect("the relayed response builds")
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
    fn only_data_types_pass() {
        for value in ["application/json; charset=utf-8", "IMAGE/PNG", "text/plain"] {
            assert!(data(&HeaderValue::from_static(value)), "{value}");
        }
        for value in ["text/html", "image/svg+xml", "application/xhtml+xml"] {
            assert!(!data(&HeaderValue::from_static(value)), "{value}");
        }
    }
}
