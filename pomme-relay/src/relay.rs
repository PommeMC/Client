//! `/relay/<host>/<path>`: the page's requests to Mojang and Microsoft, which
//! don't allow cross-origin ones.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::BoxError;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::{Stream, StreamExt as _};

use crate::limits::{Limits, Spend};
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

/// Response headers passed back; the rest belong to the relay's own hop. The
/// length is one of them so that a body cut short reads as a failure even
/// where nothing else marks its end.
const KEPT_RESPONSE_HEADERS: &[header::HeaderName] = &[
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
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

/// More than an account or sign-in request sends; the largest is a skin, a
/// few kilobytes of PNG.
pub const MAX_BODY: usize = 256 * 1024;

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
    request: Request,
) -> Response {
    let (request, body) = request.into_parts();
    let Some((host, rest)) = target(&request.uri) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if guard::cross_site(&request.headers) {
        tracing::debug!("Refused a cross-site request for {host}");
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(&(_, spend)) = ALLOWED_HOSTS.iter().find(|(allowed, _)| *allowed == host) else {
        tracing::debug!("Refused a request for {host}");
        return StatusCode::FORBIDDEN.into_response();
    };
    let download = spend == Spend::Download;
    if download && !matches!(request.method, Method::GET | Method::HEAD) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let client = guard::client(&state.config, peer, &request.headers);
    if !state.limits.allow(spend, client) {
        tracing::debug!("Rate limited {client}'s requests for {host}");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    // Read only now, and never for a download, so nothing is uploaded for a
    // request the relay refuses or to a host that takes none.
    let body = if download {
        Bytes::new()
    } else {
        match axum::body::to_bytes(body, MAX_BODY).await {
            Ok(body) => body,
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        }
    };
    let url = format!("https://{host}/{rest}");
    let mut forwarded = request.headers;
    for name in DROPPED_REQUEST_HEADERS {
        forwarded.remove(*name);
    }
    let upstream = match state
        .http
        .request(request.method, &url)
        .headers(forwarded)
        .body(body)
        .send()
        .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            tracing::warn!("Couldn't reach {host}: {}", error.without_url());
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    // The page couldn't follow one either: its target isn't under `/relay`.
    if upstream.status().is_redirection() && upstream.headers().contains_key(header::LOCATION) {
        tracing::warn!("{host} redirects, which the relay doesn't follow");
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
    let body = upstream.bytes_stream();
    let body = if download {
        Body::from_stream(metered(body, state.limits, client))
    } else {
        Body::from_stream(body)
    };
    response.body(body).expect("the relayed response builds")
}

/// `download`, counted against what `client` may download and ended with an
/// error once that is spent, so the page sees it fail and not finish short.
fn metered<E: Into<BoxError>>(
    download: impl Stream<Item = Result<Bytes, E>>,
    limits: Arc<Limits>,
    client: IpAddr,
) -> impl Stream<Item = Result<Bytes, BoxError>> {
    download.map(move |chunk| {
        let chunk = chunk.map_err(Into::into)?;
        if !limits.relayed(client, chunk.len()) {
            tracing::warn!("Cut {client}'s download short at what it may download");
            return Err("over the download limit".into());
        }
        Ok(chunk)
    })
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

    #[tokio::test]
    async fn a_download_ends_in_an_error_once_its_bytes_are_spent() {
        let client = IpAddr::from([203, 0, 113, 7]);
        let limits = Arc::new(Limits::default());
        let chunk = Bytes::from(vec![0; 1 << 20]);
        limits.leave(client, chunk.len() * 3 / 2);
        let chunks = (0..3).map(|_| Ok::<_, BoxError>(chunk.clone()));
        let download = metered(futures_util::stream::iter(chunks), limits, client);
        let relayed: Vec<bool> = download.map(|chunk| chunk.is_ok()).collect().await;
        assert_eq!(relayed, [true, false, false]);
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
