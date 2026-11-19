//! OAuth for remote MCP servers (MCP 2025-06-18 authorization, ADR 22).
//!
//! Discovery follows the server: its 401 names the protected resource
//! metadata (RFC 9728) in `WWW-Authenticate`, else the metadata is at the
//! well-known URL. That metadata names the authorization server, whose own
//! metadata comes from RFC 8414's URL or OpenID Connect's.
//!
//! Tokens, codes, client secrets and PKCE verifiers never appear in an
//! error: every message here is built from URLs, status codes and the
//! authorization server's error code alone.

use super::http::{describe, read_body};
use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};
use std::time::Duration;

/// Each request to an authorization server must finish within this long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A metadata document or token response larger than this is refused.
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// What discovery found about a server and its authorization server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Discovery {
    /// The `resource` of authorization and token requests (RFC 8707): the
    /// metadata's `resource`, which covers the server's URL.
    pub(super) resource: String,
    /// `scopes_supported` of the protected resource metadata.
    pub(super) scopes_supported: Vec<String>,
    pub(super) authorization_endpoint: Url,
    pub(super) token_endpoint: Url,
    pub(super) registration_endpoint: Option<Url>,
    /// `token_endpoint_auth_methods_supported`, when listed.
    pub(super) token_auth_methods: Option<Vec<String>>,
}

/// Finds the authorization server of the MCP server at `url`. `Err` is a
/// phrase that follows "it" or stands alone after a colon.
pub(super) async fn discover(client: &Client, url: &str) -> Result<Discovery, String> {
    let server = Url::parse(url).map_err(|err| format!("its url is invalid: {err}"))?;
    // An unauthenticated initialize, as any first request would be.
    let probe = client
        .post(server.clone())
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 0,
                "method": "initialize",
                "params": {
                    "protocolVersion": super::PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "socium", "version": env!("CARGO_PKG_VERSION")},
                },
            })
            .to_string(),
        )
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| format!("the server could not be reached: {}", describe(&err)))?;
    let challenged = probe.status() == StatusCode::UNAUTHORIZED;
    let named = probe
        .headers()
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(resource_metadata_parameter);
    drop(probe);
    let mut candidates = Vec::new();
    if let Some(named) = named {
        let (named, _) = crate::models::check_endpoint_url("resource_metadata", &named)
            .map_err(|err| format!("its WWW-Authenticate header names a {err}"))?;
        candidates.push(named);
    }
    candidates.extend(protected_resource_urls(&server));
    let mut found = None;
    for candidate in &candidates {
        if let Some(document) = get_json(client, candidate).await
            && document["authorization_servers"]
                .as_array()
                .is_some_and(|servers| !servers.is_empty())
        {
            found = Some(document);
            break;
        }
    }
    let Some(metadata) = found else {
        return Err(if challenged {
            "the server asked for authorization (HTTP 401) but publishes no protected resource metadata (RFC 9728) that names an authorization server".into()
        } else {
            "the server did not ask for authorization and publishes no protected resource metadata (RFC 9728); it may need no login".into()
        });
    };
    let resource = match metadata["resource"].as_str() {
        Some(resource) => {
            let covered = Url::parse(resource).is_ok_and(|parsed| covers(&parsed, &server));
            if !covered {
                return Err(format!(
                    "the server's metadata is for the resource '{resource}', which does not cover {url}"
                ));
            }
            resource.to_string()
        }
        None => url.to_string(),
    };
    let scopes_supported = strings(&metadata["scopes_supported"]).unwrap_or_default();
    // RFC 9728 leaves the choice to the client; the first is the server's
    // own preference.
    let issuer = metadata["authorization_servers"][0]
        .as_str()
        .ok_or("the server's metadata names an authorization server that is not a URL")?;
    let (issuer_url, _) = crate::models::check_endpoint_url("authorization server", issuer)
        .map_err(|err| format!("the server's metadata names an {err}"))?;
    let mut server_metadata = None;
    for candidate in authorization_server_urls(&issuer_url) {
        if let Some(document) = get_json(client, &candidate).await {
            server_metadata = Some(document);
            break;
        }
    }
    let metadata = server_metadata.ok_or_else(|| {
        format!(
            "the authorization server {issuer} publishes no metadata (RFC 8414 or OpenID Connect Discovery)"
        )
    })?;
    // RFC 8414 3.3: the metadata must be the issuer's own.
    if metadata["issuer"]
        .as_str()
        .map(|given| given.trim_end_matches('/'))
        != Some(issuer.trim_end_matches('/'))
    {
        return Err(format!(
            "the metadata of the authorization server {issuer} names another issuer"
        ));
    }
    let endpoint = |field: &str| -> Result<Option<Url>, String> {
        match &metadata[field] {
            Value::Null => Ok(None),
            Value::String(raw) => crate::models::check_endpoint_url(field, raw)
                .map(|(url, _)| Some(url))
                .map_err(|err| format!("the authorization server's {err}")),
            _ => Err(format!("the authorization server's {field} is not a URL")),
        }
    };
    let required = |field: &str| {
        endpoint(field)?.ok_or_else(|| {
            format!("the metadata of the authorization server {issuer} has no {field}")
        })
    };
    let authorization_endpoint = required("authorization_endpoint")?;
    let token_endpoint = required("token_endpoint")?;
    let registration_endpoint = endpoint("registration_endpoint")?;
    if let Some(methods) = strings(&metadata["code_challenge_methods_supported"])
        && !methods.iter().any(|method| method == "S256")
    {
        return Err(format!(
            "the authorization server {issuer} does not support PKCE with S256"
        ));
    }
    Ok(Discovery {
        resource,
        scopes_supported,
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        token_auth_methods: strings(&metadata["token_endpoint_auth_methods_supported"]),
    })
}

/// The `resource_metadata` parameter of a `Bearer` challenge.
fn resource_metadata_parameter(header: &str) -> Option<String> {
    let rest = header.trim_start();
    let scheme_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if !rest[..scheme_end].eq_ignore_ascii_case("bearer") {
        return None;
    }
    let mut rest = &rest[scheme_end..];
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        let equals = rest.find('=')?;
        let name = rest[..equals].trim();
        rest = rest[equals + 1..].trim_start();
        let value;
        if let Some(quoted) = rest.strip_prefix('"') {
            let mut text = String::new();
            let mut chars = quoted.char_indices();
            let mut end = None;
            while let Some((index, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            text.push(escaped);
                        }
                    }
                    '"' => {
                        end = Some(index);
                        break;
                    }
                    other => text.push(other),
                }
            }
            rest = &quoted[end? + 1..];
            value = text;
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_string();
            rest = &rest[end..];
        }
        if name.eq_ignore_ascii_case("resource_metadata") {
            return Some(value);
        }
    }
}

/// RFC 9728 3.1: the well-known path goes between the host and the
/// server's path; the root document comes second.
fn protected_resource_urls(server: &Url) -> Vec<Url> {
    well_known(server, "oauth-protected-resource")
}

/// RFC 8414 3.1 and OpenID Connect Discovery, in the order the MCP
/// specification's later revisions give.
fn authorization_server_urls(issuer: &Url) -> Vec<Url> {
    let path = issuer.path().trim_end_matches('/');
    let mut urls = well_known(issuer, "oauth-authorization-server");
    urls.truncate(1);
    urls.extend(
        well_known(issuer, "openid-configuration")
            .into_iter()
            .take(1),
    );
    if !path.is_empty() {
        let mut appended = issuer.clone();
        appended.set_path(&format!("{path}/.well-known/openid-configuration"));
        urls.push(appended);
    }
    urls
}

/// `scheme://host/.well-known/<name><path>`, then the root one when `url`
/// has a path.
fn well_known(url: &Url, name: &str) -> Vec<Url> {
    let path = url.path().trim_end_matches('/');
    let mut urls = Vec::new();
    for suffix in [path, ""] {
        let mut candidate = url.clone();
        candidate.set_query(None);
        candidate.set_fragment(None);
        candidate.set_path(&format!("/.well-known/{name}{suffix}"));
        if !urls.contains(&candidate) {
            urls.push(candidate);
        }
    }
    urls
}

/// Whether `resource` names `server` or a path above it on the same origin.
fn covers(resource: &Url, server: &Url) -> bool {
    if resource.origin() != server.origin() {
        return false;
    }
    let base = resource.path().trim_end_matches('/');
    let path = server.path();
    path == base
        || base.is_empty()
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A JSON object served at `url` with 200, or `None` for anything else.
async fn get_json(client: &Client, url: &Url) -> Option<Value> {
    let response = client
        .get(url.clone())
        .header(ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let body = read_body(response, MAX_DOCUMENT_BYTES).await.ok()?;
    serde_json::from_slice::<Value>(&body)
        .ok()
        .filter(Value::is_object)
}

/// An array of strings, or `None` when the field is not one.
fn strings(value: &Value) -> Option<Vec<String>> {
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::mcp::tests::FakeHttpServer;

    /// Runs `future` on a runtime of its own, as the login does.
    pub(crate) fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn discovered(options: &[&str]) -> (FakeHttpServer, Result<Discovery, String>) {
        let fake = FakeHttpServer::start(options);
        let client = super::super::http::http_client().unwrap();
        let found = block_on(discover(&client, &fake.url));
        (fake, found)
    }

    #[test]
    fn the_authorization_server_is_found_from_the_401_header_or_the_well_known_url() {
        for options in [
            &["--oauth"][..],
            &["--oauth", "--discovery=well-known"],
            &["--oauth", "--as-metadata=oidc", "--scopes=read,write"],
        ] {
            let (fake, found) = discovered(options);
            let found = found.unwrap();
            let origin = fake.url.trim_end_matches("/mcp");

            assert_eq!(found.resource, fake.url, "{options:?}");
            assert_eq!(
                found.authorization_endpoint.as_str(),
                format!("{origin}/auth/authorize")
            );
            assert_eq!(
                found.token_endpoint.as_str(),
                format!("{origin}/auth/token")
            );
            assert_eq!(
                found.registration_endpoint.unwrap().as_str(),
                format!("{origin}/auth/register")
            );
            let expected_scopes: &[&str] = if options.len() == 3 {
                &["read", "write"]
            } else {
                &[]
            };
            assert_eq!(found.scopes_supported, expected_scopes);
            let paths = fake
                .requests()
                .into_iter()
                .filter_map(|request| request["path"].as_str().map(str::to_string))
                .collect::<Vec<_>>();
            let expected: &[&str] = match options.get(1) {
                None => &[
                    "/meta/resource",
                    "/.well-known/oauth-authorization-server/auth",
                ],
                Some(&"--discovery=well-known") => &[
                    "/.well-known/oauth-protected-resource/mcp",
                    "/.well-known/oauth-authorization-server/auth",
                ],
                Some(_) => &[
                    "/meta/resource",
                    "/.well-known/oauth-authorization-server/auth",
                    "/.well-known/openid-configuration/auth",
                    "/auth/.well-known/openid-configuration",
                ],
            };
            assert_eq!(paths, expected, "{options:?}");
        }
    }

    #[test]
    fn a_server_without_metadata_is_an_error() {
        let (_, found) = discovered(&["--token=t"]);
        assert!(
            found
                .unwrap_err()
                .contains("publishes no protected resource metadata"),
        );
        let (_, found) = discovered(&[]);
        assert!(found.unwrap_err().contains("it may need no login"));
        let client = super::super::http::http_client().unwrap();
        let found = block_on(discover(&client, "http://127.0.0.1:9/mcp"));
        assert!(found.unwrap_err().contains("could not be reached"));
    }

    #[test]
    fn the_resource_metadata_parameter_is_read_from_a_bearer_challenge() {
        for (header, expected) in [
            (
                r#"Bearer resource_metadata="https://a.example/.well-known/oauth-protected-resource""#,
                Some("https://a.example/.well-known/oauth-protected-resource"),
            ),
            (
                r#"bearer error="invalid_token", error_description="a \"quoted\", text", resource_metadata="https://a.example/m""#,
                Some("https://a.example/m"),
            ),
            (
                "Bearer realm=mcp, resource_metadata=https://a.example/m",
                Some("https://a.example/m"),
            ),
            (r#"Bearer realm="mcp""#, None),
            (r#"Basic resource_metadata="https://a.example/m""#, None),
            ("Bearer", None),
        ] {
            assert_eq!(
                resource_metadata_parameter(header).as_deref(),
                expected,
                "{header}"
            );
        }
    }

    #[test]
    fn well_known_urls_follow_rfc_9728_and_rfc_8414() {
        let urls = |list: Vec<Url>| list.iter().map(Url::to_string).collect::<Vec<_>>();
        assert_eq!(
            urls(protected_resource_urls(
                &Url::parse("https://a.example/v1/mcp/").unwrap()
            )),
            [
                "https://a.example/.well-known/oauth-protected-resource/v1/mcp",
                "https://a.example/.well-known/oauth-protected-resource",
            ]
        );
        assert_eq!(
            urls(protected_resource_urls(
                &Url::parse("https://a.example").unwrap()
            )),
            ["https://a.example/.well-known/oauth-protected-resource"]
        );
        assert_eq!(
            urls(authorization_server_urls(
                &Url::parse("https://auth.example/tenant").unwrap()
            )),
            [
                "https://auth.example/.well-known/oauth-authorization-server/tenant",
                "https://auth.example/.well-known/openid-configuration/tenant",
                "https://auth.example/tenant/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            urls(authorization_server_urls(
                &Url::parse("https://auth.example/").unwrap()
            )),
            [
                "https://auth.example/.well-known/oauth-authorization-server",
                "https://auth.example/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn a_resource_covers_its_own_path_and_those_below_it() {
        let covered = |resource: &str, server: &str| {
            covers(&Url::parse(resource).unwrap(), &Url::parse(server).unwrap())
        };
        assert!(covered("https://a.example/mcp", "https://a.example/mcp"));
        assert!(covered("https://a.example/mcp", "https://a.example/mcp/"));
        assert!(covered("https://a.example", "https://a.example/mcp"));
        assert!(covered("https://A.example/", "https://a.example/x/mcp"));
        assert!(!covered("https://a.example/mcp", "https://a.example/mcpx"));
        assert!(!covered("https://a.example/mcp", "https://a.example/"));
        assert!(!covered("https://b.example/mcp", "https://a.example/mcp"));
        assert!(!covered("http://a.example/mcp", "https://a.example/mcp"));
    }
}
