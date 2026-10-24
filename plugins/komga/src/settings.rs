//! The plugin's settings: what the user configures in Ito, and how it becomes a request.
//!
//! Settings are read from the host on every call (see [`Settings::load`]) and nothing is
//! cached, so a change in the settings screen applies to the very next request.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ito_rs::models::{Setting, SettingIssue, SettingsSchema};

use crate::error::{self, Result};
use crate::host::Host;

pub const SERVER_URL: &str = "server_url";
pub const API_KEY: &str = "api_key";
pub const USERNAME: &str = "username";
pub const PASSWORD: &str = "password";

/// The settings screen Ito shows for this plugin.
pub fn schema() -> SettingsSchema {
    SettingsSchema {
        settings: vec![
            Setting::Text {
                id: SERVER_URL.into(),
                name: "Server URL".into(),
                summary: Some("https://komga.example.com".into()),
                default_value: String::new(),
            },
            Setting::Secret {
                id: API_KEY.into(),
                name: "API key".into(),
                summary: Some("Komga ≥ 1.20: Account settings → API keys".into()),
            },
            Setting::Text {
                id: USERNAME.into(),
                name: "Username".into(),
                summary: Some("Only used when no API key is set".into()),
                default_value: String::new(),
            },
            Setting::Secret {
                id: PASSWORD.into(),
                name: "Password".into(),
                summary: None,
            },
        ],
    }
}

/// How requests prove who is asking.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    ApiKey(String),
    Basic { username: String, password: String },
}

impl Auth {
    /// The header that carries the credentials.
    pub fn header(&self) -> (&'static str, String) {
        match self {
            Auth::ApiKey(key) => ("X-API-Key", key.clone()),
            Auth::Basic { username, password } => (
                "Authorization",
                format!(
                    "Basic {}",
                    STANDARD.encode(format!("{username}:{password}"))
                ),
            ),
        }
    }
}

// Credentials must never reach a log or a failed assertion.
impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Auth::ApiKey(_) => f.write_str("Auth::ApiKey(<redacted>)"),
            Auth::Basic { .. } => f.write_str("Auth::Basic(<redacted>)"),
        }
    }
}

/// A usable configuration: where the server is and how to authenticate to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Without a trailing slash, so paths are appended as `{server_url}/api/v1/...`.
    pub server_url: String,
    pub auth: Auth,
}

impl Settings {
    /// Reads the settings from the host. Fails with a sentence for the user if the plugin
    /// is not configured.
    pub fn load(host: &impl Host) -> Result<Self> {
        Self::from_values(
            host.get_setting(SERVER_URL)?,
            host.get_setting(API_KEY)?,
            host.get_setting(USERNAME)?,
            host.get_setting(PASSWORD)?,
        )
    }

    /// Settings from raw stored values. A missing or blank value counts as unset.
    ///
    /// An API key wins over a username and password; a username without a password (or the
    /// reverse) is not a login.
    fn from_values(
        server_url: Option<String>,
        api_key: Option<String>,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<Self> {
        let server_url = server_url
            .as_deref()
            .and_then(normalize_server_url)
            .ok_or_else(error::not_configured)?;

        let api_key = api_key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty());
        let username = username
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        // Passwords may legitimately start or end with spaces, so only an empty one is unset.
        let password = password.filter(|password| !password.is_empty());

        let auth = match (api_key, username, password) {
            (Some(key), _, _) => Auth::ApiKey(key),
            (None, Some(username), Some(password)) => Auth::Basic { username, password },
            _ => return Err(error::missing_credentials()),
        };
        Ok(Self { server_url, auth })
    }
}

/// Trims whitespace and trailing slashes, so `" https://komga.example.com/ "` becomes
/// `"https://komga.example.com"`. A path prefix (`https://example.com/komga`) is kept.
/// Returns `None` for an empty value.
pub fn normalize_server_url(raw: &str) -> Option<String> {
    let url = raw
        .trim_start()
        .trim_end_matches(|c: char| c == '/' || c.is_whitespace());
    if url.is_empty() {
        None
    } else {
        Some(url.to_owned())
    }
}

/// What is missing or malformed in the settings, checked locally: no request is made and
/// nothing is written. Messages never repeat a value the user entered. An empty list means the
/// plugin can try to connect; only the server can say whether the credentials are accepted.
pub fn validate(host: &impl Host) -> ito_rs::Result<Vec<SettingIssue>> {
    let server_url = host.get_setting(SERVER_URL)?;
    let has_api_key = host
        .get_setting(API_KEY)?
        .is_some_and(|key| !key.trim().is_empty());
    let has_username = host
        .get_setting(USERNAME)?
        .is_some_and(|name| !name.trim().is_empty());
    let has_password = host
        .get_setting(PASSWORD)?
        .is_some_and(|password| !password.is_empty());

    let mut issues = Vec::new();
    match server_url.as_deref().and_then(normalize_server_url) {
        None => issues.push(issue(SERVER_URL, "Enter your Komga server URL.")),
        Some(url) if !has_http_scheme(&url) => issues.push(issue(
            SERVER_URL,
            "The server URL must start with http:// or https://.",
        )),
        Some(_) => {}
    }
    if !has_api_key {
        match (has_username, has_password) {
            (true, true) => {}
            (true, false) => issues.push(issue(PASSWORD, "Enter the password for this username.")),
            (false, true) => issues.push(issue(USERNAME, "Enter the username for this password.")),
            (false, false) => issues.push(issue(
                API_KEY,
                "Enter an API key, or a username and password.",
            )),
        }
    }
    Ok(issues)
}

fn has_http_scheme(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["http://", "https://"].iter().any(|scheme| {
        lower
            .strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty())
    })
}

fn issue(key: &str, message: &str) -> SettingIssue {
    SettingIssue {
        key: key.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{MISSING_CREDENTIALS, NOT_CONFIGURED};
    use crate::testing::{FakeHost, message};

    fn values(
        server_url: Option<&str>,
        api_key: Option<&str>,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<Settings> {
        Settings::from_values(
            server_url.map(str::to_owned),
            api_key.map(str::to_owned),
            username.map(str::to_owned),
            password.map(str::to_owned),
        )
    }

    #[test]
    fn server_url_is_trimmed_of_whitespace_and_trailing_slashes() {
        for (raw, expected) in [
            ("https://komga.example.com", "https://komga.example.com"),
            ("https://komga.example.com/", "https://komga.example.com"),
            ("https://komga.example.com///", "https://komga.example.com"),
            ("  https://komga.example.com  ", "https://komga.example.com"),
            (
                "\thttps://komga.example.com/\n",
                "https://komga.example.com",
            ),
            (
                " https://komga.example.com/ / ",
                "https://komga.example.com",
            ),
            ("http://192.168.1.20:25600/", "http://192.168.1.20:25600"),
            ("https://example.com/komga/", "https://example.com/komga"),
        ] {
            assert_eq!(
                normalize_server_url(raw).as_deref(),
                Some(expected),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn an_empty_server_url_is_unset() {
        for raw in ["", "   ", "/", " / ", "\n"] {
            assert_eq!(normalize_server_url(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn api_key_is_used_when_set() {
        let settings = values(Some("https://k.test"), Some("key-123"), None, None).unwrap();
        assert_eq!(settings.server_url, "https://k.test");
        assert_eq!(settings.auth, Auth::ApiKey("key-123".into()));
        assert_eq!(settings.auth.header(), ("X-API-Key", "key-123".to_string()));
    }

    #[test]
    fn api_key_is_preferred_over_username_and_password() {
        let settings = values(
            Some("https://k.test"),
            Some("key-123"),
            Some("user"),
            Some("pass"),
        )
        .unwrap();
        assert_eq!(settings.auth, Auth::ApiKey("key-123".into()));
    }

    #[test]
    fn username_and_password_are_the_fallback() {
        let settings = values(Some("https://k.test"), None, Some("user"), Some("pass")).unwrap();
        assert_eq!(
            settings.auth,
            Auth::Basic {
                username: "user".into(),
                password: "pass".into()
            }
        );
        // base64("user:pass")
        assert_eq!(
            settings.auth.header(),
            ("Authorization", "Basic dXNlcjpwYXNz".to_string())
        );
    }

    #[test]
    fn basic_credentials_are_encoded_as_utf8() {
        let auth = Auth::Basic {
            username: "rené".into(),
            password: "pässword".into(),
        };
        // base64("rené:pässword")
        assert_eq!(auth.header().1, "Basic cmVuw6k6cMOkc3N3b3Jk");
    }

    #[test]
    fn blank_values_count_as_unset() {
        // A blank API key falls through to the login.
        let settings = values(
            Some("https://k.test"),
            Some("   "),
            Some("user"),
            Some("pass"),
        )
        .unwrap();
        assert!(matches!(settings.auth, Auth::Basic { .. }));
        let settings =
            values(Some("https://k.test"), Some(""), Some("user"), Some("pass")).unwrap();
        assert!(matches!(settings.auth, Auth::Basic { .. }));
    }

    #[test]
    fn credentials_are_trimmed_but_a_password_keeps_its_spaces() {
        let settings = values(Some("https://k.test"), Some("  key-123\n"), None, None).unwrap();
        assert_eq!(settings.auth, Auth::ApiKey("key-123".into()));

        let settings =
            values(Some("https://k.test"), None, Some(" user "), Some(" pass ")).unwrap();
        assert_eq!(
            settings.auth,
            Auth::Basic {
                username: "user".into(),
                password: " pass ".into()
            }
        );
    }

    #[test]
    fn no_server_url_is_a_not_configured_error() {
        for server_url in [None, Some(""), Some("  /  ")] {
            let err = values(server_url, Some("key"), None, None).unwrap_err();
            assert_eq!(message(err), NOT_CONFIGURED);
        }
        // Nothing configured at all.
        assert_eq!(
            message(values(None, None, None, None).unwrap_err()),
            NOT_CONFIGURED
        );
    }

    #[test]
    fn no_credentials_is_a_missing_credentials_error() {
        for (key, user, pass) in [
            (None, None, None),
            (Some("  "), None, None),
            (None, Some("user"), None),
            (None, None, Some("pass")),
            (None, Some("user"), Some("")),
            (None, Some("  "), Some("pass")),
        ] {
            let err = values(Some("https://k.test"), key, user, pass).unwrap_err();
            assert_eq!(message(err), MISSING_CREDENTIALS);
        }
    }

    #[test]
    fn the_error_sentences_tell_the_user_what_to_do() {
        assert_eq!(
            NOT_CONFIGURED,
            "Add your Komga server URL and API key in the Komga plugin settings."
        );
        assert_eq!(
            MISSING_CREDENTIALS,
            "Add your Komga API key, or your username and password, in the Komga plugin settings."
        );
    }

    #[test]
    fn load_reads_the_current_values_from_the_host_every_time() {
        let host = FakeHost::new()
            .setting(SERVER_URL, "https://one.test/")
            .setting(API_KEY, "key-one");
        assert_eq!(
            Settings::load(&host).unwrap().server_url,
            "https://one.test"
        );

        host.set_setting(SERVER_URL, "https://two.test");
        host.set_setting(API_KEY, "key-two");
        let settings = Settings::load(&host).unwrap();
        assert_eq!(settings.server_url, "https://two.test");
        assert_eq!(settings.auth, Auth::ApiKey("key-two".into()));
    }

    #[test]
    fn load_with_an_empty_host_is_not_configured() {
        let err = Settings::load(&FakeHost::new()).unwrap_err();
        assert_eq!(message(err), NOT_CONFIGURED);
    }

    #[test]
    fn debug_output_never_contains_credentials() {
        let key = Auth::ApiKey("super-secret-key".into());
        let basic = Auth::Basic {
            username: "someone".into(),
            password: "super-secret-password".into(),
        };
        let settings = Settings {
            server_url: "https://k.test".into(),
            auth: basic.clone(),
        };
        for rendered in [
            format!("{key:?}"),
            format!("{basic:?}"),
            format!("{settings:?}"),
        ] {
            assert!(!rendered.contains("secret"), "{rendered}");
            assert!(!rendered.contains("someone"), "{rendered}");
        }
    }

    #[test]
    fn the_schema_declares_the_four_settings_the_host_expects() {
        let schema = schema();
        let ids: Vec<&str> = schema
            .settings
            .iter()
            .map(|setting| match setting {
                Setting::Text { id, .. } | Setting::Secret { id, .. } => id.as_str(),
                other => panic!("unexpected setting {other:?}"),
            })
            .collect();
        assert_eq!(ids, ["server_url", "api_key", "username", "password"]);

        // Credentials are secrets, so the host keeps them out of plain storage and backups.
        assert!(matches!(&schema.settings[0], Setting::Text { name, .. } if name == "Server URL"));
        assert!(matches!(&schema.settings[1], Setting::Secret { name, .. } if name == "API key"));
        assert!(matches!(&schema.settings[2], Setting::Text { name, .. } if name == "Username"));
        assert!(matches!(&schema.settings[3], Setting::Secret { name, .. } if name == "Password"));
        assert!(matches!(
            &schema.settings[0],
            Setting::Text { summary: Some(hint), default_value, .. }
                if hint == "https://komga.example.com" && default_value.is_empty()
        ));
        assert!(matches!(
            &schema.settings[1],
            Setting::Secret { summary: Some(hint), .. }
                if hint == "Komga ≥ 1.20: Account settings → API keys"
        ));
    }

    #[test]
    fn the_schema_survives_the_wire_format_the_host_decodes() {
        let bytes = ito_rs::postcard::to_allocvec(&schema()).unwrap();
        let decoded: SettingsSchema = ito_rs::postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.settings.len(), 4);
        assert!(matches!(&decoded.settings[1], Setting::Secret { id, .. } if id == "api_key"));
        assert!(matches!(&decoded.settings[3], Setting::Secret { id, .. } if id == "password"));
    }

    fn issues(host: &FakeHost) -> Vec<(String, String)> {
        validate(host)
            .unwrap()
            .into_iter()
            .map(|issue| (issue.key, issue.message))
            .collect()
    }

    #[test]
    fn validation_names_the_missing_fields_of_a_new_install() {
        assert_eq!(
            issues(&FakeHost::new()),
            [
                (SERVER_URL.into(), "Enter your Komga server URL.".into()),
                (
                    API_KEY.into(),
                    "Enter an API key, or a username and password.".into()
                ),
            ]
        );
    }

    #[test]
    fn validation_accepts_what_the_plugin_can_connect_with() {
        let api_key = FakeHost::new()
            .setting(SERVER_URL, " HTTPS://komga.example.com/ ")
            .setting(API_KEY, " key ");
        assert!(issues(&api_key).is_empty());
        let login = FakeHost::new()
            .setting(SERVER_URL, "http://10.0.0.2:25600/komga")
            .setting(USERNAME, "reader")
            // A password made of spaces is a password, as it is when signing in.
            .setting(PASSWORD, "  ");
        assert!(issues(&login).is_empty());
    }

    #[test]
    fn validation_points_at_the_half_of_a_login_that_is_missing() {
        let no_password = FakeHost::new()
            .setting(SERVER_URL, "https://komga.example.com")
            .setting(USERNAME, "reader");
        assert_eq!(
            issues(&no_password),
            [(
                PASSWORD.into(),
                "Enter the password for this username.".into()
            )]
        );
        let no_username = FakeHost::new()
            .setting(SERVER_URL, "https://komga.example.com")
            .setting(USERNAME, "   ")
            .setting(PASSWORD, "secret");
        assert_eq!(
            issues(&no_username),
            [(
                USERNAME.into(),
                "Enter the username for this password.".into()
            )]
        );
    }

    #[test]
    fn validation_rejects_urls_the_host_cannot_fetch_without_repeating_them() {
        for raw in [
            "komga.example.com",
            "ftp://komga.example.com",
            "https://",
            "javascript:alert(1)",
        ] {
            let host = FakeHost::new()
                .setting(SERVER_URL, raw)
                .setting(API_KEY, "key");
            let found = issues(&host);
            assert_eq!(
                found,
                [(
                    SERVER_URL.into(),
                    "The server URL must start with http:// or https://.".into()
                )],
                "{raw}"
            );
        }
    }
}
