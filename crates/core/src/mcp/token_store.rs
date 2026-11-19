//! Where `socium mcp login` keeps a remote server's OAuth tokens (ADR 22):
//! `$XDG_STATE_HOME/socium/mcp-auth/<server>.json`, else
//! `~/.local/state/socium/mcp-auth/<server>.json`. That is per user and
//! outside every workspace, so neither the model nor a project can read or
//! plant one. The directory is 0700, each file 0600 and replaced
//! atomically.

use crate::agent::state::{atomic_write_with_mode, read_state_bytes};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A token file is small; anything larger is not one.
const MAX_TOKEN_FILE_BYTES: u64 = 256 * 1024;

/// The tokens and client one login produced.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredToken {
    /// The server's `url` when the login ran. A token for another URL is
    /// ignored, so changing the URL never sends it elsewhere.
    pub(crate) url: String,
    /// The `resource` the token was requested for (RFC 8707).
    pub(crate) resource: String,
    pub(crate) access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    /// Seconds since the Unix epoch; `None` when the server gave no
    /// lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at: Option<u64>,
    pub(crate) token_endpoint: String,
    pub(crate) client_id: String,
    /// Only for a client the login registered; a configured secret stays
    /// in the environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_secret: Option<String>,
    #[serde(default)]
    pub(crate) client_auth: ClientAuth,
}

/// How the client proves itself at the token endpoint (RFC 6749 2.3.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientAuth {
    /// A public client: `client_id` in the form only.
    #[default]
    None,
    /// HTTP Basic with the client ID and secret.
    ClientSecretBasic,
    /// The secret in the form.
    ClientSecretPost,
}

impl std::fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hidden = |value: &Option<String>| value.as_ref().map(|_| "<hidden>");
        f.debug_struct("StoredToken")
            .field("url", &self.url)
            .field("resource", &self.resource)
            .field("access_token", &"<hidden>")
            .field("refresh_token", &hidden(&self.refresh_token))
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("client_secret", &hidden(&self.client_secret))
            .field("client_auth", &self.client_auth)
            .finish()
    }
}

impl StoredToken {
    /// Whether the access token expires within `margin` seconds.
    pub(crate) fn expires_within(&self, margin: u64) -> bool {
        self.expires_at
            .is_some_and(|at| now_seconds().saturating_add(margin) >= at)
    }
}

pub(crate) fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The token directory of one user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenStore {
    dir: PathBuf,
}

impl TokenStore {
    /// `$XDG_STATE_HOME/socium/mcp-auth` (an absolute value only), else
    /// `$HOME/.local/state/socium/mcp-auth`; `None` without either.
    pub(crate) fn from_env(lookup: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let state_home = lookup("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                lookup("HOME")
                    .filter(|home| !home.is_empty())
                    .map(|home| Path::new(&home).join(".local/state"))
            })?;
        Some(Self::at(state_home.join("socium").join("mcp-auth")))
    }

    pub(crate) fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file of `server`, whose name settings validated, so it has no
    /// path separator.
    pub(crate) fn path(&self, server: &str) -> PathBuf {
        self.dir.join(format!("{server}.json"))
    }

    /// The token stored for `server` at `url`. `None` when there is none or
    /// it was stored for another URL. An unreadable file is an error that
    /// never quotes its content.
    pub(crate) fn load(&self, server: &str, url: &str) -> io::Result<Option<StoredToken>> {
        let path = self.path(server);
        let Some(bytes) = read_state_bytes(&path, MAX_TOKEN_FILE_BYTES)? else {
            return Ok(None);
        };
        let token: StoredToken = serde_json::from_slice(&bytes).map_err(|_| {
            Error::new(
                ErrorKind::InvalidData,
                format!("{} is not a Socium token file", path.display()),
            )
        })?;
        Ok((token.url == url).then_some(token))
    }

    /// Replaces the token of `server`, creating the directory owner-only.
    pub(crate) fn save(&self, server: &str, token: &StoredToken) -> io::Result<()> {
        self.prepare_dir()?;
        let bytes = serde_json::to_vec_pretty(token).map_err(Error::other)?;
        atomic_write_with_mode(&self.path(server), &bytes, Some(0o600))
    }

    /// Deletes the token of `server`; `false` when there was none.
    pub(crate) fn remove(&self, server: &str) -> io::Result<bool> {
        match fs::remove_file(self.path(server)) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Creates `socium/mcp-auth` 0700 and refuses one that is a symlink or
    /// belongs to another user.
    fn prepare_dir(&self) -> io::Result<()> {
        if let Some(parent) = self.dir.parent().and_then(Path::parent) {
            fs::create_dir_all(parent)?;
        }
        for dir in [self.dir.parent(), Some(self.dir.as_path())]
            .into_iter()
            .flatten()
        {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(dir) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }
        let metadata = fs::symlink_metadata(&self.dir)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::other(format!(
                "{} must be a directory, not a link",
                self.dir.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            // SAFETY: geteuid has no arguments or memory preconditions.
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(Error::other(format!(
                    "{} belongs to another user",
                    self.dir.display()
                )));
            }
            fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn token(url: &str) -> StoredToken {
        StoredToken {
            url: url.into(),
            resource: url.into(),
            access_token: "access-s3cret".into(),
            refresh_token: Some("refresh-s3cret".into()),
            expires_at: Some(now_seconds() + 3600),
            token_endpoint: "https://auth.example/token".into(),
            client_id: "client-1".into(),
            client_secret: Some("client-s3cret".into()),
            client_auth: ClientAuth::ClientSecretBasic,
        }
    }

    #[test]
    fn the_store_lives_under_the_state_home() {
        let lookup = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(
            TokenStore::from_env(&lookup(&[("XDG_STATE_HOME", "/s"), ("HOME", "/h")])).unwrap(),
            TokenStore::at("/s/socium/mcp-auth".into())
        );
        // A relative XDG_STATE_HOME is ignored, as the XDG spec says.
        assert_eq!(
            TokenStore::from_env(&lookup(&[("XDG_STATE_HOME", "s"), ("HOME", "/h")])).unwrap(),
            TokenStore::at("/h/.local/state/socium/mcp-auth".into())
        );
        assert_eq!(TokenStore::from_env(&lookup(&[])), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_token_is_saved_private_reloaded_for_its_url_only_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("state/socium/mcp-auth"));
        let url = "https://mcp.example/mcp";
        assert_eq!(store.load("github", url).unwrap(), None);

        store.save("github", &token(url)).unwrap();

        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.dir()), 0o700);
        assert_eq!(mode(store.dir().parent().unwrap()), 0o700);
        assert_eq!(mode(&store.path("github")), 0o600);
        assert_eq!(store.load("github", url).unwrap(), Some(token(url)));
        assert_eq!(
            store.load("github", "https://other.example/mcp").unwrap(),
            None
        );
        assert_eq!(store.load("other", url).unwrap(), None);
        // Replacing keeps the file private and leaves no temporary file.
        let mut refreshed = token(url);
        refreshed.access_token = "new-access".into();
        store.save("github", &refreshed).unwrap();
        assert_eq!(mode(&store.path("github")), 0o600);
        assert_eq!(fs::read_dir(store.dir()).unwrap().count(), 1);
        assert_eq!(
            store.load("github", url).unwrap().unwrap().access_token,
            "new-access"
        );

        assert!(store.remove("github").unwrap());
        assert!(!store.remove("github").unwrap());
        assert_eq!(store.load("github", url).unwrap(), None);
    }

    #[test]
    fn neither_debug_nor_a_broken_file_shows_a_secret() {
        let shown = format!("{:?}", token("https://mcp.example/mcp"));
        assert!(shown.contains("client-1"), "{shown}");
        assert!(!shown.contains("s3cret"), "{shown}");

        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().to_path_buf());
        fs::write(store.path("x"), "{\"access_token\": \"s3cret\"").unwrap();
        let err = store.load("x", "https://mcp.example/mcp").unwrap_err();
        assert!(
            err.to_string().contains("is not a Socium token file"),
            "{err}"
        );
        assert!(!err.to_string().contains("s3cret"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_token_file_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let store = TokenStore::at(home.path().join("mcp-auth"));
        store
            .save("real", &token("https://mcp.example/mcp"))
            .unwrap();
        std::os::unix::fs::symlink(store.path("real"), store.path("link")).unwrap();

        assert!(store.load("link", "https://mcp.example/mcp").is_err());
    }
}
