//! One log line per request: the live view CloudFront's batched logs are not.

use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// The line is billed by the byte, and some agents are absurd.
const AGENT_MAX: usize = 120;

/// Who asked. Each hop appends to `x-forwarded-for`, so the caller is its
/// first entry — but that entry is whatever the browser sent. Cloudflare
/// overwrites `cf-connecting-ip`, so prefer it and fall back to the chain.
fn client_ip(headers: &axum::http::HeaderMap) -> Option<&str> {
    let header = |name| headers.get(name)?.to_str().ok();
    let forwarded = || header("x-forwarded-for").and_then(|raw| raw.split(',').next());
    header("cf-connecting-ip")
        .or_else(forwarded)
        .map(str::trim)
        .filter(|ip| !ip.is_empty())
}

fn agent(headers: &axum::http::HeaderMap) -> Option<&str> {
    let raw = headers.get(axum::http::header::USER_AGENT)?.to_str().ok()?;
    Some(&raw[..raw.len().min(AGENT_MAX)])
}

/// Method, path, status and duration. The path only, never the query string:
/// an OAuth callback carries the code and nonce there. The body is never read.
pub async fn access_log(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let headers = request.headers();
    let ip = client_ip(headers).unwrap_or("-").to_owned();
    let agent = agent(headers).unwrap_or("-").to_owned();

    let started = Instant::now();
    let response = next.run(request).await;

    tracing::info!(
        %method,
        path,
        status = response.status().as_u16(),
        ms = started.elapsed().as_millis(),
        ip,
        agent,
        "request"
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use axum::http::header::{HeaderValue, USER_AGENT};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    /// The forwarded chain reads caller-first: Cloudflare's edge and
    /// CloudFront's are appended behind it.
    #[test]
    fn the_caller_is_the_first_hop_not_the_last() {
        let map = headers(&[("x-forwarded-for", "203.0.113.7, 70.132.1.1, 10.0.0.9")]);
        assert_eq!(client_ip(&map), Some("203.0.113.7"));
        assert_eq!(
            client_ip(&headers(&[("x-forwarded-for", "198.51.100.2")])),
            Some("198.51.100.2")
        );
    }

    /// A browser can send its own `x-forwarded-for`, and Cloudflare appends to
    /// it rather than replacing it — so the head of that chain is a claim.
    /// `cf-connecting-ip` is Cloudflare's own word and outranks it.
    #[test]
    fn cloudflares_own_header_beats_a_forged_chain() {
        let map = headers(&[
            ("x-forwarded-for", "9.9.9.9, 203.0.113.7, 70.132.1.1"),
            ("cf-connecting-ip", "203.0.113.7"),
        ]);
        assert_eq!(client_ip(&map), Some("203.0.113.7"));
    }

    #[test]
    fn a_missing_or_empty_forwarded_for_is_nobody() {
        assert_eq!(client_ip(&HeaderMap::new()), None);
        assert_eq!(client_ip(&headers(&[("x-forwarded-for", " ")])), None);
        assert_eq!(client_ip(&headers(&[("cf-connecting-ip", "")])), None);
    }

    #[test]
    fn a_long_user_agent_is_cut_rather_than_billed_for() {
        let mut map = HeaderMap::new();
        map.insert(USER_AGENT, HeaderValue::from_str(&"a".repeat(500)).unwrap());
        assert_eq!(agent(&map).map(str::len), Some(AGENT_MAX));
        assert_eq!(agent(&HeaderMap::new()), None);
    }
}
