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

use super::http::{describe, http_client, read_body};
use super::token_store::{ClientAuth, StoredToken, TokenStore, now_seconds};
use crate::settings::{McpOAuthSettings, McpServerSettings, McpTransportSettings, Settings};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, WWW_AUTHENTICATE};
use reqwest::{Client, Response, StatusCode, Url};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Each request to an authorization server must finish within this long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A metadata document or token response larger than this is refused.
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
/// The longest request the callback listener reads, and how long one
/// connection may take to send it.
const MAX_CALLBACK_BYTES: usize = 16 * 1024;
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `socium mcp login` waits for the browser to come back.
pub(crate) const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// `socium mcp login <server>`: logs in to the server of that name in
/// config.toml. `Ok` and `Err` are sentences for the user.
pub(crate) fn login_configured(
    lookup: &dyn Fn(&str) -> Option<String>,
    server: &str,
    browser: &(dyn Fn(&str) + Sync),
    timeout: Duration,
) -> Result<String, String> {
    let (settings, warnings) = Settings::load(lookup).map_err(|err| err.to_string())?;
    let Some(found) = settings.mcp_servers.iter().find(|s| s.name == server) else {
        let prefix = format!("mcp.servers.{server}.");
        if let Some(warning) = warnings.iter().find(|w| w.starts_with(&prefix)) {
            return Err(format!(
                "Could not log in to MCP server {server}: {warning}."
            ));
        }
        return Err(match &settings.source {
            Some(path) => format!(
                "No MCP server named {server} is configured in {}, or it is turned off.",
                path.display()
            ),
            None => format!("No MCP server named {server} is configured: there is no config.toml."),
        });
    };
    let store = TokenStore::from_env(lookup).ok_or_else(|| {
        format!("Could not log in to MCP server {server}: neither XDG_STATE_HOME nor HOME is set, so there is nowhere to keep the token.")
    })?;
    login(found, &store, browser, timeout)?;
    Ok(format!(
        "Logged in to MCP server {server}. Socium keeps the token in {} and uses it from the next start.",
        store.path(server).display()
    ))
}

/// `socium mcp logout <server>`: forgets the server's token, whether or
/// not config.toml still names it.
pub(crate) fn logout_configured(
    lookup: &dyn Fn(&str) -> Option<String>,
    server: &str,
) -> Result<String, String> {
    if !crate::settings::valid_mcp_server_name(server) {
        return Err(format!("'{server}' is not an MCP server name."));
    }
    let store = TokenStore::from_env(lookup)
        .ok_or("Neither XDG_STATE_HOME nor HOME is set, so there is no token store.")?;
    let path = store.path(server);
    match store.remove(server) {
        Ok(true) => Ok(format!(
            "Logged out of MCP server {server}: removed {}.",
            path.display()
        )),
        Ok(false) => Ok(format!("MCP server {server} has no stored token.")),
        Err(err) => Err(format!("Could not remove {}: {err}.", path.display())),
    }
}

/// Logs in to the remote server of `settings` and stores its token.
/// `browser` gets the authorization URL to open; it must not block. `Err`
/// is a sentence for the user.
pub(crate) fn login(
    settings: &McpServerSettings,
    store: &TokenStore,
    browser: &(dyn Fn(&str) + Sync),
    timeout: Duration,
) -> Result<(), String> {
    let name = &settings.name;
    let McpTransportSettings::Http {
        url,
        headers,
        oauth,
    } = &settings.transport
    else {
        return Err(format!(
            "MCP server {name} runs a command; only a server with url logs in with OAuth."
        ));
    };
    if headers
        .keys()
        .any(|header| header.eq_ignore_ascii_case("authorization"))
    {
        return Err(format!(
            "MCP server {name} sets an Authorization header in config.toml, which Socium sends instead of an OAuth token; remove it to log in."
        ));
    }
    let config = oauth.clone().unwrap_or_default();
    // reqwest needs a runtime, and the caller may be inside one, so the
    // login runs on a thread with its own.
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| format!("its runtime could not start: {err}"))?
                    .block_on(authorize(name, url, &config, store, browser, timeout))
            })
            .join()
            .unwrap_or_else(|_| Err("the login panicked".into()))
    })
    .map_err(|why| format!("Could not log in to MCP server {name}: {why}."))
}

async fn authorize(
    name: &str,
    url: &str,
    config: &McpOAuthSettings,
    store: &TokenStore,
    browser: &(dyn Fn(&str) + Sync),
    timeout: Duration,
) -> Result<(), String> {
    let client =
        http_client().map_err(|err| format!("its HTTP client could not be built: {err}"))?;
    let found = discover(&client, url).await?;
    let port = config.callback_port.unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|err| {
            format!("Socium could not listen on 127.0.0.1:{port} for the browser: {err}")
        })?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("Socium could not listen for the browser: {err}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let oauth_client = match &config.client_id {
        Some(id) => OAuthClient {
            id: id.clone(),
            secret: config.client_secret.clone(),
            auth: match &config.client_secret {
                None => ClientAuth::None,
                // RFC 6749 2.3.1: every server supports Basic.
                Some(_) => match &found.token_auth_methods {
                    Some(methods)
                        if !methods.iter().any(|m| m == "client_secret_basic")
                            && methods.iter().any(|m| m == "client_secret_post") =>
                    {
                        ClientAuth::ClientSecretPost
                    }
                    _ => ClientAuth::ClientSecretBasic,
                },
            },
        },
        None => {
            let endpoint = found.registration_endpoint.as_ref().ok_or_else(|| {
                format!(
                    "its authorization server does not register clients; register one there and set mcp.servers.{name}.oauth.client_id"
                )
            })?;
            register(&client, endpoint, &redirect_uri).await?
        }
    };
    let verifier = random_text(32);
    let state = random_text(16);
    let scopes = if config.scopes.is_empty() {
        &found.scopes_supported
    } else {
        &config.scopes
    };
    let mut authorization = found.authorization_endpoint.clone();
    {
        let mut query = authorization.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &oauth_client.id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("code_challenge", &code_challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &found.resource);
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
    }
    browser(authorization.as_str());
    let code = tokio::time::timeout(timeout, wait_for_code(&listener, &state))
        .await
        .map_err(|_| {
            format!(
                "the browser did not come back within {}",
                describe_duration(timeout)
            )
        })??;
    drop(listener);
    let issued = token_request(
        &client,
        &found.token_endpoint,
        vec![
            ("grant_type", "authorization_code".into()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("resource", found.resource.clone()),
        ],
        &oauth_client,
    )
    .await?;
    let token = StoredToken {
        url: url.to_string(),
        resource: found.resource,
        access_token: issued.access_token,
        refresh_token: issued.refresh_token,
        expires_at: issued.expires_at,
        token_endpoint: found.token_endpoint.to_string(),
        // A configured secret stays in the environment.
        client_secret: if config.client_id.is_some() {
            None
        } else {
            oauth_client.secret
        },
        client_id: oauth_client.id,
        client_auth: oauth_client.auth,
    };
    store.save(name, &token).map_err(|err| {
        format!(
            "the token could not be saved in {}: {err}",
            store.dir().display()
        )
    })
}

/// Answers the browser until it comes back to `/callback`, and returns the
/// code when the state matches this login's.
async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let Ok(Ok(target)) =
            tokio::time::timeout(CALLBACK_READ_TIMEOUT, read_request_target(&mut stream)).await
        else {
            continue;
        };
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
            respond(
                &mut stream,
                "400 Bad Request",
                "This is not a login callback.",
            )
            .await;
            continue;
        };
        if url.path() != "/callback" {
            respond(
                &mut stream,
                "404 Not Found",
                "This is not a login callback.",
            )
            .await;
            continue;
        }
        let parameter = |key: &str| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };
        if parameter("state").as_deref() != Some(state) {
            respond(
                &mut stream,
                "400 Bad Request",
                "Socium refused this login: the answer is not for the login it started. Run socium mcp login again.",
            )
            .await;
            return Err(
                "the browser came back with a state that does not match this login, so the answer was refused".into(),
            );
        }
        if let Some(error) = parameter("error") {
            respond(
                &mut stream,
                "400 Bad Request",
                "The authorization server refused the login. You can close this tab.",
            )
            .await;
            return Err(format!(
                "the authorization server refused{}",
                error_code(&json!({ "error": error }))
            ));
        }
        let Some(code) = parameter("code").filter(|code| !code.is_empty()) else {
            respond(&mut stream, "400 Bad Request", "The answer has no code.").await;
            return Err("the browser came back without a code".into());
        };
        respond(
            &mut stream,
            "200 OK",
            "Socium is logged in. You can close this tab and return to the terminal.",
        )
        .await;
        return Ok(code);
    }
}

/// The target of a `GET` request line.
async fn read_request_target(stream: &mut TcpStream) -> Result<String, ()> {
    let mut request = Vec::new();
    let mut buffer = [0; 2048];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.map_err(drop)?;
        if read == 0 || request.len() + read > MAX_CALLBACK_BYTES {
            return Err(());
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let line = request.split(|byte| *byte == b'\n').next().ok_or(())?;
    let line = std::str::from_utf8(line).map_err(drop)?;
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) if target.starts_with('/') => Ok(target.to_string()),
        _ => Err(()),
    }
}

async fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body =
        format!("<!doctype html><meta charset=\"utf-8\"><title>Socium</title><p>{message}</p>\n");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn describe_duration(duration: Duration) -> String {
    match duration.as_secs() {
        seconds if seconds >= 60 && seconds % 60 == 0 => format!("{} minutes", seconds / 60),
        1 => "1 second".into(),
        seconds => format!("{seconds} seconds"),
    }
}

/// `bytes` random bytes as base64url, for a PKCE verifier (RFC 7636 4.1:
/// 32 bytes give 43 characters) or a state.
fn random_text(bytes: usize) -> String {
    let mut random = vec![0; bytes];
    aws_lc_rs::rand::fill(&mut random).expect("the system random source works");
    URL_SAFE_NO_PAD.encode(random)
}

/// RFC 7636 4.2: `BASE64URL(SHA256(verifier))`.
fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA256,
        verifier.as_bytes(),
    ))
}

/// A stored token renews this long before it expires.
const REFRESH_MARGIN_SECONDS: u64 = 60;

/// A connected server's token: sent as `Authorization: Bearer`, refreshed
/// before it expires or once after a 401, and saved again when renewed.
pub(super) struct OAuthSession {
    server: String,
    store: TokenStore,
    /// The configured `client_secret`, which is not in the token file.
    client_secret: Option<String>,
    token: tokio::sync::Mutex<StoredToken>,
}

impl OAuthSession {
    pub(super) fn new(
        server: &str,
        store: TokenStore,
        client_secret: Option<String>,
        token: StoredToken,
    ) -> Self {
        Self {
            server: server.to_string(),
            store,
            client_secret,
            token: tokio::sync::Mutex::new(token),
        }
    }

    /// The access token to send, renewed first when it is about to expire.
    /// `Err` means the user has to log in again.
    pub(super) async fn access_token(&self, client: &Client) -> Result<String, ()> {
        let mut token = self.token.lock().await;
        if token.expires_within(REFRESH_MARGIN_SECONDS) && token.refresh_token.is_some() {
            self.renew(&mut token, client).await?;
        }
        Ok(token.access_token.clone())
    }

    /// After the server refused `sent` with 401: renews the token unless
    /// another request already did. `Err` means the user has to log in.
    pub(super) async fn refused(&self, client: &Client, sent: &str) -> Result<(), ()> {
        let mut token = self.token.lock().await;
        if token.access_token != sent {
            return Ok(());
        }
        if token.refresh_token.is_none() {
            return Err(());
        }
        self.renew(&mut token, client).await
    }

    async fn renew(&self, token: &mut StoredToken, client: &Client) -> Result<(), ()> {
        match refresh(client, token, self.client_secret.as_deref()).await {
            Ok(renewed) => {
                // A token that cannot be saved still works for this run.
                let _ = self.store.save(&self.server, &renewed);
                *token = renewed;
                Ok(())
            }
            Err(_) => {
                // Another Socium may have renewed it first, using up the
                // refresh token this one holds.
                match self.store.load(&self.server, &token.url) {
                    Ok(Some(stored)) if stored.access_token != token.access_token => {
                        *token = stored;
                        Ok(())
                    }
                    _ => Err(()),
                }
            }
        }
    }
}

/// Exchanges the refresh token of `token` for a new access token (OAuth
/// 2.1 4.3), keeping the old refresh token when the server issues none.
pub(super) async fn refresh(
    client: &Client,
    token: &StoredToken,
    configured_secret: Option<&str>,
) -> Result<StoredToken, String> {
    let refresh_token = token
        .refresh_token
        .clone()
        .ok_or("there is no refresh token")?;
    let (endpoint, _) = crate::models::check_endpoint_url("token_endpoint", &token.token_endpoint)?;
    let oauth_client = OAuthClient {
        id: token.client_id.clone(),
        secret: configured_secret
            .map(str::to_string)
            .or_else(|| token.client_secret.clone()),
        auth: token.client_auth,
    };
    let issued = token_request(
        client,
        &endpoint,
        vec![
            ("grant_type", "refresh_token".into()),
            ("refresh_token", refresh_token),
            ("resource", token.resource.clone()),
        ],
        &oauth_client,
    )
    .await?;
    Ok(StoredToken {
        access_token: issued.access_token,
        refresh_token: issued.refresh_token.or_else(|| token.refresh_token.clone()),
        expires_at: issued.expires_at,
        ..token.clone()
    })
}

/// What a token endpoint issued.
pub(super) struct Issued {
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) expires_at: Option<u64>,
}

/// Posts `form` to the token endpoint as `oauth_client`.
pub(super) async fn token_request(
    client: &Client,
    endpoint: &Url,
    mut form: Vec<(&'static str, String)>,
    oauth_client: &OAuthClient,
) -> Result<Issued, String> {
    let mut request = client
        .post(endpoint.clone())
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .timeout(REQUEST_TIMEOUT);
    match (oauth_client.auth, &oauth_client.secret) {
        (ClientAuth::ClientSecretBasic, Some(secret)) => {
            // RFC 6749 2.3.1: each part is form-encoded first.
            let credentials = format!("{}:{}", form_encode(&oauth_client.id), form_encode(secret));
            let mut value =
                HeaderValue::from_str(&format!("Basic {}", STANDARD.encode(credentials)))
                    .map_err(|_| "the client credentials do not fit in a header")?;
            value.set_sensitive(true);
            request = request.header(AUTHORIZATION, value);
        }
        (ClientAuth::ClientSecretPost, Some(secret)) => {
            form.push(("client_id", oauth_client.id.clone()));
            form.push(("client_secret", secret.clone()));
        }
        _ => form.push(("client_id", oauth_client.id.clone())),
    }
    let response = request
        .body(encode_form(&form))
        .send()
        .await
        .map_err(|err| {
            format!(
                "the token endpoint {endpoint} could not be reached: {}",
                describe(&err)
            )
        })?;
    let document = answer(response, "token endpoint", endpoint).await?;
    let access_token = document["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| format!("the token endpoint {endpoint} answered without an access_token"))?;
    if HeaderValue::from_str(&format!("Bearer {access_token}")).is_err() {
        return Err(format!(
            "the token endpoint {endpoint} issued an access token that does not fit in a header"
        ));
    }
    match document["token_type"].as_str() {
        Some(kind) if kind.eq_ignore_ascii_case("bearer") => {}
        Some(kind) => {
            return Err(format!(
                "the token endpoint {endpoint} issued a {} token; Socium uses only Bearer tokens",
                kind.chars().take(32).collect::<String>()
            ));
        }
        None => {
            return Err(format!(
                "the token endpoint {endpoint} answered without a token_type"
            ));
        }
    }
    Ok(Issued {
        access_token: access_token.to_string(),
        refresh_token: document["refresh_token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(str::to_string),
        expires_at: document["expires_in"]
            .as_u64()
            .map(|seconds| now_seconds().saturating_add(seconds)),
    })
}

/// `application/x-www-form-urlencoded`, as the URL crate writes a query.
fn encode_form(pairs: &[(&str, String)]) -> String {
    let mut url = Url::parse("http://form.invalid/").expect("a valid URL");
    url.query_pairs_mut()
        .extend_pairs(pairs.iter().map(|(key, value)| (*key, value.as_str())));
    url.query().unwrap_or_default().to_string()
}

fn form_encode(value: &str) -> String {
    encode_form(&[("v", value.to_string())])[2..].to_string()
}

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

/// A client of the authorization server: registered by the login, or
/// configured in `oauth`.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct OAuthClient {
    pub(super) id: String,
    pub(super) secret: Option<String>,
    pub(super) auth: ClientAuth,
}

impl std::fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClient")
            .field("id", &self.id)
            .field("secret", &self.secret.as_ref().map(|_| "<hidden>"))
            .field("auth", &self.auth)
            .finish()
    }
}

/// Registers Socium as a client that redirects to `redirect_uri`
/// (RFC 7591), asking to be a public one: PKCE protects the code.
pub(super) async fn register(
    client: &Client,
    endpoint: &Url,
    redirect_uri: &str,
) -> Result<OAuthClient, String> {
    let response = client
        .post(endpoint.clone())
        .header(ACCEPT, "application/json")
        .json(&json!({
            "client_name": "Socium",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            format!(
                "the registration endpoint {endpoint} could not be reached: {}",
                describe(&err)
            )
        })?;
    let document = answer(response, "registration endpoint", endpoint).await?;
    let id = document["client_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            format!("the registration endpoint {endpoint} answered without a client_id")
        })?;
    let secret = document["client_secret"].as_str().map(str::to_string);
    let auth = match (&secret, document["token_endpoint_auth_method"].as_str()) {
        (None, _) => ClientAuth::None,
        (Some(_), Some("client_secret_post")) => ClientAuth::ClientSecretPost,
        (Some(_), _) => ClientAuth::ClientSecretBasic,
    };
    Ok(OAuthClient {
        id: id.to_string(),
        secret,
        auth,
    })
}

/// The JSON object of a successful answer from an authorization server's
/// `what` at `endpoint`. A refusal quotes only the OAuth error code.
async fn answer(response: Response, what: &str, endpoint: &Url) -> Result<Value, String> {
    let status = response.status();
    if status.is_redirection() {
        return Err(format!(
            "the {what} {endpoint} answered with a redirect (HTTP {status}), which Socium does not follow"
        ));
    }
    let body = read_body(response, MAX_DOCUMENT_BYTES)
        .await
        .map_err(|why| format!("the {what} {endpoint} broke off its answer: {why}"))?;
    let document = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!(
            "the {what} {endpoint} refused (HTTP {status}{})",
            error_code(&document)
        ));
    }
    if !document.is_object() {
        return Err(format!(
            "the {what} {endpoint} answered with no JSON object"
        ));
    }
    Ok(document)
}

/// `": <error>"` for an OAuth error response. The description is left
/// out: a server may repeat what it was sent, such as a code.
fn error_code(document: &Value) -> String {
    match document["error"].as_str() {
        Some(code)
            if !code.is_empty()
                && code.len() <= 64
                && code
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) =>
        {
            format!(": {code}")
        }
        _ => String::new(),
    }
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
pub(crate) mod tests {
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
    fn socium_registers_as_a_public_client_with_its_redirect_uri() {
        let (fake, found) = discovered(&["--oauth"]);
        let client = super::super::http::http_client().unwrap();
        let endpoint = found.unwrap().registration_endpoint.unwrap();

        let registered = block_on(register(
            &client,
            &endpoint,
            "http://127.0.0.1:4567/callback",
        ))
        .unwrap();

        assert!(registered.id.starts_with("client-"), "{registered:?}");
        assert_eq!(
            (registered.secret, registered.auth),
            (None, ClientAuth::None)
        );
        let request = fake
            .requests()
            .into_iter()
            .find(|request| request["path"] == "/auth/register")
            .unwrap();
        assert_eq!(
            request["params"]["redirect_uris"],
            json!(["http://127.0.0.1:4567/callback"])
        );
        assert_eq!(request["params"]["token_endpoint_auth_method"], "none");
        assert_eq!(
            request["params"]["grant_types"],
            json!(["authorization_code", "refresh_token"])
        );

        // A server without registration answers 404.
        let (fake, _) = discovered(&["--oauth", "--no-registration"]);
        let endpoint = Url::parse(&format!(
            "{}/auth/register",
            fake.url.trim_end_matches("/mcp")
        ))
        .unwrap();
        let err =
            block_on(register(&client, &endpoint, "http://127.0.0.1:1/callback")).unwrap_err();
        assert!(err.contains("refused (HTTP 404 Not Found)"), "{err}");
    }

    #[test]
    fn an_oauth_error_is_quoted_by_its_code_alone() {
        assert_eq!(
            error_code(&json!({"error": "invalid_grant", "error_description": "code abc is used"})),
            ": invalid_grant"
        );
        assert_eq!(error_code(&json!({"error": "has spaces"})), "");
        assert_eq!(error_code(&Value::Null), "");
    }

    /// A GET over plain HTTP: the status and the `Location` header.
    pub(crate) fn http_get(url: &str) -> (u16, Option<String>) {
        use std::io::{Read, Write};
        let url = Url::parse(url).unwrap();
        let mut stream = std::net::TcpStream::connect((
            url.host_str().unwrap(),
            url.port_or_known_default().unwrap(),
        ))
        .unwrap();
        let target = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let status = response[9..12].parse().unwrap();
        let location = response
            .lines()
            .take_while(|line| !line.is_empty())
            .find_map(|line| line.strip_prefix("Location: "))
            .map(str::to_string);
        (status, location)
    }

    /// Acts as the user's browser: opens the authorization URL, where the
    /// fake server agrees at once, and follows its redirect to Socium's
    /// callback after `tamper` changed it. Records the URLs it was given
    /// and the callback's status.
    pub(crate) struct Browser {
        pub(crate) opened: std::sync::Mutex<Vec<String>>,
        pub(crate) callback: std::sync::Arc<std::sync::Mutex<Option<u16>>>,
        tamper: fn(String) -> String,
    }

    impl Browser {
        pub(crate) fn new(tamper: fn(String) -> String) -> Self {
            Self {
                opened: std::sync::Mutex::new(Vec::new()),
                callback: Default::default(),
                tamper,
            }
        }

        pub(crate) fn open(&self, url: &str) {
            self.opened.lock().unwrap().push(url.to_string());
            let (url, tamper, callback) = (url.to_string(), self.tamper, self.callback.clone());
            std::thread::spawn(move || {
                let (status, location) = http_get(&url);
                assert_eq!(status, 302, "the fake server refused {url}");
                let (status, _) = http_get(&tamper(location.unwrap()));
                *callback.lock().unwrap() = Some(status);
            });
        }

        /// The status the callback answered, once the browser thread has
        /// read it: the login may return before the thread gets there.
        pub(crate) fn callback_status(&self) -> Option<u16> {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let status = *self.callback.lock().unwrap();
                if status.is_some() || std::time::Instant::now() >= deadline {
                    return status;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    pub(crate) fn oauth_settings(
        fake: &FakeHttpServer,
        name: &str,
        oauth: Option<McpOAuthSettings>,
    ) -> McpServerSettings {
        let mut settings = fake.settings(name, &[], &[]);
        if let McpTransportSettings::Http { oauth: slot, .. } = &mut settings.transport {
            *slot = oauth;
        }
        settings
    }

    fn params_of(fake: &FakeHttpServer, path: &str) -> Vec<Value> {
        fake.requests()
            .into_iter()
            .filter(|request| request["path"] == path)
            .map(|request| request["params"].clone())
            .collect()
    }

    #[test]
    fn a_login_registers_proves_pkce_and_stores_the_token_for_the_resource() {
        let fake = FakeHttpServer::start(&["--oauth", "--scopes=read,write"]);
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("mcp-auth"));
        let browser = Browser::new(|location| location);

        login(
            &oauth_settings(&fake, "fake", None),
            &store,
            &|url| browser.open(url),
            Duration::from_secs(20),
        )
        .unwrap();

        assert_eq!(browser.callback_status(), Some(200));
        let opened = browser.opened.lock().unwrap()[0].clone();
        let register = &params_of(&fake, "/auth/register")[0];
        let authorize = &params_of(&fake, "/auth/authorize")[0];
        let exchange = &params_of(&fake, "/auth/token")[0];
        let redirect = register["redirect_uris"][0].as_str().unwrap();
        assert!(redirect.starts_with("http://127.0.0.1:"), "{redirect}");
        assert!(redirect.ends_with("/callback"), "{redirect}");
        assert_eq!(authorize["redirect_uri"], redirect);
        assert_eq!(authorize["resource"], fake.url.as_str());
        assert_eq!(authorize["code_challenge_method"], "S256");
        assert_eq!(authorize["scope"], "read write");
        assert_eq!(authorize["state"].as_str().unwrap().len(), 22);
        assert_eq!(exchange["grant_type"], "authorization_code");
        assert_eq!(exchange["resource"], fake.url.as_str());
        assert_eq!(exchange["redirect_uri"], redirect);
        assert_eq!(exchange["client_id"], register_client(&fake));
        let verifier = exchange["code_verifier"].as_str().unwrap();
        assert_eq!(verifier.len(), 43);
        assert_eq!(
            authorize["code_challenge"].as_str().unwrap(),
            code_challenge(verifier)
        );
        // Only the challenge travels through the browser.
        assert!(!opened.contains(verifier), "{opened}");

        let token = store.load("fake", &fake.url).unwrap().unwrap();
        assert!(token.access_token.starts_with("access-"), "{token:?}");
        assert!(
            token
                .refresh_token
                .as_ref()
                .unwrap()
                .starts_with("refresh-")
        );
        assert!(!token.expires_within(3000), "{token:?}");
        assert_eq!(token.resource, fake.url);
        assert_eq!(token.client_auth, ClientAuth::None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path("fake"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    fn register_client(fake: &FakeHttpServer) -> String {
        params_of(fake, "/auth/authorize")[0]["client_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_configured_client_logs_in_with_its_secret_and_the_secret_is_not_stored() {
        let fake =
            FakeHttpServer::start(&["--oauth", "--no-registration", "--discovery=well-known"]);
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("mcp-auth"));
        let browser = Browser::new(|location| location);
        let oauth = McpOAuthSettings {
            client_id: Some("pre-client".into()),
            client_secret: Some("pre-secret".into()),
            scopes: vec!["repo".into()],
            callback_port: None,
        };

        login(
            &oauth_settings(&fake, "fake", Some(oauth)),
            &store,
            &|url| browser.open(url),
            Duration::from_secs(20),
        )
        .unwrap();

        assert!(params_of(&fake, "/auth/register").is_empty());
        assert_eq!(params_of(&fake, "/auth/authorize")[0]["scope"], "repo");
        let exchange = fake
            .requests()
            .into_iter()
            .find(|request| request["path"] == "/auth/token")
            .unwrap();
        assert!(exchange["basic"].as_str().unwrap().starts_with("Basic "));
        assert_eq!(exchange["params"]["client_id"], Value::Null);
        let token = store.load("fake", &fake.url).unwrap().unwrap();
        assert_eq!(
            (
                token.client_id.as_str(),
                &token.client_secret,
                token.client_auth
            ),
            ("pre-client", &None, ClientAuth::ClientSecretBasic)
        );
        let file = std::fs::read_to_string(store.path("fake")).unwrap();
        assert!(!file.contains("pre-secret"), "{file}");
    }

    #[test]
    fn a_login_fails_without_a_token_when_the_state_differs_or_the_server_refuses() {
        let fake = FakeHttpServer::start(&["--oauth"]);
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("mcp-auth"));
        let settings = oauth_settings(&fake, "fake", None);
        let run = |tamper: fn(String) -> String| {
            let browser = Browser::new(tamper);
            let result = login(
                &settings,
                &store,
                &|url| browser.open(url),
                Duration::from_secs(20),
            );
            (result.unwrap_err(), browser.callback_status())
        };

        let (err, page) = run(|location| location.replace("state=", "state=forged"));
        assert!(
            err.starts_with("Could not log in to MCP server fake: the browser came back with a state that does not match"),
            "{err}"
        );
        assert_eq!(page, Some(400));
        let (err, _) = run(|location| {
            let callback = location.split('?').next().unwrap().to_string();
            let state = location.split("state=").nth(1).unwrap().to_string();
            format!("{callback}?error=access_denied&error_description=code-1+leaked&state={state}")
        });
        assert_eq!(
            err,
            "Could not log in to MCP server fake: the authorization server refused: access_denied."
        );
        assert!(params_of(&fake, "/auth/token").is_empty());
        assert_eq!(store.load("fake", &fake.url).unwrap(), None);

        // No browser at all.
        let err = login(&settings, &store, &|_| {}, Duration::from_secs(1)).unwrap_err();
        assert!(
            err.ends_with("the browser did not come back within 1 second."),
            "{err}"
        );
    }

    #[test]
    fn a_login_refuses_a_command_server_and_a_configured_authorization_header() {
        let fake = FakeHttpServer::start(&["--oauth"]);
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("mcp-auth"));
        let with_header = fake.settings("fake", &[("authorization", "Bearer x")], &[]);
        let err = login(&with_header, &store, &|_| {}, Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("sets an Authorization header"), "{err}");
        let command = crate::mcp::tests::fake_settings("local", "", &[]);
        let err = login(&command, &store, &|_| {}, Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("runs a command"), "{err}");
    }

    #[test]
    fn a_configured_callback_port_is_the_redirect_uri() {
        let fake = FakeHttpServer::start(&["--oauth"]);
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("mcp-auth"));
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let browser = Browser::new(|location| location);
        let oauth = McpOAuthSettings {
            callback_port: Some(port),
            ..McpOAuthSettings::default()
        };

        login(
            &oauth_settings(&fake, "fake", Some(oauth)),
            &store,
            &|url| browser.open(url),
            Duration::from_secs(20),
        )
        .unwrap();

        assert_eq!(
            params_of(&fake, "/auth/authorize")[0]["redirect_uri"],
            format!("http://127.0.0.1:{port}/callback")
        );
    }

    #[test]
    fn login_and_logout_find_the_server_in_config_toml() {
        let fake = FakeHttpServer::start(&["--oauth"]);
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "[mcp.servers.fake]\nurl = \"{}\"\n[mcp.servers.gated]\nurl = \"https://a.example/mcp\"\noauth = {{ client_id = \"c\", client_secret = \"${{UNSET_SECRET}}\" }}\n[mcp.servers.local]\ncommand = [\"x\"]\n",
                fake.url
            ),
        )
        .unwrap();
        let state = home.path().join("state");
        let (config, state) = (
            config.to_str().unwrap().to_string(),
            state.to_str().unwrap().to_string(),
        );
        let lookup = move |name: &str| match name {
            "SOCIUM_CONFIG_FILE" => Some(config.clone()),
            "XDG_STATE_HOME" => Some(state.clone()),
            _ => None,
        };
        let browser = Browser::new(|location| location);

        let done = login_configured(
            &lookup,
            "fake",
            &|url| browser.open(url),
            Duration::from_secs(20),
        )
        .unwrap();

        let path = home.path().join("state/socium/mcp-auth/fake.json");
        assert_eq!(
            done,
            format!(
                "Logged in to MCP server fake. Socium keeps the token in {} and uses it from the next start.",
                path.display()
            )
        );
        assert!(path.exists());
        let err = login_configured(&lookup, "other", &|_| {}, Duration::from_secs(1)).unwrap_err();
        assert!(
            err.starts_with("No MCP server named other is configured in "),
            "{err}"
        );
        let err = login_configured(&lookup, "gated", &|_| {}, Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("${UNSET_SECRET} is not set"), "{err}");
        let err = login_configured(&lookup, "local", &|_| {}, Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("runs a command"), "{err}");

        assert_eq!(
            logout_configured(&lookup, "fake").unwrap(),
            format!("Logged out of MCP server fake: removed {}.", path.display())
        );
        assert!(!path.exists());
        assert_eq!(
            logout_configured(&lookup, "fake").unwrap(),
            "MCP server fake has no stored token."
        );
        assert!(logout_configured(&lookup, "../x").is_err());
    }

    /// A server of the fake with a token stored by a real login.
    pub(crate) fn logged_in(options: &[&str]) -> (FakeHttpServer, tempfile::TempDir, TokenStore) {
        let fake = FakeHttpServer::start(options);
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("mcp-auth"));
        let browser = Browser::new(|location| location);
        login(
            &oauth_settings(&fake, "fake", None),
            &store,
            &|url| browser.open(url),
            Duration::from_secs(20),
        )
        .unwrap();
        (fake, home, store)
    }

    /// The `Authorization` header of each MCP request.
    fn sent_authorization(fake: &FakeHttpServer) -> Vec<Value> {
        fake.requests()
            .into_iter()
            .filter(|request| request.get("rpc").is_some())
            .map(|request| request["auth"].clone())
            .collect()
    }

    fn grants(fake: &FakeHttpServer) -> Vec<String> {
        params_of(fake, "/auth/token")
            .iter()
            .map(|params| params["grant_type"].as_str().unwrap().to_string())
            .collect()
    }

    fn echo(registry: &crate::mcp::McpRegistry) -> Result<String, String> {
        registry
            .call(
                "mcp__fake__echo",
                &json!({"text": "hi"}),
                &crate::TaskControl::default(),
            )
            .expect("offered")
            .map_err(|err| err.to_string())
    }

    /// Every secret the login and its refreshes produced.
    fn secrets(fake: &FakeHttpServer) -> Vec<String> {
        let mut secrets = Vec::new();
        for params in params_of(fake, "/auth/token") {
            for key in ["code", "code_verifier", "refresh_token", "client_secret"] {
                if let Some(value) = params[key].as_str() {
                    secrets.push(value.to_string());
                }
            }
        }
        for request in fake.requests() {
            if let Some(bearer) = request["auth"].as_str() {
                secrets.push(bearer.trim_start_matches("Bearer ").to_string());
            }
        }
        secrets
    }

    #[test]
    fn a_stored_token_is_sent_as_a_bearer_and_reused_on_the_next_connect() {
        let (fake, _home, store) = logged_in(&["--oauth"]);
        let token = store.load("fake", &fake.url).unwrap().unwrap();
        let settings = oauth_settings(&fake, "fake", None);

        for _ in 0..2 {
            let (registry, warnings) = crate::mcp::tests::connect_with_tokens(
                std::slice::from_ref(&settings),
                Some(&store),
            );
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert_eq!(echo(&registry), Ok("echo: hi".into()));
        }

        let bearer = format!("Bearer {}", token.access_token);
        let sent = sent_authorization(&fake);
        // The probe of the login went without one.
        assert_eq!(sent[0], Value::Null);
        assert!(
            sent[1..].iter().all(|auth| *auth == bearer.as_str()),
            "{sent:?}"
        );
        assert_eq!(grants(&fake), ["authorization_code"]);
    }

    #[test]
    fn a_token_about_to_expire_is_renewed_and_saved() {
        // Within the renewal margin from the start.
        let (fake, _home, store) = logged_in(&["--oauth", "--expires=30"]);
        let before = store.load("fake", &fake.url).unwrap().unwrap();
        let settings = oauth_settings(&fake, "fake", None);

        let (registry, _) = crate::mcp::tests::connect_with_tokens(&[settings], Some(&store));

        assert_eq!(echo(&registry), Ok("echo: hi".into()));
        let after = store.load("fake", &fake.url).unwrap().unwrap();
        assert_ne!(after.access_token, before.access_token);
        assert_ne!(after.refresh_token, before.refresh_token);
        let refresh = &params_of(&fake, "/auth/token")[1];
        assert_eq!(refresh["grant_type"], "refresh_token");
        assert_eq!(
            refresh["refresh_token"],
            before.refresh_token.unwrap().as_str()
        );
        assert_eq!(refresh["resource"], fake.url.as_str());
        let sent = sent_authorization(&fake);
        assert_eq!(
            sent.last().unwrap(),
            &format!("Bearer {}", after.access_token).as_str()
        );
    }

    #[test]
    fn a_refused_token_is_renewed_once_and_the_request_retried() {
        let (fake, _home, store) = logged_in(&["--oauth"]);
        let mut stale = store.load("fake", &fake.url).unwrap().unwrap();
        stale.access_token = "revoked-token".into();
        store.save("fake", &stale).unwrap();

        let (registry, warnings) = crate::mcp::tests::connect_with_tokens(
            &[oauth_settings(&fake, "fake", None)],
            Some(&store),
        );

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(echo(&registry), Ok("echo: hi".into()));
        assert_eq!(grants(&fake), ["authorization_code", "refresh_token"]);
        let sent = sent_authorization(&fake);
        assert_eq!(sent[1], "Bearer revoked-token");
        assert_ne!(sent[2], "Bearer revoked-token");
    }

    #[test]
    fn without_a_usable_token_the_server_asks_for_a_login() {
        let login_warning = "MCP server fake needs you to log in: run socium mcp login fake";
        // The refresh is refused.
        let (fake, _home, store) = logged_in(&["--oauth", "--expires=30", "--refresh=fail"]);
        let (registry, warnings) = crate::mcp::tests::connect_with_tokens(
            &[oauth_settings(&fake, "fake", None)],
            Some(&store),
        );
        assert!(registry.target("mcp__fake__echo").is_none());
        assert_eq!(warnings, [login_warning]);
        let mut texts = warnings;

        // The token was stored for another URL.
        let (fake, _home, store) = logged_in(&["--oauth"]);
        let mut moved = store.load("fake", &fake.url).unwrap().unwrap();
        moved.url = "https://elsewhere.example/mcp".into();
        store.save("fake", &moved).unwrap();
        let (_, warnings) = crate::mcp::tests::connect_with_tokens(
            &[oauth_settings(&fake, "fake", None)],
            Some(&store),
        );
        assert_eq!(warnings, [login_warning]);
        assert!(sent_authorization(&fake).iter().all(Value::is_null));
        texts.extend(warnings);

        // A configured Authorization header wins over the stored token.
        let (fake, _home, store) = logged_in(&["--oauth"]);
        let settings = fake.settings("fake", &[("Authorization", "Bearer configured")], &[]);
        let (_, warnings) = crate::mcp::tests::connect_with_tokens(&[settings], Some(&store));
        assert!(
            warnings[0].contains("refused the Authorization header"),
            "{warnings:?}"
        );
        assert_eq!(
            sent_authorization(&fake).last().unwrap(),
            "Bearer configured"
        );
        texts.extend(warnings);
        texts.push(format!("{:?}", store.load("fake", &fake.url).unwrap()));

        // A call whose token can no longer be renewed.
        let (fake, _home, store) = logged_in(&["--oauth", "--expires=2", "--refresh=fail"]);
        let mut lasting = store.load("fake", &fake.url).unwrap().unwrap();
        lasting.expires_at = Some(now_seconds() + 3600);
        store.save("fake", &lasting).unwrap();
        let (registry, _) = crate::mcp::tests::connect_with_tokens(
            &[oauth_settings(&fake, "fake", None)],
            Some(&store),
        );
        std::thread::sleep(Duration::from_millis(2500));
        let err = echo(&registry).unwrap_err();
        assert_eq!(err, login_warning);
        texts.push(err);

        for secret in secrets(&fake) {
            for text in &texts {
                assert!(!text.contains(&secret), "{text} holds {secret}");
            }
        }
    }

    #[test]
    fn the_code_challenge_matches_rfc_7636() {
        // RFC 7636 appendix B.
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mJ92K6Wy-dA-l5JSiEqWhcCUAnXc3yqOFzAvpIIU0ZZKLpRwnSXg8"),
            "qrXhlVndRVbSmVRR1eZZGpi1j1q-9zXQQcaH37ltwMA"
        );
        assert_ne!(random_text(32), random_text(32));
        assert_eq!(form_encode("a b+c:d"), "a+b%2Bc%3Ad");
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
