//! What the plugin does, one function per `MangaProvider` method, written against [`Host`].
//!
//! [`crate::Komga`] calls these with the real host; the tests call them with a fake one.

use ito_rs::Result;
use ito_rs::models::manga::{Chapter, Manga, PageResult};
use ito_rs::models::{
    ContentKind, HomeComponent, HomeComponentValue, HomeLayout, Listing, Page, PluginImageRequest,
};

use crate::api::{self, Client, SeriesQuery};
use crate::dto::{BookDto, Paged, SeriesDto};
use crate::error::{self, Failure};
use crate::host::Host;
use crate::mapping;
use crate::settings::Settings;

/// The id of the listing that spans every library.
pub const ALL_LISTING: &str = "all";

/// The home screen: what the user is reading, what is new, and the way into the libraries.
///
/// Sections are scrollers of series. The feeds come first: "Keep reading" (books in
/// progress, one card per series), "On deck" (the next book of each series being read),
/// "Recently added" and "Recently updated". Then "All series" and, when the server has
/// several libraries, one section per library; those headers link to the full listing, which
/// is how Ito offers a listing. A section with nothing in it is left out.
///
/// Ito shows an empty home when a plugin fails, with no message. So a problem the user can
/// fix (no server URL, no credentials, a rejected sign-in, an unreachable server) is not an
/// error here: the home is a single empty section whose title and subtitle say what to do.
/// Errors from the other methods stay errors.
pub fn home(host: &impl Host) -> Result<HomeLayout> {
    let client = match Client::connect(host) {
        Ok(client) => client,
        Err(Failure::NotConfigured) => return Ok(notice(NOT_CONFIGURED_NOTICE)),
        Err(Failure::MissingCredentials) => return Ok(notice(MISSING_CREDENTIALS_NOTICE)),
        Err(other) => return Err(other.into()),
    };
    match feeds(&client) {
        Ok(layout) => Ok(layout),
        Err(Failure::Rejected(_)) => Ok(notice(REJECTED_NOTICE)),
        Err(_) => Ok(unreachable_notice(client.base())),
    }
}

fn feeds(client: &Client<'_, impl Host>) -> error::Result<HomeLayout> {
    let base = client.base();
    let mut components = Vec::new();

    let keep_reading = book_cards(client.in_progress_books()?, base);
    components.extend(section("Keep reading", keep_reading, None));
    let on_deck = book_cards(client.on_deck_books()?, base);
    components.extend(section("On deck", on_deck, None));
    let added = series_cards(client.new_series()?, base);
    components.extend(section("Recently added", added, None));
    let updated = series_cards(client.updated_series()?, base);
    components.extend(section("Recently updated", updated, None));

    let all = series_cards(client.series_page(SeriesQuery::All, 1)?, base);
    let all_listing = Listing {
        id: ALL_LISTING.to_owned(),
        name: "All series".to_owned(),
        kind: 0,
    };
    components.extend(section("All series", all, Some(all_listing)));

    // With a single library "All series" already is that library.
    let libraries = client.libraries()?;
    if libraries.len() > 1 {
        for library in libraries {
            let cards = series_cards(
                client.series_page(SeriesQuery::Library(&library.id), 1)?,
                base,
            );
            let listing = Listing {
                id: library.id,
                name: library.name.clone(),
                kind: 0,
            };
            components.extend(section(&library.name, cards, Some(listing)));
        }
    }

    Ok(HomeLayout { components })
}

/// What the home screen says when it has nothing to show: a title and what to do about it.
struct Notice {
    title: &'static str,
    subtitle: &'static str,
}

const NOT_CONFIGURED_NOTICE: Notice = Notice {
    title: "Connect your Komga server",
    subtitle: "Tap the gear icon above and add your server URL and an API key (or username and password).",
};
const MISSING_CREDENTIALS_NOTICE: Notice = Notice {
    title: "Add your Komga sign-in",
    subtitle: "Tap the gear icon above and add an API key, or your username and password.",
};
const REJECTED_NOTICE: Notice = Notice {
    title: "Komga rejected the sign-in",
    subtitle: "Check the API key or password in the plugin settings.",
};
const UNREACHABLE_TITLE: &str = "Can't reach your Komga server";

/// A home that is one empty section carrying the message.
fn notice(notice: Notice) -> HomeLayout {
    message_home(notice.title, notice.subtitle.to_owned())
}

/// The server did not answer, or answered with something that isn't Komga. The subtitle names
/// the host only: no scheme, credentials, path or query.
fn unreachable_notice(server_url: &str) -> HomeLayout {
    let subtitle = match host_name(server_url) {
        Some(name) => format!("Check that {name} is running and reachable from this device."),
        None => {
            "Check that your Komga server is running and reachable from this device.".to_owned()
        }
    };
    message_home(UNREACHABLE_TITLE, subtitle)
}

fn message_home(title: &str, subtitle: String) -> HomeLayout {
    HomeLayout {
        components: vec![HomeComponent {
            title: Some(title.to_owned()),
            subtitle: Some(subtitle),
            value: HomeComponentValue::Scroller(Vec::new(), None),
        }],
    }
}

/// The `host[:port]` of a server URL, dropping the scheme, any `user:password@`, the path,
/// the query and the fragment. `None` if nothing is left.
fn host_name(server_url: &str) -> Option<&str> {
    let rest = server_url
        .split_once("://")
        .map_or(server_url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let name = authority.rsplit('@').next().unwrap_or_default().trim();
    (!name.is_empty()).then_some(name)
}

/// One page of a listing: every library, or [`ALL_LISTING`].
pub fn manga_list(host: &impl Host, listing: &Listing, page: i32) -> Result<PageResult> {
    let client = Client::connect(host)?;
    let query = if listing.id == ALL_LISTING {
        SeriesQuery::All
    } else {
        SeriesQuery::Library(&listing.id)
    };
    let series = client.series_page(query, page)?;
    Ok(page_result(series, client.base()))
}

/// One page of search results. An empty query lists everything.
pub fn search(host: &impl Host, query: &str, page: i32) -> Result<PageResult> {
    let client = Client::connect(host)?;
    let query = query.trim();
    let query = if query.is_empty() {
        SeriesQuery::All
    } else {
        SeriesQuery::Search(query)
    };
    let series = client.series_page(query, page)?;
    Ok(page_result(series, client.base()))
}

/// Refreshes a title: its details, its chapters, both or neither, as Ito asks. What isn't
/// refreshed stays as Ito sent it.
pub fn manga_update(
    host: &impl Host,
    mut manga: Manga,
    needs_details: bool,
    needs_chapters: bool,
) -> Result<Manga> {
    if !needs_details && !needs_chapters {
        return Ok(manga);
    }
    let client = Client::connect(host)?;
    if needs_details {
        let series = client.series(&manga.key)?;
        let fresh = mapping::series_to_manga(&series, client.base());
        manga = Manga {
            chapters: manga.chapters,
            ..fresh
        };
    }
    if needs_chapters {
        let books = client.series_books(&manga.key)?;
        manga.chapters = Some(mapping::chapters(&books));
    }
    Ok(manga)
}

/// Chapter content uses plugin-owned locators; credentials are resolved only when loaded.
pub fn page_list(host: &impl Host, chapter: &Chapter) -> Result<Vec<Page>> {
    api::validate_book_key(&chapter.key)?;
    let client = Client::connect(host)?;
    if chapter.kind == ContentKind::Text {
        let manifest = client.epub_manifest(&chapter.key)?;
        if manifest.reading_order.is_empty() {
            return Err(ito_rs::Error::Host(
                "Komga sent an EPUB with no reading order.".into(),
            ));
        }
        // Validate the whole spine before requesting any resource. Manifest URLs are never followed.
        let paths = manifest
            .reading_order
            .iter()
            .map(|link| crate::epub::spine_path(&link.href, &chapter.key))
            .collect::<Result<Vec<_>>>()?;
        let mut content = Vec::new();
        for path in paths {
            let xhtml = client.epub_resource(&chapter.key, &path)?;
            let pages = host.parse_epub_document(&xhtml, &chapter.key, &path)?;
            for mut page in pages {
                page.index = i32::try_from(content.len())
                    .map_err(|_| ito_rs::Error::Host("Komga EPUB has too much content.".into()))?;
                content.push(page);
            }
        }
        if content.is_empty() {
            return Err(ito_rs::Error::Host(
                "Komga EPUB contains no readable content.".into(),
            ));
        }
        return Ok(content);
    }
    let pages = client.book_pages(&chapter.key)?;
    Ok(pages
        .iter()
        .enumerate()
        .map(|(index, page)| {
            mapping::page(index as i32, mapping::page_url(&chapter.key, page.number))
        })
        .collect())
}

/// Sends only a completion flag; the host owns durable delivery and target-scope authorization.
pub fn mark_chapter_read(host: &impl Host, chapter_key: &str) -> Result<()> {
    Ok(api::complete_book(host, chapter_key)?)
}

/// The id of the settings screen's connection check.
pub const TEST_CONNECTION: &str = "test_connection";

/// Runs a settings action by id. An id the plugin never declared is not handled, so the host
/// cannot mistake it for a successful check, and nothing is requested.
pub fn run_settings_action(host: &impl Host, action_id: &str) -> Result<String> {
    match action_id {
        TEST_CONNECTION => test_connection(host),
        _ => Err(ito_rs::Error::Unsupported),
    }
}

/// The settings screen's "Test connection": one read-only request with the settings the host
/// supplies, which may include an unsaved key. Failures keep their categories and sentences.
pub fn test_connection(host: &impl Host) -> Result<String> {
    let libraries = Client::connect(host)?.libraries()?;
    Ok(match libraries.len() {
        0 => "Connected to Komga. No libraries are shared with this account.".to_owned(),
        1 => "Connected to Komga. 1 library is available.".to_owned(),
        count => format!("Connected to Komga. {count} libraries are available."),
    })
}

/// The configured server is the sole image authority; never derive it from an image path.
///
/// This is a base URL, possibly including a reverse-proxy path prefix. Ito normalizes it to
/// its scheme/host/effective-port origin for the native destination and redirect checks.
/// Loading settings also preserves the typed setup error when configuration is missing.
pub fn image_origins(host: &impl Host) -> Result<Vec<String>> {
    Ok(vec![Settings::load(host)?.server_url])
}

/// Resolve a server-relative image path without sending a request. Validate before building
/// the destination or attaching credentials, using the same policy as the legacy byte path.
pub fn image_request(host: &impl Host, path: &str) -> Result<PluginImageRequest> {
    let path = api::image_path(path)?;
    let client = Client::connect(host)?;
    Ok(PluginImageRequest {
        url: format!("{}/{path}", client.base()),
        headers: Some(client.image_headers()),
    })
}

/// An image Ito asked the plugin for, by the path that followed the plugin id in its
/// `ito://` URL.
pub fn image(host: &impl Host, path: &str) -> Result<Vec<u8>> {
    Ok(Client::connect(host)?.image(path)?)
}

fn section(title: &str, entries: Vec<Manga>, listing: Option<Listing>) -> Option<HomeComponent> {
    if entries.is_empty() {
        return None;
    }
    Some(HomeComponent {
        title: Some(title.to_owned()),
        subtitle: None,
        value: HomeComponentValue::Scroller(entries, listing),
    })
}

/// Cards for the series that are still in the library; Komga also lists trashed ones.
fn series_cards(page: Paged<SeriesDto>, base: &str) -> Vec<Manga> {
    page.content
        .iter()
        .filter(|series| !series.deleted)
        .map(|series| mapping::series_to_manga(series, base))
        .collect()
}

/// One card per series for a page of books.
fn book_cards(page: Paged<BookDto>, base: &str) -> Vec<Manga> {
    let cards = page
        .content
        .iter()
        .filter(|book| mapping::is_readable(book))
        .filter_map(|book| mapping::series_stub(book, base))
        .collect();
    mapping::dedupe(cards)
}

fn page_result(page: Paged<SeriesDto>, base: &str) -> PageResult {
    let has_next_page = page.has_next();
    PageResult {
        entries: series_cards(page, base),
        has_next_page,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::NOT_CONFIGURED;
    use crate::host::Method;
    use crate::testing::{BASE, FakeHost, fixtures, message};
    use ito_rs::models::manga::Status;
    use serde_json::{Value, json};

    const EMPTY_PAGE: &str = r#"{"content":[],"last":true}"#;

    const IN_PROGRESS: &str = "/api/v1/books/list?page=0&size=20&sort=readProgress.readDate,desc";
    const ON_DECK: &str = "/api/v1/books/ondeck?page=0&size=20";
    const NEW: &str = "/api/v1/series/new?page=0&size=20";
    const UPDATED: &str = "/api/v1/series/updated?page=0&size=20";
    const ALL_SERIES: &str = "/api/v1/series/list?page=0&size=20&sort=metadata.titleSort,asc";
    const LIBRARIES: &str = "/api/v1/libraries";

    fn home_host() -> FakeHost {
        FakeHost::configured()
            .json(Method::Post, IN_PROGRESS, fixtures::BOOKS_IN_PROGRESS)
            .json(Method::Get, ON_DECK, fixtures::BOOKS_ONDECK)
            .json(Method::Get, NEW, fixtures::SERIES_NEW)
            .json(Method::Get, UPDATED, fixtures::SERIES_UPDATED)
            .json(Method::Post, ALL_SERIES, fixtures::SERIES_LIST)
            .json(Method::Get, LIBRARIES, fixtures::LIBRARIES)
    }

    /// A section's title, its titles and its listing id.
    fn describe(component: &HomeComponent) -> (String, Vec<String>, Option<String>) {
        let HomeComponentValue::Scroller(entries, listing) = &component.value else {
            panic!("expected a scroller, got {:?}", component.value);
        };
        (
            component.title.clone().unwrap_or_default(),
            entries.iter().map(|manga| manga.title.clone()).collect(),
            listing.as_ref().map(|listing| listing.id.clone()),
        )
    }

    fn titles(entries: &[Manga]) -> Vec<&str> {
        entries.iter().map(|manga| manga.title.as_str()).collect()
    }

    fn urls(host: &FakeHost) -> Vec<String> {
        host.requests()
            .iter()
            .map(|request| {
                format!(
                    "{:?} {}",
                    request.method,
                    request.url.trim_start_matches(BASE)
                )
            })
            .collect()
    }

    fn body(host: &FakeHost, index: usize) -> Value {
        serde_json::from_slice(host.requests()[index].body.as_deref().unwrap()).unwrap()
    }

    #[test]
    fn home_lists_the_feeds_then_all_series() {
        let host = home_host();
        let layout = home(&host).unwrap();
        let sections: Vec<_> = layout.components.iter().map(describe).collect();

        assert_eq!(
            sections,
            [
                (
                    "Keep reading".to_owned(),
                    vec!["Glass Orchard".to_owned(), "Night Harbor".to_owned()],
                    None
                ),
                (
                    "On deck".to_owned(),
                    vec!["Paper Lanterns".to_owned()],
                    None
                ),
                (
                    "Recently added".to_owned(),
                    vec![
                        "Vertical Rain".to_owned(),
                        "Night Harbor".to_owned(),
                        "Paper Lanterns".to_owned(),
                        "Glass Orchard".to_owned()
                    ],
                    None
                ),
                (
                    "Recently updated".to_owned(),
                    vec!["Paper Lanterns".to_owned(), "Glass Orchard".to_owned()],
                    None
                ),
                (
                    "All series".to_owned(),
                    vec![
                        "Glass Orchard".to_owned(),
                        "Night Harbor".to_owned(),
                        "Paper Lanterns".to_owned(),
                        "Vertical Rain".to_owned()
                    ],
                    Some("all".to_owned())
                ),
            ]
        );
    }

    #[test]
    fn home_asks_komga_for_each_feed_in_turn() {
        let host = home_host();
        home(&host).unwrap();
        assert_eq!(
            urls(&host),
            [
                format!("Post {IN_PROGRESS}"),
                format!("Get {ON_DECK}"),
                format!("Get {NEW}"),
                format!("Get {UPDATED}"),
                format!("Post {ALL_SERIES}"),
                format!("Get {LIBRARIES}"),
            ]
        );
        assert_eq!(
            body(&host, 0),
            json!({ "condition": { "readStatus": { "operator": "is", "value": "IN_PROGRESS" } } })
        );
    }

    #[test]
    fn keep_reading_has_one_card_per_series_even_with_several_books_in_progress() {
        // Night Harbor has two books in progress.
        let layout = home(&home_host()).unwrap();
        let (_, entries, _) = describe(&layout.components[0]);
        assert_eq!(entries, ["Glass Orchard", "Night Harbor"]);
    }

    #[test]
    fn home_cards_point_at_series_with_server_relative_covers() {
        let layout = home(&home_host()).unwrap();
        let HomeComponentValue::Scroller(entries, _) = &layout.components[1].value else {
            panic!("expected a scroller");
        };
        let deck = &entries[0];
        assert_eq!(deck.key, "0RZXGDDYZJRQM");
        assert_eq!(
            deck.cover.as_deref(),
            Some("ito://com.atiinlabs.komga/api/v1/series/0RZXGDDYZJRQM/thumbnail")
        );

        // Cards for whole series carry their details.
        let HomeComponentValue::Scroller(added, _) = &layout.components[2].value else {
            panic!("expected a scroller");
        };
        let harbor = added
            .iter()
            .find(|manga| manga.title == "Night Harbor")
            .unwrap();
        assert!(matches!(harbor.status, Status::Ongoing));
        assert_eq!(harbor.authors.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn trashed_series_are_not_shown() {
        // "Ghost Town" is in Recently added, Recently updated and the full list, all flagged deleted.
        let layout = home(&home_host()).unwrap();
        for component in &layout.components {
            let (title, entries, _) = describe(component);
            assert!(!entries.contains(&"Ghost Town".to_owned()), "{title}");
        }
    }

    #[test]
    fn a_trashed_book_does_not_put_its_series_in_keep_reading() {
        let page: Paged<BookDto> = serde_json::from_value(json!({
            "content": [
                { "id": "B1", "seriesId": "S1", "seriesTitle": "Gone", "deleted": true, "media": { "status": "READY", "mediaProfile": "DIVINA" } },
                { "id": "B2", "seriesId": "S2", "seriesTitle": "Here", "media": { "status": "READY", "mediaProfile": "DIVINA" } },
                { "id": "B3", "seriesId": "S3", "seriesTitle": "Unsupported", "media": { "status": "UNSUPPORTED", "mediaProfile": "" } }
            ],
            "last": true
        }))
        .unwrap();
        assert_eq!(titles(&book_cards(page, BASE)), ["Here"]);
    }

    #[test]
    fn sections_with_nothing_in_them_are_left_out() {
        let host = FakeHost::configured()
            .respond(
                Method::Post,
                &format!("{BASE}{IN_PROGRESS}"),
                200,
                EMPTY_PAGE,
            )
            .respond(Method::Get, &format!("{BASE}{ON_DECK}"), 200, EMPTY_PAGE)
            .json(Method::Get, NEW, fixtures::SERIES_NEW)
            .respond(Method::Get, &format!("{BASE}{UPDATED}"), 200, EMPTY_PAGE)
            .json(Method::Post, ALL_SERIES, fixtures::SERIES_LIST)
            .json(Method::Get, LIBRARIES, fixtures::LIBRARIES);
        let layout = home(&host).unwrap();
        let sections: Vec<String> = layout.components.iter().map(|c| describe(c).0).collect();
        assert_eq!(sections, ["Recently added", "All series"]);
    }

    #[test]
    fn an_empty_server_has_an_empty_home() {
        let host = FakeHost::configured()
            .respond(
                Method::Post,
                &format!("{BASE}{IN_PROGRESS}"),
                200,
                EMPTY_PAGE,
            )
            .respond(Method::Get, &format!("{BASE}{ON_DECK}"), 200, EMPTY_PAGE)
            .respond(Method::Get, &format!("{BASE}{NEW}"), 200, EMPTY_PAGE)
            .respond(Method::Get, &format!("{BASE}{UPDATED}"), 200, EMPTY_PAGE)
            .respond(
                Method::Post,
                &format!("{BASE}{ALL_SERIES}"),
                200,
                EMPTY_PAGE,
            )
            .respond(Method::Get, &format!("{BASE}{LIBRARIES}"), 200, "[]");
        assert!(home(&host).unwrap().components.is_empty());
    }

    #[test]
    fn several_libraries_each_get_a_section_that_links_to_their_listing() {
        let host = home_host()
            .respond(
                Method::Get,
                &format!("{BASE}{LIBRARIES}"),
                200,
                r#"[{"id":"LIB1","name":"Manga"},{"id":"LIB2","name":"Comics"}]"#,
            )
            .respond(
                Method::Post,
                &format!("{BASE}/api/v1/series/list?page=0&size=20&sort=metadata.titleSort,asc"),
                200,
                fixtures::SERIES_LIST,
            );
        let layout = home(&host).unwrap();
        let sections: Vec<_> = layout.components.iter().map(describe).collect();
        let tail: Vec<(&str, Option<&str>)> = sections[4..]
            .iter()
            .map(|(title, _, listing)| (title.as_str(), listing.as_deref()))
            .collect();
        assert_eq!(
            tail,
            [
                ("All series", Some("all")),
                ("Manga", Some("LIB1")),
                ("Comics", Some("LIB2"))
            ]
        );

        // The library sections come from the library-filtered series list.
        let requests = urls(&host);
        assert_eq!(requests.len(), 8, "{requests:?}");
        assert_eq!(
            body(&host, 6),
            json!({ "condition": { "libraryId": { "operator": "is", "value": "LIB1" } } })
        );
        assert_eq!(
            body(&host, 7),
            json!({ "condition": { "libraryId": { "operator": "is", "value": "LIB2" } } })
        );
    }

    /// The title and subtitle of a home that is nothing but one empty section with a message.
    fn the_notice(layout: &HomeLayout) -> (String, String) {
        let [component] = layout.components.as_slice() else {
            panic!(
                "expected exactly one component, got {}",
                layout.components.len()
            );
        };
        let HomeComponentValue::Scroller(entries, listing) = &component.value else {
            panic!("expected a scroller, got {:?}", component.value);
        };
        assert!(entries.is_empty());
        assert!(listing.is_none());
        (
            component.title.clone().expect("a title"),
            component.subtitle.clone().expect("a subtitle"),
        )
    }

    #[test]
    fn home_without_a_server_url_asks_for_one() {
        let host = FakeHost::new();
        let layout = home(&host).unwrap();
        assert_eq!(
            the_notice(&layout),
            (
                "Connect your Komga server".to_owned(),
                "Tap the gear icon above and add your server URL and an API key (or username and password)."
                    .to_owned()
            )
        );
        assert!(host.requests().is_empty());
    }

    #[test]
    fn home_with_a_server_but_no_credentials_asks_for_them() {
        let expected = (
            "Add your Komga sign-in".to_owned(),
            "Tap the gear icon above and add an API key, or your username and password.".to_owned(),
        );
        let host = FakeHost::new().setting("server_url", BASE);
        assert_eq!(the_notice(&home(&host).unwrap()), expected);
        assert!(host.requests().is_empty());

        // A username without a password is not a login.
        let host = FakeHost::new()
            .setting("server_url", BASE)
            .setting("username", "user");
        assert_eq!(the_notice(&home(&host).unwrap()), expected);
    }

    #[test]
    fn home_says_when_komga_rejects_the_sign_in() {
        let expected = (
            "Komga rejected the sign-in".to_owned(),
            "Check the API key or password in the plugin settings.".to_owned(),
        );
        for status in [401, 403] {
            let host = FakeHost::configured().respond(
                Method::Post,
                &format!("{BASE}{IN_PROGRESS}"),
                status,
                "{}",
            );
            assert_eq!(the_notice(&home(&host).unwrap()), expected, "{status}");
        }

        // A rejection halfway through the feeds still replaces the whole home.
        let host = home_host().respond(Method::Get, &format!("{BASE}{UPDATED}"), 401, "{}");
        assert_eq!(the_notice(&home(&host).unwrap()), expected);
    }

    #[test]
    fn home_says_when_the_server_cannot_be_reached() {
        let title = "Can't reach your Komga server";
        let subtitle = "Check that komga.test is running and reachable from this device.";
        let in_progress = format!("{BASE}{IN_PROGRESS}");

        // The connection fails.
        let host = FakeHost::configured().fail(Method::Post, &in_progress);
        assert_eq!(
            the_notice(&home(&host).unwrap()),
            (title.to_owned(), subtitle.to_owned())
        );

        // The runner reports a failed connection as HTTP 500; a server error reads the same.
        for status in [500, 502] {
            let host = FakeHost::configured().respond(Method::Post, &in_progress, status, "");
            assert_eq!(
                the_notice(&home(&host).unwrap()),
                (title.to_owned(), subtitle.to_owned()),
                "{status}"
            );
        }

        // Something answers, but it isn't Komga.
        let host = FakeHost::configured().respond(Method::Post, &in_progress, 200, "<html>");
        assert_eq!(
            the_notice(&home(&host).unwrap()),
            (title.to_owned(), subtitle.to_owned())
        );
    }

    #[test]
    fn the_unreachable_notice_names_the_host_only() {
        // The trailing slash is dropped from the server URL, so requests go to .../komga/api/...
        let host = FakeHost::new()
            .setting(
                "server_url",
                "https://me:hunter2@komga.example.com:25600/komga/",
            )
            .setting("api_key", "secret-key")
            .fail(
                Method::Post,
                "https://me:hunter2@komga.example.com:25600/komga/api/v1/books/list?page=0&size=20&sort=readProgress.readDate,desc",
            );
        let (title, subtitle) = the_notice(&home(&host).unwrap());
        assert_eq!(title, "Can't reach your Komga server");
        assert_eq!(
            subtitle,
            "Check that komga.example.com:25600 is running and reachable from this device."
        );
    }

    #[test]
    fn host_name_drops_everything_but_the_host_and_port() {
        assert_eq!(
            host_name("https://komga.example.com"),
            Some("komga.example.com")
        );
        assert_eq!(
            host_name("http://192.168.1.5:25600/komga"),
            Some("192.168.1.5:25600")
        );
        assert_eq!(
            host_name("https://me:hunter2@komga.example.com/komga?token=abc#top"),
            Some("komga.example.com")
        );
        assert_eq!(host_name("komga.local"), Some("komga.local"));
        assert_eq!(host_name("https:///path"), None);
    }

    #[test]
    fn listing_all_series_maps_a_page_of_series() {
        let host = FakeHost::configured().json(Method::Post, ALL_SERIES, fixtures::SERIES_LIST);
        let listing = Listing {
            id: "all".into(),
            name: "All series".into(),
            kind: 0,
        };
        let result = manga_list(&host, &listing, 1).unwrap();
        // The captured page holds a trashed series that is not shown.
        assert_eq!(
            titles(&result.entries),
            [
                "Glass Orchard",
                "Night Harbor",
                "Paper Lanterns",
                "Vertical Rain"
            ]
        );
        assert!(!result.has_next_page);
        assert_eq!(body(&host, 0), json!({}));
    }

    #[test]
    fn listing_a_library_filters_to_it_and_pages_from_one() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/series/list?page=1&size=20&sort=metadata.titleSort,asc",
            fixtures::SERIES_LIST_FIRST_PAGE,
        );
        let listing = Listing {
            id: "0RZXGDDWFJZ22".into(),
            name: "Comics".into(),
            kind: 0,
        };
        let result = manga_list(&host, &listing, 2).unwrap();
        // The second request of the app (page 2) is Komga's page 1, and more pages follow.
        assert!(result.has_next_page);
        assert_eq!(
            body(&host, 0),
            json!({ "condition": { "libraryId": { "operator": "is", "value": "0RZXGDDWFJZ22" } } })
        );
    }

    #[test]
    fn searching_finds_series_by_text() {
        let host = FakeHost::configured().json(
            Method::Post,
            "/api/v1/series/list?page=0&size=20",
            fixtures::SERIES_SEARCH_HARBOR,
        );
        let result = search(&host, "  harbor ", 1).unwrap();
        assert_eq!(titles(&result.entries), ["Night Harbor"]);
        assert!(!result.has_next_page);
        assert_eq!(body(&host, 0), json!({ "fullTextSearch": "harbor" }));
    }

    #[test]
    fn an_empty_search_lists_everything() {
        let host = FakeHost::configured().json(Method::Post, ALL_SERIES, fixtures::SERIES_LIST);
        let result = search(&host, "   ", 1).unwrap();
        assert_eq!(result.entries.len(), 4);
        assert_eq!(body(&host, 0), json!({}));
    }

    #[test]
    fn search_and_listings_need_the_plugin_to_be_configured() {
        let host = FakeHost::new();
        assert_eq!(message(search(&host, "x", 1).unwrap_err()), NOT_CONFIGURED);
        let listing = Listing {
            id: "all".into(),
            name: "All".into(),
            kind: 0,
        };
        assert_eq!(
            message(manga_list(&host, &listing, 1).unwrap_err()),
            NOT_CONFIGURED
        );
    }

    fn saved_manga() -> Manga {
        // What Ito holds for a title it saved earlier: a card without details or chapters.
        mapping::series_stub(
            &serde_json::from_value(json!({
                "id": "B", "seriesId": "0RZXGDDYZJRQS", "seriesTitle": "Old Title"
            }))
            .unwrap(),
            BASE,
        )
        .unwrap()
    }

    fn update_host() -> FakeHost {
        FakeHost::configured()
            .json(
                Method::Get,
                "/api/v1/series/0RZXGDDYZJRQS",
                fixtures::SERIES_NIGHT_HARBOR,
            )
            .json(
                Method::Post,
                "/api/v1/books/list?page=0&size=500&sort=metadata.numberSort,asc",
                fixtures::BOOKS_NIGHT_HARBOR,
            )
    }

    #[test]
    fn an_update_for_details_and_chapters_fetches_both() {
        let host = update_host();
        let manga = manga_update(&host, saved_manga(), true, true).unwrap();

        assert_eq!(manga.title, "Night Harbor");
        assert!(matches!(manga.viewer, ito_rs::models::manga::Viewer::Rtl));
        let chapters = manga.chapters.expect("chapters were requested");
        assert_eq!(chapters.len(), 6);
        assert_eq!(chapters[0].chapter, Some(6.0));
        assert_eq!(urls(&host).len(), 2);
    }

    #[test]
    fn an_update_for_details_alone_leaves_the_chapters_as_they_were() {
        let host = update_host();
        let mut saved = saved_manga();
        saved.chapters = Some(mapping::chapters(&[]));
        saved.chapters.as_mut().unwrap().push(Chapter {
            key: "kept".into(),
            title: None,
            volume: None,
            chapter: Some(1.0),
            date_updated: None,
            scanlator: None,
            url: None,
            lang: None,
            paywalled: None,
            kind: ContentKind::Images,
        });

        let manga = manga_update(&host, saved, true, false).unwrap();
        assert_eq!(manga.title, "Night Harbor");
        assert_eq!(manga.chapters.unwrap()[0].key, "kept");
        assert_eq!(urls(&host), ["Get /api/v1/series/0RZXGDDYZJRQS"]);
    }

    #[test]
    fn an_update_for_chapters_alone_leaves_the_details_as_they_were() {
        let host = update_host();
        let manga = manga_update(&host, saved_manga(), false, true).unwrap();
        assert_eq!(manga.title, "Old Title");
        assert_eq!(manga.chapters.unwrap().len(), 6);
        assert_eq!(urls(&host).len(), 1);
        assert!(urls(&host)[0].starts_with("Post /api/v1/books/list"));
    }

    #[test]
    fn an_update_that_needs_nothing_sends_nothing() {
        let host = FakeHost::new();
        let manga = manga_update(&host, saved_manga(), false, false).unwrap();
        assert_eq!(manga.title, "Old Title");
        assert!(host.requests().is_empty());
    }

    #[test]
    fn an_update_of_a_series_komga_no_longer_has_says_so() {
        let host = FakeHost::configured().respond(
            Method::Get,
            &format!("{BASE}/api/v1/series/0RZXGDDYZJRQS"),
            404,
            "{}",
        );
        let err = manga_update(&host, saved_manga(), true, true).unwrap_err();
        assert!(message(err).starts_with("Komga couldn't find that (HTTP 404)."));
    }

    fn chapter(key: &str) -> Chapter {
        Chapter {
            key: key.into(),
            title: None,
            volume: None,
            chapter: Some(1.0),
            date_updated: None,
            scanlator: None,
            url: None,
            lang: None,
            paywalled: None,
            kind: ContentKind::Images,
        }
    }

    #[test]
    fn an_invalid_text_spine_is_refused_before_any_resource_is_requested() {
        let host = FakeHost::configured().json(
            Method::Get,
            "/api/v1/books/B/manifest/epub",
            r#"{"readingOrder":[
                {"href":"/api/v1/books/B/resource/EPUB/one.xhtml"},
                {"href":"/api/v1/books/B/resource/EPUB/%2e%2e/two.xhtml"}
            ]}"#,
        );
        let mut chapter = chapter("B");
        chapter.kind = ContentKind::Text;
        assert!(page_list(&host, &chapter).is_err());
        assert_eq!(host.requests().len(), 1);
    }

    #[test]
    fn an_epub_without_a_reading_order_is_not_an_empty_success() {
        let host = FakeHost::configured().json(
            Method::Get,
            "/api/v1/books/B/manifest/epub",
            r#"{"readingOrder":[]}"#,
        );
        let mut chapter = chapter("B");
        chapter.kind = ContentKind::Text;
        assert!(page_list(&host, &chapter).is_err());
    }

    #[test]
    fn a_chapters_pages_are_addressed_from_one_and_listed_from_zero() {
        let host = FakeHost::configured().json(
            Method::Get,
            "/api/v1/books/0RZXGDDZ3JRXJ/pages",
            fixtures::PAGES_NIGHT_HARBOR_V01,
        );
        let pages = page_list(&host, &chapter("0RZXGDDZ3JRXJ")).unwrap();

        assert_eq!(pages.len(), 12);
        for (index, page) in pages.iter().enumerate() {
            assert_eq!(page.index, index as i32);
            let ito_rs::models::PageContent::Url(url) = &page.content else {
                panic!("expected a URL page");
            };
            assert_eq!(
                url,
                &format!(
                    "ito://com.atiinlabs.komga/api/v1/books/0RZXGDDZ3JRXJ/pages/{}",
                    index + 1
                )
            );
            assert!(!page.has_description);
            assert!(page.description.is_none());
            assert!(page.headers.is_none());
        }
    }

    #[test]
    fn native_image_resolution_uses_the_configured_server_and_api_key_without_network() {
        let host = FakeHost::configured();
        let request = image_request(&host, "api/v1/series/S1/thumbnail").unwrap();
        assert_eq!(request.url, format!("{BASE}/api/v1/series/S1/thumbnail"));
        let headers = request.headers.unwrap();
        assert_eq!(headers["X-API-Key"], "test-api-key");
        assert_eq!(headers["Accept"], api::IMAGE_ACCEPT);
        assert!(!headers.contains_key("Authorization"));
        assert!(host.requests().is_empty());
    }

    #[test]
    fn native_images_keep_the_base_prefix_query_and_basic_auth_without_network() {
        let host = FakeHost::new()
            .setting("server_url", "https://proxy.test:8443/komga/")
            .setting("username", "user")
            .setting("password", "pass");
        let request = image_request(&host, "api/v1/books/B/pages/3?convert=jpeg").unwrap();
        assert_eq!(
            request.url,
            "https://proxy.test:8443/komga/api/v1/books/B/pages/3?convert=jpeg"
        );
        let headers = request.headers.unwrap();
        assert_eq!(headers["Authorization"], "Basic dXNlcjpwYXNz");
        assert_eq!(headers["Accept"], api::IMAGE_ACCEPT);
        assert!(!headers.contains_key("X-API-Key"));
        assert_eq!(
            image_origins(&host).unwrap(),
            ["https://proxy.test:8443/komga"]
        );
        assert!(host.requests().is_empty());
    }

    #[test]
    fn image_authority_is_independent_of_refused_locators_and_tracks_configuration() {
        let host = FakeHost::configured();
        for path in [
            "https://evil.test/api/v1/books/B/pages/1",
            "//evil.test/api/v1/books/B/pages/1",
            "ito://evil.test/api/v1/books/B/pages/1",
            "api/v1/../../admin",
            "api/v1/books/../admin",
            "api/v1/books/B\\pages/1",
            "api/v1/books/B/pages/\n1",
            "api/v1/books/B/pages/\u{0}1",
            "api/v1/books/B/pages/ 1",
        ] {
            assert_eq!(image_origins(&host).unwrap(), [BASE]);
            let native = image_request(&host, path).unwrap_err();
            let legacy = image(&host, path).unwrap_err();
            assert_eq!(message(native), message(legacy), "{path:?}");
            assert_eq!(image_origins(&host).unwrap(), [BASE]);
        }
        host.set_setting("server_url", "http://local.test:25600/komga/");
        assert_eq!(
            image_origins(&host).unwrap(),
            ["http://local.test:25600/komga"]
        );
        assert!(host.requests().is_empty());
    }

    #[test]
    fn a_native_image_path_is_validated_even_before_loading_missing_settings() {
        let host = FakeHost::new();
        let err = image_request(&host, "https://evil.test/image").unwrap_err();
        assert!(matches!(err, ito_rs::Error::Host(_)));
        assert!(host.requests().is_empty());
    }

    #[test]
    fn native_images_and_authority_report_typed_setup_errors() {
        for host in [FakeHost::new(), FakeHost::new().setting("server_url", BASE)] {
            assert!(matches!(
                image_origins(&host).unwrap_err(),
                ito_rs::Error::NeedsSetup(_)
            ));
            assert!(matches!(
                image_request(&host, "api/v1/series/S1/thumbnail").unwrap_err(),
                ito_rs::Error::NeedsSetup(_)
            ));
            assert!(host.requests().is_empty());
        }
    }

    #[test]
    fn page_locators_do_not_embed_basic_auth_or_the_server() {
        let host = FakeHost::new()
            .setting("server_url", "https://proxy.test/komga/")
            .setting("username", "user")
            .setting("password", "pass")
            .respond(
                Method::Get,
                "https://proxy.test/komga/api/v1/books/B/pages",
                200,
                fixtures::PAGES_NIGHT_HARBOR_V01,
            );
        let pages = page_list(&host, &chapter("B")).unwrap();
        for (index, page) in pages.iter().enumerate() {
            let ito_rs::models::PageContent::Url(url) = &page.content else {
                panic!("expected a URL page");
            };
            assert_eq!(
                url,
                &format!(
                    "ito://com.atiinlabs.komga/api/v1/books/B/pages/{}",
                    index + 1
                )
            );
            assert!(page.headers.is_none());
        }
    }

    #[test]
    fn a_saved_page_resolves_current_settings_and_still_works_on_legacy_hosts() {
        let host = FakeHost::configured().json(
            Method::Get,
            "/api/v1/books/B/pages",
            fixtures::PAGES_NIGHT_HARBOR_V01,
        );
        let pages = page_list(&host, &chapter("B")).unwrap();
        let ito_rs::models::PageContent::Url(url) = &pages[0].content else {
            panic!("expected a URL page");
        };
        let path = url.strip_prefix("ito://com.atiinlabs.komga/").unwrap();
        host.set_setting("server_url", "https://moved.test/komga/");
        host.set_setting("api_key", "new-key");
        let descriptor = image_request(&host, path).unwrap();
        assert_eq!(
            descriptor.url,
            "https://moved.test/komga/api/v1/books/B/pages/1"
        );
        assert_eq!(descriptor.headers.unwrap()["X-API-Key"], "new-key");
        assert_eq!(host.requests().len(), 1, "resolution sends no request");
        host.add_response(
            Method::Get,
            "https://moved.test/komga/api/v1/books/B/pages/1",
            200,
            vec![0xFF, 0xD8, 0xFF],
        );
        assert_eq!(image(&host, path).unwrap(), [0xFF, 0xD8, 0xFF]);
        let requests = host.requests();
        assert_eq!(requests[1].url, descriptor.url);
        assert!(
            requests[1]
                .headers
                .iter()
                .any(|(name, value)| { name == "X-API-Key" && value == "new-key" })
        );
    }

    #[test]
    fn a_book_komga_cannot_serve_reports_the_status() {
        let host = FakeHost::configured().respond(
            Method::Get,
            &format!("{BASE}/api/v1/books/B/pages"),
            404,
            "{}",
        );
        let err = page_list(&host, &chapter("B")).unwrap_err();
        assert!(message(err).contains("HTTP 404"));
    }

    #[test]
    fn a_cover_request_returns_the_image_bytes() {
        let host = FakeHost::configured().respond(
            Method::Get,
            &format!("{BASE}/api/v1/series/0RZXGDDYZJRQS/thumbnail"),
            200,
            vec![0xFF, 0xD8, 0xFF],
        );
        let bytes = image(&host, "api/v1/series/0RZXGDDYZJRQS/thumbnail").unwrap();
        assert_eq!(bytes, [0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn a_saved_cover_follows_the_server_url_when_it_changes() {
        let cover = mapping::cover_url("S1");
        let path = cover.strip_prefix("ito://com.atiinlabs.komga/").unwrap();

        let host = FakeHost::configured().respond(
            Method::Get,
            &format!("{BASE}/api/v1/series/S1/thumbnail"),
            200,
            vec![1],
        );
        assert_eq!(image(&host, path).unwrap(), [1]);

        host.set_setting("server_url", "http://192.168.1.20:25600/");
        host.add_response(
            Method::Get,
            "http://192.168.1.20:25600/api/v1/series/S1/thumbnail",
            200,
            vec![2],
        );
        assert_eq!(image(&host, path).unwrap(), [2]);
    }

    #[test]
    fn image_requests_for_other_hosts_are_refused_and_send_nothing() {
        let host = FakeHost::configured();
        let err = image(&host, "https://evil.example/steal.jpg").unwrap_err();
        assert!(message(err).contains("api/v1/"));
        assert!(host.requests().is_empty());
    }

    #[test]
    fn an_image_error_status_is_an_error() {
        let host = FakeHost::configured().respond(
            Method::Get,
            &format!("{BASE}/api/v1/series/S1/thumbnail"),
            401,
            "",
        );
        let err = image(&host, "api/v1/series/S1/thumbnail").unwrap_err();
        assert_eq!(message(err), "Komga rejected the API key (HTTP 401).");
    }

    #[test]
    fn an_image_request_without_settings_asks_the_user_to_configure_the_plugin() {
        let err = image(&FakeHost::new(), "api/v1/series/S1/thumbnail").unwrap_err();
        assert_eq!(message(err), NOT_CONFIGURED);
    }

    #[test]
    fn test_connection_reports_the_libraries_the_account_can_see() {
        for (body, expected) in [
            (
                "[]",
                "Connected to Komga. No libraries are shared with this account.",
            ),
            (
                r#"[{"id":"L1","name":"Manga"}]"#,
                "Connected to Komga. 1 library is available.",
            ),
            (
                r#"[{"id":"L1","name":"Manga"},{"id":"L2","name":"Comics"}]"#,
                "Connected to Komga. 2 libraries are available.",
            ),
        ] {
            let host = FakeHost::configured().respond(
                Method::Get,
                &format!("{BASE}{LIBRARIES}"),
                200,
                body,
            );
            assert_eq!(test_connection(&host).unwrap(), expected);
            let requests = host.requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].method, Method::Get);
            assert!(requests[0].body.is_none());
        }
    }

    #[test]
    fn test_connection_keeps_the_category_the_settings_screen_acts_on() {
        let rejected =
            FakeHost::configured().respond(Method::Get, &format!("{BASE}{LIBRARIES}"), 401, "");
        assert!(matches!(
            test_connection(&rejected),
            Err(ito_rs::Error::Unauthorized(message)) if message == "Komga rejected the API key (HTTP 401)."
        ));
        let down = FakeHost::configured().fail(Method::Get, &format!("{BASE}{LIBRARIES}"));
        assert!(matches!(test_connection(&down), Err(ito_rs::Error::Net(_))));

        let unconfigured = FakeHost::new();
        assert!(matches!(
            test_connection(&unconfigured),
            Err(ito_rs::Error::NeedsSetup(message)) if message == NOT_CONFIGURED
        ));
        assert!(unconfigured.requests().is_empty());
    }

    #[test]
    fn only_the_declared_settings_action_runs() {
        let host =
            FakeHost::configured().respond(Method::Get, &format!("{BASE}{LIBRARIES}"), 200, "[]");
        for id in ["", "Test_Connection", "test_connection ", "delete"] {
            assert!(matches!(
                run_settings_action(&host, id),
                Err(ito_rs::Error::Unsupported)
            ));
        }
        assert!(host.requests().is_empty());
        assert!(run_settings_action(&host, TEST_CONNECTION).is_ok());
        assert_eq!(host.requests().len(), 1);
    }
}
