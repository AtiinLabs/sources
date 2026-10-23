//! Komga's REST API: one operation per endpoint the plugin uses.
//!
//! A [`Client`] is built per operation from the current settings (see [`Client::connect`]),
//! so it never holds on to a stale server URL or credential.

use std::collections::HashMap;

use ito_rs::net::NetResponse;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::dto::{BookDto, LibraryDto, PageDto, Paged, SeriesDto};
use crate::error::{self, Result};
use crate::host::{Host, HttpRequest, Method};
use crate::settings::{Auth, Settings};

/// How many series or books a listing, a search page or a home section holds.
pub const PAGE_SIZE: i32 = 20;

/// A series' books are fetched in pages of this size until it is exhausted.
const BOOKS_PAGE_SIZE: i32 = 500;

/// `Accept` for covers and pages: the formats iOS decodes, then anything else Komga serves as is.
pub const IMAGE_ACCEPT: &str = "image/jpeg,image/png,image/webp,image/*";

const SORT_BY_TITLE: &str = "&sort=metadata.titleSort,asc";

/// Which series to list.
pub enum SeriesQuery<'a> {
    /// Every series the account can see, by title.
    All,
    /// One library's series, by title.
    Library(&'a str),
    /// Full-text search, best match first.
    Search(&'a str),
}

pub struct Client<'h, H: Host> {
    host: &'h H,
    base: String,
    auth: Auth,
}

impl<'h, H: Host> Client<'h, H> {
    /// A client for the server in the current settings, or the sentence that tells the user
    /// what to configure.
    pub fn connect(host: &'h H) -> Result<Self> {
        let Settings { server_url, auth } = Settings::load(host)?;
        Ok(Self {
            host,
            base: server_url,
            auth,
        })
    }

    /// The server URL, without a trailing slash.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Headers that make an image request succeed. Ito's image loader sends exactly these in
    /// place of its default `User-Agent` and `Referer`.
    pub fn image_headers(&self) -> HashMap<String, String> {
        let (name, value) = self.auth.header();
        HashMap::from([
            (name.to_owned(), value),
            ("Accept".to_owned(), IMAGE_ACCEPT.to_owned()),
        ])
    }

    pub fn libraries(&self) -> Result<Vec<LibraryDto>> {
        self.get_json("/api/v1/libraries")
    }

    /// One page of series. `page` is 1-based, as Ito counts; Komga counts from 0.
    pub fn series_page(&self, query: SeriesQuery<'_>, page: i32) -> Result<Paged<SeriesDto>> {
        let (sort, body) = match query {
            SeriesQuery::All => (SORT_BY_TITLE, json!({})),
            SeriesQuery::Library(id) => (
                SORT_BY_TITLE,
                json!({ "condition": { "libraryId": { "operator": "is", "value": id } } }),
            ),
            SeriesQuery::Search(text) => ("", json!({ "fullTextSearch": text })),
        };
        let page = (page - 1).max(0);
        self.post_json(
            &format!("/api/v1/series/list?page={page}&size={PAGE_SIZE}{sort}"),
            &body,
        )
    }

    pub fn series(&self, id: &str) -> Result<SeriesDto> {
        self.get_json(&format!("/api/v1/series/{id}"))
    }

    /// Every book of a series in reading order, whatever its state: callers decide which are
    /// readable.
    pub fn series_books(&self, series_id: &str) -> Result<Vec<BookDto>> {
        let body = json!({ "condition": { "seriesId": { "operator": "is", "value": series_id } } });
        let mut books = Vec::new();
        for page in 0.. {
            let batch: Paged<BookDto> = self.post_json(
                &format!(
                    "/api/v1/books/list?page={page}&size={BOOKS_PAGE_SIZE}&sort=metadata.numberSort,asc"
                ),
                &body,
            )?;
            let has_next = batch.has_next() && !batch.content.is_empty();
            books.extend(batch.content);
            if !has_next {
                break;
            }
        }
        Ok(books)
    }

    /// Books the user started and has not finished, most recently read first.
    pub fn in_progress_books(&self) -> Result<Paged<BookDto>> {
        let body =
            json!({ "condition": { "readStatus": { "operator": "is", "value": "IN_PROGRESS" } } });
        self.post_json(
            &format!("/api/v1/books/list?page=0&size={PAGE_SIZE}&sort=readProgress.readDate,desc"),
            &body,
        )
    }

    /// The next unread book of each series the user is partway through.
    pub fn on_deck_books(&self) -> Result<Paged<BookDto>> {
        self.get_json(&format!("/api/v1/books/ondeck?page=0&size={PAGE_SIZE}"))
    }

    pub fn new_series(&self) -> Result<Paged<SeriesDto>> {
        self.get_json(&format!("/api/v1/series/new?page=0&size={PAGE_SIZE}"))
    }

    pub fn updated_series(&self) -> Result<Paged<SeriesDto>> {
        self.get_json(&format!("/api/v1/series/updated?page=0&size={PAGE_SIZE}"))
    }

    /// The pages of a book, in order.
    pub fn book_pages(&self, book_id: &str) -> Result<Vec<PageDto>> {
        self.get_json(&format!("/api/v1/books/{book_id}/pages"))
    }

    /// Fetches an image by its path relative to the server, e.g.
    /// `api/v1/series/{id}/thumbnail`. See [`image_path`] for what is accepted.
    pub fn image(&self, path: &str) -> Result<Vec<u8>> {
        let path = image_path(path)?;
        let response = self.execute(Method::Get, format_args!("/{path}"), IMAGE_ACCEPT, None)?;
        Ok(response.body)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        parse(&self.execute(Method::Get, path, "application/json", None)?)
    }

    fn post_json<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let body = serde_json::to_vec(body).map_err(error::unencodable_request)?;
        parse(&self.execute(Method::Post, path, "application/json", Some(body))?)
    }

    fn execute(
        &self,
        method: Method,
        path: impl std::fmt::Display,
        accept: &str,
        body: Option<Vec<u8>>,
    ) -> Result<NetResponse> {
        let (auth_name, auth_value) = self.auth.header();
        let mut headers = vec![
            (auth_name.to_owned(), auth_value),
            ("Accept".to_owned(), accept.to_owned()),
        ];
        if body.is_some() {
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        }
        let request = HttpRequest {
            method,
            url: format!("{}{path}", self.base),
            headers,
            body,
        };
        let response = self.host.send(&request).map_err(|_| error::unreachable())?;
        error::check_status(response.status, &self.auth)?;
        Ok(response)
    }
}

/// Completes one book without sending a page number or changing any other user metadata.
/// Validate the opaque key as one URL-safe token before reading credentials or making a request.
pub fn complete_book(host: &impl Host, book_key: &str) -> Result<()> {
    if book_key.is_empty()
        || matches!(book_key, "." | "..")
        || !book_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
    {
        return Err(error::invalid_book_key());
    }
    let client = Client::connect(host)?;
    client.execute(
        Method::Patch,
        format_args!("/api/v1/books/{book_key}/read-progress"),
        "application/json",
        Some(br#"{"completed":true}"#.to_vec()),
    )?;
    Ok(())
}

fn parse<T: DeserializeOwned>(response: &NetResponse) -> Result<T> {
    serde_json::from_slice(&response.body).map_err(|_| error::unreadable_response())
}

/// Checks the part of an `ito://com.atiinlabs.komga/...` image URL that follows the plugin id
/// before it is sent to the server together with the user's credentials.
///
/// Only Komga's own API is reachable: the path must start with `api/v1/`, so an absolute URL
/// (which would carry the credentials to another host) is refused, and a `..` segment, a
/// backslash, whitespace or a control character is refused too.
pub fn image_path(path: &str) -> Result<&str> {
    let allowed = path.starts_with("api/v1/")
        && !path
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        && !path.split(['/', '?', '#']).any(|segment| segment == "..");
    if allowed {
        Ok(path)
    } else {
        Err(error::invalid_image_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::NOT_CONFIGURED;
    use crate::testing::{BASE, FakeHost, fixtures, message};
    use ito_rs::Error;
    use ito_rs::error::ErrorCategory;

    fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn body_json(request: &HttpRequest) -> Value {
        serde_json::from_slice(request.body.as_deref().expect("request has a body")).unwrap()
    }

    #[test]
    fn completing_a_book_accepts_an_empty_204_and_never_sends_a_lower_page() {
        let host = FakeHost::configured()
            .setting("server_url", "https://proxy.test/komga/")
            .respond(
                Method::Patch,
                "https://proxy.test/komga/api/v1/books/B_1-2.~/read-progress",
                204,
                [],
            );
        crate::service::mark_chapter_read(&host, "B_1-2.~").unwrap();
        let requests = host.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::Patch);
        assert_eq!(
            requests[0].url,
            "https://proxy.test/komga/api/v1/books/B_1-2.~/read-progress"
        );
        assert_eq!(body_json(&requests[0]), json!({ "completed": true }));
        assert_eq!(header(&requests[0], "Content-Type"), Some("application/json"));
    }

    #[test]
    fn completion_keys_cannot_widen_the_write_target_and_are_checked_before_credentials() {
        for key in [
            "",
            ".",
            "..",
            "../series/S",
            "B/../../series/S",
            "https://evil.test/B",
            "//evil.test/B",
            "B?key=private",
            "B#fragment",
            "B%2f..",
            "B;parameters",
            "B\\path",
            "B path",
            "B\r\nheader",
            "B\0",
        ] {
            let host = FakeHost::new();
            let error = crate::service::mark_chapter_read(&host, key).unwrap_err();
            assert!(matches!(error, Error::Host(_)));
            assert!(host.requests().is_empty());
        }
    }

    #[test]
    fn connecting_without_settings_asks_the_user_to_configure_the_plugin() {
        let host = FakeHost::new();
        assert_eq!(
            message(Client::connect(&host).err().unwrap()),
            NOT_CONFIGURED
        );
        assert!(
            host.requests().is_empty(),
            "nothing is sent without a server"
        );
    }

    #[test]
    fn every_request_carries_the_api_key_and_goes_to_the_configured_server() {
        let host =
            FakeHost::configured().json(Method::Get, "/api/v1/libraries", fixtures::LIBRARIES);
        let client = Client::connect(&host).unwrap();
        assert_eq!(client.base(), BASE);

        let libraries = client.libraries().unwrap();
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].id, "0RZXGDDWFJZ22");
        assert_eq!(libraries[0].name, "Comics");

        let requests = host.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::Get);
        assert_eq!(requests[0].url, "https://komga.test/api/v1/libraries");
        assert_eq!(header(&requests[0], "X-API-Key"), Some("test-api-key"));
        assert_eq!(header(&requests[0], "Authorization"), None);
        assert_eq!(header(&requests[0], "Accept"), Some("application/json"));
        assert!(requests[0].body.is_none());
    }

    #[test]
    fn username_and_password_are_sent_as_basic_auth_when_there_is_no_api_key() {
        let host = FakeHost::new()
            .setting("server_url", BASE)
            .setting("username", "user")
            .setting("password", "pass")
            .json(Method::Get, "/api/v1/libraries", fixtures::LIBRARIES);
        Client::connect(&host).unwrap().libraries().unwrap();

        let requests = host.requests();
        assert_eq!(
            header(&requests[0], "Authorization"),
            Some("Basic dXNlcjpwYXNz")
        );
        assert_eq!(header(&requests[0], "X-API-Key"), None);
    }

    #[test]
    fn the_api_key_wins_when_both_kinds_of_credentials_are_set() {
        let host = FakeHost::configured()
            .setting("username", "user")
            .setting("password", "pass")
            .json(Method::Get, "/api/v1/libraries", fixtures::LIBRARIES);
        Client::connect(&host).unwrap().libraries().unwrap();

        let requests = host.requests();
        assert_eq!(header(&requests[0], "X-API-Key"), Some("test-api-key"));
        assert_eq!(header(&requests[0], "Authorization"), None);
    }

    #[test]
    fn changed_settings_apply_to_the_next_client_without_a_restart() {
        let host =
            FakeHost::configured().json(Method::Get, "/api/v1/libraries", fixtures::LIBRARIES);
        Client::connect(&host).unwrap().libraries().unwrap();

        host.set_setting("server_url", "https://elsewhere.test/");
        host.set_setting("api_key", "another-key");
        host.add_response(
            Method::Get,
            "https://elsewhere.test/api/v1/libraries",
            200,
            fixtures::LIBRARIES,
        );
        Client::connect(&host).unwrap().libraries().unwrap();

        let requests = host.requests();
        assert_eq!(requests[1].url, "https://elsewhere.test/api/v1/libraries");
        assert_eq!(header(&requests[1], "X-API-Key"), Some("another-key"));
    }

    #[test]
    fn listing_all_series_is_sorted_by_title_and_has_no_condition() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/series/list?page=0&size=20&sort=metadata.titleSort,asc",
            fixtures::SERIES_LIST,
        );
        let page = Client::connect(&host)
            .unwrap()
            .series_page(SeriesQuery::All, 1)
            .unwrap();
        assert_eq!(page.content.len(), 5);
        assert!(!page.has_next());

        let requests = host.requests();
        assert_eq!(requests[0].method, Method::Post);
        assert_eq!(body_json(&requests[0]), json!({}));
        assert_eq!(
            header(&requests[0], "Content-Type"),
            Some("application/json")
        );
    }

    #[test]
    fn listing_a_library_filters_by_library_id() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/series/list?page=2&size=20&sort=metadata.titleSort,asc",
            fixtures::SERIES_LIST,
        );
        Client::connect(&host)
            .unwrap()
            .series_page(SeriesQuery::Library("LIB1"), 3)
            .unwrap();

        assert_eq!(
            body_json(&host.requests()[0]),
            json!({ "condition": { "libraryId": { "operator": "is", "value": "LIB1" } } })
        );
    }

    #[test]
    fn searching_posts_the_text_and_keeps_komgas_relevance_order() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/series/list?page=0&size=20",
            fixtures::SERIES_SEARCH_HARBOR,
        );
        let page = Client::connect(&host)
            .unwrap()
            .series_page(SeriesQuery::Search("night harbor"), 1)
            .unwrap();
        assert_eq!(page.content.len(), 1);

        let requests = host.requests();
        // No `sort` parameter: Komga then orders full-text results by relevance.
        assert_eq!(
            requests[0].url,
            "https://komga.test/api/v1/series/list?page=0&size=20"
        );
        assert_eq!(
            body_json(&requests[0]),
            json!({ "fullTextSearch": "night harbor" })
        );
    }

    #[test]
    fn ito_pages_are_one_based_and_komga_pages_are_zero_based() {
        for (ito_page, komga_page) in [(1, 0), (2, 1), (10, 9), (0, 0), (-3, 0)] {
            let host = FakeHost::configured().json(
                Method::Post,
                &format!("/api/v1/series/list?page={komga_page}&size=20"),
                fixtures::SERIES_SEARCH_HARBOR,
            );
            Client::connect(&host)
                .unwrap()
                .series_page(SeriesQuery::Search("x"), ito_page)
                .unwrap();
        }
    }

    #[test]
    fn a_page_that_is_not_last_reports_a_next_page() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/series/list?page=0&size=20&sort=metadata.titleSort,asc",
            fixtures::SERIES_LIST_FIRST_PAGE,
        );
        let page = Client::connect(&host)
            .unwrap()
            .series_page(SeriesQuery::All, 1)
            .unwrap();
        assert_eq!(page.content.len(), 2);
        assert!(page.has_next());
    }

    #[test]
    fn a_series_is_fetched_by_id() {
        let host = FakeHost::configured().json(
            Method::Get,
            "/api/v1/series/0RZXGDDYZJRQS",
            fixtures::SERIES_NIGHT_HARBOR,
        );
        let series = Client::connect(&host)
            .unwrap()
            .series("0RZXGDDYZJRQS")
            .unwrap();
        assert_eq!(series.id, "0RZXGDDYZJRQS");
        assert_eq!(series.metadata.title, "Night Harbor");
    }

    #[test]
    fn a_series_books_come_back_in_reading_order_from_one_request() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/books/list?page=0&size=500&sort=metadata.numberSort,asc",
            fixtures::BOOKS_NIGHT_HARBOR,
        );
        let books = Client::connect(&host)
            .unwrap()
            .series_books("0RZXGDDYZJRQS")
            .unwrap();
        assert_eq!(books.len(), 6);
        assert_eq!(books[0].name, "Night Harbor v01");
        assert_eq!(host.requests().len(), 1);
        assert_eq!(
            body_json(&host.requests()[0]),
            json!({ "condition": { "seriesId": { "operator": "is", "value": "0RZXGDDYZJRQS" } } })
        );
    }

    /// Splits a captured books page into two pages the way a long series would arrive.
    fn two_pages_of_books() -> (String, String) {
        let mut first: Value = serde_json::from_str(fixtures::BOOKS_NIGHT_HARBOR).unwrap();
        let mut second = first.clone();
        let all = first["content"].as_array().unwrap().clone();
        first["content"] = Value::Array(all[..4].to_vec());
        first["last"] = Value::Bool(false);
        second["content"] = Value::Array(all[4..].to_vec());
        second["last"] = Value::Bool(true);
        (first.to_string(), second.to_string())
    }

    #[test]
    fn a_long_series_is_fetched_page_by_page_until_the_last() {
        let (first, second) = two_pages_of_books();
        let host = FakeHost::configured()
            .respond(
                Method::Post,
                "https://komga.test/api/v1/books/list?page=0&size=500&sort=metadata.numberSort,asc",
                200,
                &first,
            )
            .respond(
                Method::Post,
                "https://komga.test/api/v1/books/list?page=1&size=500&sort=metadata.numberSort,asc",
                200,
                &second,
            );
        let books = Client::connect(&host).unwrap().series_books("S").unwrap();
        let names: Vec<&str> = books.iter().map(|book| book.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Night Harbor v01",
                "Night Harbor v02",
                "Night Harbor v03",
                "Night Harbor v04",
                "Night Harbor v05",
                "Night Harbor v06"
            ]
        );
        assert_eq!(host.requests().len(), 2);
    }

    #[test]
    fn paging_stops_when_a_server_claims_more_pages_but_sends_none() {
        let host = FakeHost::configured().respond(
            Method::Post,
            "https://komga.test/api/v1/books/list?page=0&size=500&sort=metadata.numberSort,asc",
            200,
            r#"{"content":[],"last":false}"#,
        );
        let books = Client::connect(&host).unwrap().series_books("S").unwrap();
        assert!(books.is_empty());
        assert_eq!(host.requests().len(), 1);
    }

    #[test]
    fn keep_reading_asks_for_in_progress_books_by_read_date() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/books/list?page=0&size=20&sort=readProgress.readDate,desc",
            fixtures::BOOKS_IN_PROGRESS,
        );
        let page = Client::connect(&host).unwrap().in_progress_books().unwrap();
        assert_eq!(page.content.len(), 3);
        assert_eq!(
            body_json(&host.requests()[0]),
            json!({ "condition": { "readStatus": { "operator": "is", "value": "IN_PROGRESS" } } })
        );
    }

    #[test]
    fn the_home_feeds_use_komgas_own_endpoints() {
        let host = FakeHost::configured()
            .json(
                Method::Get,
                "/api/v1/books/ondeck?page=0&size=20",
                fixtures::BOOKS_ONDECK,
            )
            .json(
                Method::Get,
                "/api/v1/series/new?page=0&size=20",
                fixtures::SERIES_NEW,
            )
            .json(
                Method::Get,
                "/api/v1/series/updated?page=0&size=20",
                fixtures::SERIES_UPDATED,
            );
        let client = Client::connect(&host).unwrap();
        assert_eq!(client.on_deck_books().unwrap().content.len(), 1);
        assert_eq!(client.new_series().unwrap().content.len(), 5);
        assert_eq!(client.updated_series().unwrap().content.len(), 3);
    }

    #[test]
    fn a_books_pages_are_listed_by_number() {
        let host = FakeHost::configured().json(
            Method::Get,
            "/api/v1/books/B1/pages",
            fixtures::PAGES_NIGHT_HARBOR_V01,
        );
        let pages = Client::connect(&host).unwrap().book_pages("B1").unwrap();
        let numbers: Vec<i32> = pages.iter().map(|page| page.number).collect();
        assert_eq!(numbers, (1..=12).collect::<Vec<_>>());
    }

    #[test]
    fn an_image_is_fetched_with_credentials_from_the_configured_server() {
        let host = FakeHost::configured().respond(
            Method::Get,
            "https://komga.test/api/v1/series/S1/thumbnail",
            200,
            vec![0xFF, 0xD8, 0xFF, 0xE0],
        );
        let bytes = Client::connect(&host)
            .unwrap()
            .image("api/v1/series/S1/thumbnail")
            .unwrap();
        assert_eq!(bytes, [0xFF, 0xD8, 0xFF, 0xE0]);

        let requests = host.requests();
        assert_eq!(requests[0].method, Method::Get);
        assert_eq!(header(&requests[0], "X-API-Key"), Some("test-api-key"));
        assert_eq!(header(&requests[0], "Accept"), Some(IMAGE_ACCEPT));
    }

    #[test]
    fn an_image_path_keeps_its_query_string() {
        let host = FakeHost::configured().respond(
            Method::Get,
            "https://komga.test/api/v1/books/B1/pages/3?convert=jpeg",
            200,
            vec![1, 2, 3],
        );
        let bytes = Client::connect(&host)
            .unwrap()
            .image("api/v1/books/B1/pages/3?convert=jpeg")
            .unwrap();
        assert_eq!(bytes, [1, 2, 3]);
    }

    #[test]
    fn only_komgas_api_paths_are_accepted_for_images() {
        for good in [
            "api/v1/series/S1/thumbnail",
            "api/v1/books/B1/pages/3?convert=jpeg",
            "api/v1/books/B1/thumbnail",
        ] {
            assert_eq!(image_path(good).unwrap(), good);
        }
    }

    #[test]
    fn absolute_urls_and_other_paths_are_never_fetched_with_credentials() {
        let host = FakeHost::configured();
        let client = Client::connect(&host).unwrap();
        for bad in [
            "https://evil.example/api/v1/series/S1/thumbnail",
            "http://evil.example/x.jpg",
            "//evil.example/api/v1/x",
            "/api/v1/series/S1/thumbnail",
            "api/v2/series/S1/thumbnail",
            "API/V1/series/S1/thumbnail",
            "api/v1",
            "api/v1x/series",
            "series/S1/thumbnail",
            "komga/api/v1/series/S1/thumbnail",
            "",
            "api/v1/../../etc/passwd",
            "api/v1/series/../../../admin",
            "api/v1/series/S1/thumbnail?next=../../x",
            "api/v1/series\\S1",
            "api/v1/series/S1 /thumbnail",
            "api/v1/series/S1\n/thumbnail",
            "api/v1/series/S1\u{0}/thumbnail",
        ] {
            let err = client.image(bad).unwrap_err();
            assert_eq!(
                message(err),
                "Komga images must be requested with a path that starts with api/v1/.",
                "{bad:?}"
            );
        }
        assert!(host.requests().is_empty(), "a refused path sends nothing");
    }

    #[test]
    fn an_image_error_status_is_an_error() {
        let host = FakeHost::configured().respond(
            Method::Get,
            "https://komga.test/api/v1/series/S1/thumbnail",
            404,
            "not here",
        );
        let err = Client::connect(&host)
            .unwrap()
            .image("api/v1/series/S1/thumbnail")
            .unwrap_err();
        assert!(message(err).contains("HTTP 404"));
    }

    #[test]
    fn a_rejected_api_key_is_reported_with_its_status() {
        let host = FakeHost::configured().respond(
            Method::Get,
            "https://komga.test/api/v1/libraries",
            401,
            r#"{"status":401,"error":"Unauthorized"}"#,
        );
        let error: Error = Client::connect(&host).unwrap().libraries().unwrap_err().into();
        assert_eq!(error.category(), ErrorCategory::Unauthorized);
        assert_eq!(message(error), "Komga rejected the API key (HTTP 401).");
    }

    #[test]
    fn a_rejected_login_is_reported_as_such() {
        let host = FakeHost::new()
            .setting("server_url", BASE)
            .setting("username", "user")
            .setting("password", "wrong")
            .respond(Method::Get, "https://komga.test/api/v1/libraries", 401, "");
        let err = Client::connect(&host).unwrap().libraries().unwrap_err();
        assert_eq!(
            message(err),
            "Komga rejected the username or password (HTTP 401)."
        );
    }

    #[test]
    fn a_failed_connection_is_reported_without_internals() {
        let host = FakeHost::configured().fail(Method::Get, "https://komga.test/api/v1/libraries");
        let error: Error = Client::connect(&host).unwrap().libraries().unwrap_err().into();
        assert_eq!(error.category(), ErrorCategory::Network);
        assert_eq!(
            message(error),
            "Couldn't connect to Komga. Check the server URL and that the server is running."
        );
    }

    #[test]
    fn a_server_error_never_echoes_the_response_body() {
        let host = FakeHost::configured().respond(
            Method::Get,
            "https://komga.test/api/v1/libraries",
            500,
            "FFI Error in net_fetch_v2: secret-token-in-url",
        );
        let error: Error = Client::connect(&host).unwrap().libraries().unwrap_err().into();
        assert_eq!(error.category(), ErrorCategory::Server);
        let text = message(error);
        assert!(text.contains("HTTP 500"));
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn a_page_that_is_not_komga_json_is_reported_as_unreadable() {
        let host = FakeHost::configured().respond(
            Method::Get,
            "https://komga.test/api/v1/libraries",
            200,
            "<html><body>Sign in</body></html>",
        );
        let err = Client::connect(&host).unwrap().libraries().unwrap_err();
        assert_eq!(
            message(err),
            "Komga sent a response Ito couldn't read. Check that the server URL points to Komga."
        );
    }
}
