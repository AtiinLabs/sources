//! Test support: a scripted [`Host`] and the Komga responses the tests replay.

use std::cell::RefCell;
use std::collections::HashMap;

use ito_rs::net::NetResponse;
use ito_rs::{Error, Result};

use crate::host::{Host, HttpRequest, Method};

/// The server URL [`FakeHost::configured`] sets.
pub const BASE: &str = "https://komga.test";

/// Responses captured from a Komga 1.29.1 server (`gotson/komga`) and replayed as they came.
///
/// The server held four series: Night Harbor, Paper Lanterns and Glass Orchard (image
/// archives) and Vertical Rain (an archive, a text EPUB, an image EPUB, a file Komga could not
/// read, and a PDF), plus a series and a book that were deleted from disk after a first
/// scan, so Komga flags them `deleted`. Metadata was filled in through Komga's own API and
/// some books were read, so every status, reading direction and read state appears.
pub mod fixtures {
    pub const LIBRARIES: &str = include_str!("../tests/fixtures/libraries.json");
    pub const SERIES_LIST: &str = include_str!("../tests/fixtures/series_list.json");
    pub const SERIES_LIST_FIRST_PAGE: &str =
        include_str!("../tests/fixtures/series_list_first_page.json");
    /// The same kind of response from a library nobody has edited: empty strings throughout.
    pub const SERIES_LIST_PLAIN: &str = include_str!("../tests/fixtures/series_list_plain.json");
    pub const SERIES_SEARCH_HARBOR: &str =
        include_str!("../tests/fixtures/series_search_harbor.json");
    pub const SERIES_NIGHT_HARBOR: &str =
        include_str!("../tests/fixtures/series_night_harbor.json");
    pub const SERIES_NEW: &str = include_str!("../tests/fixtures/series_new.json");
    pub const SERIES_UPDATED: &str = include_str!("../tests/fixtures/series_updated.json");
    pub const BOOKS_NIGHT_HARBOR: &str = include_str!("../tests/fixtures/books_night_harbor.json");
    pub const BOOKS_VERTICAL_RAIN: &str =
        include_str!("../tests/fixtures/books_vertical_rain.json");
    pub const BOOKS_PAPER_LANTERNS: &str =
        include_str!("../tests/fixtures/books_paper_lanterns.json");
    pub const BOOKS_IN_PROGRESS: &str = include_str!("../tests/fixtures/books_in_progress.json");
    pub const BOOKS_ONDECK: &str = include_str!("../tests/fixtures/books_ondeck.json");
    pub const PAGES_NIGHT_HARBOR_V01: &str =
        include_str!("../tests/fixtures/pages_night_harbor_v01.json");
}

/// The text of a plugin error (an [`Error`] or a [`crate::error::Failure`]), which must be one
/// carrying a sentence.
pub fn message(error: impl Into<Error>) -> String {
    match error.into() {
        Error::Host(message)
        | Error::NeedsSetup(message)
        | Error::Unauthorized(message)
        | Error::Net(message)
        | Error::Server(message)
        | Error::NotFound(message) => message,
        other => panic!("expected an error with a sentence, got {other:?}"),
    }
}

enum Outcome {
    Respond {
        status: i32,
        body: Vec<u8>,
    },
    /// The host could not make the request at all.
    Fail,
}

struct Route {
    method: Method,
    url: String,
    outcome: Outcome,
}

/// A [`Host`] with stored settings and canned responses. It records every request it gets
/// and panics on one nothing was scripted for, so a test fails loudly on an unexpected call.
pub struct FakeHost {
    settings: RefCell<HashMap<String, String>>,
    routes: RefCell<Vec<Route>>,
    requests: RefCell<Vec<HttpRequest>>,
}

impl FakeHost {
    /// No settings and no routes: a freshly installed plugin.
    pub fn new() -> Self {
        Self {
            settings: RefCell::default(),
            routes: RefCell::default(),
            requests: RefCell::default(),
        }
    }

    /// A server URL and an API key, the minimum for the plugin to work.
    pub fn configured() -> Self {
        Self::new()
            .setting("server_url", BASE)
            .setting("api_key", "test-api-key")
    }

    pub fn setting(self, key: &str, value: &str) -> Self {
        self.set_setting(key, value);
        self
    }

    /// Changes a setting mid-test, as the user does in Ito's settings screen.
    pub fn set_setting(&self, key: &str, value: &str) {
        self.settings
            .borrow_mut()
            .insert(key.to_owned(), value.to_owned());
    }

    /// Answers `method url` with `status` and `body`. A later route for the same request
    /// replaces an earlier one.
    pub fn respond(self, method: Method, url: &str, status: i32, body: impl AsRef<[u8]>) -> Self {
        self.add_response(method, url, status, body);
        self
    }

    pub fn add_response(&self, method: Method, url: &str, status: i32, body: impl AsRef<[u8]>) {
        self.add_route(
            method,
            url,
            Outcome::Respond {
                status,
                body: body.as_ref().to_vec(),
            },
        );
    }

    /// Answers `method {BASE}{path}` with a captured response.
    pub fn json(self, method: Method, path: &str, fixture: &str) -> Self {
        self.respond(method, &format!("{BASE}{path}"), 200, fixture)
    }

    /// Makes `method url` fail the way a dead connection does.
    pub fn fail(self, method: Method, url: &str) -> Self {
        self.add_route(method, url, Outcome::Fail);
        self
    }

    fn add_route(&self, method: Method, url: &str, outcome: Outcome) {
        self.routes.borrow_mut().push(Route {
            method,
            url: url.to_owned(),
            outcome,
        });
    }

    /// Every request sent so far, oldest first.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.borrow().clone()
    }
}

impl Host for FakeHost {
    fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.settings.borrow().get(key).cloned())
    }

    fn send(&self, request: &HttpRequest) -> Result<NetResponse> {
        self.requests.borrow_mut().push(request.clone());
        let routes = self.routes.borrow();
        let route = routes
            .iter()
            .rev()
            .find(|route| route.method == request.method && route.url == request.url)
            .unwrap_or_else(|| {
                panic!("no route scripted for {:?} {}", request.method, request.url)
            });
        match &route.outcome {
            Outcome::Respond { status, body } => Ok(NetResponse {
                status: *status,
                headers: HashMap::new(),
                body: body.clone(),
            }),
            // As the SDK reports a request the host couldn't make.
            Outcome::Fail => Err(Error::Net("connection failed".to_owned())),
        }
    }
}
