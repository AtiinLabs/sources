//! Errors the user can act on.
//!
//! Ito's runner only logs a plugin's error, but the text is what a log reader (or a host
//! that surfaces it) sees, so each one is a full sentence that says what to do. They carry
//! an HTTP status at most: never a response body, a header, a URL or a credential.
//!
//! Inside the plugin a problem is a [`Failure`], so a caller can tell a missing setting from
//! a rejected sign-in without reading a sentence; it becomes an [`Error`] at the boundary
//! (`?` in a function that returns `ito_rs::Result` converts it).

use ito_rs::Error;

use crate::settings::Auth;

pub const NOT_CONFIGURED: &str =
    "Add your Komga server URL and API key in the Komga plugin settings.";
pub const MISSING_CREDENTIALS: &str =
    "Add your Komga API key, or your username and password, in the Komga plugin settings.";
const UNREACHABLE: &str =
    "Couldn't connect to Komga. Check the server URL and that the server is running.";
const UNREADABLE: &str =
    "Komga sent a response Ito couldn't read. Check that the server URL points to Komga.";
const IMAGE_PATH: &str = "Komga images must be requested with a path that starts with api/v1/.";

pub type Result<T, E = Failure> = std::result::Result<T, E>;

/// Why an operation failed.
#[derive(Debug)]
pub enum Failure {
    /// No usable server URL.
    NotConfigured,
    /// A server URL but nothing to authenticate with.
    MissingCredentials,
    /// The server refused the credentials (HTTP 401) or the account (HTTP 403). Holds the
    /// sentence for the user.
    Rejected(String),
    /// The host could not perform the request at all.
    Unreachable,
    /// Anything else: a response that isn't Komga's JSON, an unexpected status, a refused
    /// image path, or an error from the host itself.
    Other(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Failure::Other(error)
    }
}

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::NotConfigured => host(NOT_CONFIGURED),
            Failure::MissingCredentials => host(MISSING_CREDENTIALS),
            Failure::Rejected(message) => host(message),
            Failure::Unreachable => host(UNREACHABLE),
            Failure::Other(error) => error,
        }
    }
}

fn host(message: impl Into<String>) -> Error {
    Error::Host(message.into())
}

fn other(message: impl Into<String>) -> Failure {
    Failure::Other(host(message))
}

/// The request body could not be serialised.
pub fn unencodable_request(err: impl std::fmt::Display) -> Failure {
    other(format!("Couldn't encode a request for Komga: {err}"))
}

/// No usable server URL.
pub fn not_configured() -> Failure {
    Failure::NotConfigured
}

/// A server URL but nothing to authenticate with.
pub fn missing_credentials() -> Failure {
    Failure::MissingCredentials
}

/// The host could not perform the request at all.
pub fn unreachable() -> Failure {
    Failure::Unreachable
}

/// The server answered with something that is not the JSON Komga documents.
pub fn unreadable_response() -> Failure {
    other(UNREADABLE)
}

pub fn invalid_image_path() -> Failure {
    other(IMAGE_PATH)
}

/// `Ok` for a 2xx status, otherwise the sentence for that failure.
///
/// The runner reports a request it could not make (no route to the host, a bad URL, a TLS
/// failure) as HTTP 500, so a 5xx can mean either the server or the connection.
pub fn check_status(status: i32, auth: &Auth) -> Result<()> {
    let failure = match status {
        200..=299 => return Ok(()),
        401 => Failure::Rejected(match auth {
            Auth::ApiKey(_) => "Komga rejected the API key (HTTP 401).".to_owned(),
            Auth::Basic { .. } => "Komga rejected the username or password (HTTP 401).".to_owned(),
        }),
        403 => Failure::Rejected(
            "Komga denied access (HTTP 403). Check this account's permissions and library access."
                .to_owned(),
        ),
        404 => other(
            "Komga couldn't find that (HTTP 404). Check the server URL, or the item may have been removed.",
        ),
        500..=599 => other(format!(
            "Komga didn't respond properly (HTTP {status}). Check the server URL and that the server is running."
        )),
        _ => other(format!(
            "Komga returned an unexpected response (HTTP {status})."
        )),
    };
    Err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::message;

    fn api_key() -> Auth {
        Auth::ApiKey("key".into())
    }

    fn basic() -> Auth {
        Auth::Basic {
            username: "user".into(),
            password: "pass".into(),
        }
    }

    #[test]
    fn success_statuses_pass() {
        for status in [200, 201, 204, 299] {
            assert!(check_status(status, &api_key()).is_ok(), "{status}");
        }
    }

    #[test]
    fn a_rejected_api_key_is_named_as_such() {
        assert_eq!(
            message(check_status(401, &api_key()).unwrap_err()),
            "Komga rejected the API key (HTTP 401)."
        );
    }

    #[test]
    fn a_rejected_login_is_named_as_such() {
        assert_eq!(
            message(check_status(401, &basic()).unwrap_err()),
            "Komga rejected the username or password (HTTP 401)."
        );
    }

    #[test]
    fn other_failures_say_what_to_check() {
        assert_eq!(
            message(check_status(403, &api_key()).unwrap_err()),
            "Komga denied access (HTTP 403). Check this account's permissions and library access."
        );
        assert_eq!(
            message(check_status(404, &api_key()).unwrap_err()),
            "Komga couldn't find that (HTTP 404). Check the server URL, or the item may have been removed."
        );
        for status in [500, 502, 503, 599] {
            assert_eq!(
                message(check_status(status, &api_key()).unwrap_err()),
                format!(
                    "Komga didn't respond properly (HTTP {status}). Check the server URL and that the server is running."
                )
            );
        }
        for status in [301, 400, 418] {
            assert_eq!(
                message(check_status(status, &api_key()).unwrap_err()),
                format!("Komga returned an unexpected response (HTTP {status}).")
            );
        }
    }

    #[test]
    fn errors_are_host_errors_so_the_sentence_is_the_message() {
        assert!(matches!(Error::from(unreachable()), Error::Host(_)));
        assert!(matches!(Error::from(unreadable_response()), Error::Host(_)));
        assert!(matches!(Error::from(invalid_image_path()), Error::Host(_)));
        assert_eq!(
            message(unreachable()),
            "Couldn't connect to Komga. Check the server URL and that the server is running."
        );
    }

    #[test]
    fn only_a_401_or_403_is_a_rejection() {
        for status in [401, 403] {
            assert!(
                matches!(check_status(status, &api_key()), Err(Failure::Rejected(_))),
                "{status}"
            );
        }
        for status in [301, 400, 404, 418, 500, 503] {
            assert!(
                matches!(check_status(status, &api_key()), Err(Failure::Other(_))),
                "{status}"
            );
        }
    }
}
