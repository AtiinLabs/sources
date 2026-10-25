//! The slice of Komga's REST payloads this plugin reads.
//!
//! Only fields the plugin uses are declared; Komga sends many more and serde skips them.
//! Komga documents several enum fields as strict enums but sends `""` for unset ones
//! (`readingDirection`, `mediaProfile`), so enums are kept as strings and interpreted in
//! [`crate::mapping`], where an unknown value maps to a default instead of failing the
//! whole response. A field that is missing or `null` falls back to its default for the
//! same reason.

use serde::{Deserialize, Deserializer};

/// Deserializes `null` as the type's default; combined with `#[serde(default)]` it also
/// covers a missing field.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A page of results (Spring's `Page`).
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Paged<T> {
    #[serde(default = "Vec::new", deserialize_with = "or_default")]
    pub content: Vec<T>,
    /// `true` on the final page. A response without it is treated as the final page so a
    /// misbehaving server cannot make the plugin page forever.
    #[serde(default)]
    pub last: Option<bool>,
}

impl<T> Paged<T> {
    pub fn has_next(&self) -> bool {
        self.last == Some(false)
    }
}

#[derive(Debug, Deserialize)]
pub struct LibraryDto {
    pub id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesDto {
    pub id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub name: String,
    /// Set when the series' files disappeared and Komga moved it to the trash.
    #[serde(default, deserialize_with = "or_default")]
    pub deleted: bool,
    #[serde(default, deserialize_with = "or_default")]
    pub metadata: SeriesMetadataDto,
    #[serde(default, deserialize_with = "or_default")]
    pub books_metadata: BooksMetadataDto,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesMetadataDto {
    /// `ONGOING`, `ENDED`, `ABANDONED` or `HIATUS`.
    #[serde(default, deserialize_with = "or_default")]
    pub status: String,
    #[serde(default, deserialize_with = "or_default")]
    pub title: String,
    #[serde(default, deserialize_with = "or_default")]
    pub summary: String,
    /// `LEFT_TO_RIGHT`, `RIGHT_TO_LEFT`, `VERTICAL`, `WEBTOON`, or `""` when unset.
    #[serde(default, deserialize_with = "or_default")]
    pub reading_direction: String,
    #[serde(default, deserialize_with = "or_default")]
    pub genres: Vec<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub tags: Vec<String>,
}

/// What Komga aggregates from a series' books. Authors live here, not in the series metadata.
#[derive(Debug, Default, Deserialize)]
pub struct BooksMetadataDto {
    #[serde(default, deserialize_with = "or_default")]
    pub authors: Vec<AuthorDto>,
    #[serde(default, deserialize_with = "or_default")]
    pub summary: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct AuthorDto {
    #[serde(default, deserialize_with = "or_default")]
    pub name: String,
    /// Lowercase by convention: `writer`, `penciller`, `colorist`, ...
    #[serde(default, deserialize_with = "or_default")]
    pub role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookDto {
    pub id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub series_id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub series_title: String,
    #[serde(default, deserialize_with = "or_default")]
    pub name: String,
    /// When Komga first saw the file, as `YYYY-MM-DDTHH:MM:SSZ`.
    #[serde(default, deserialize_with = "or_default")]
    pub created: String,
    /// Set when the file disappeared. Komga keeps such a book's media info, so its status
    /// still reads `READY` even though the pages can no longer be served.
    #[serde(default, deserialize_with = "or_default")]
    pub deleted: bool,
    #[serde(default, deserialize_with = "or_default")]
    pub media: MediaDto,
    #[serde(default, deserialize_with = "or_default")]
    pub metadata: BookMetadataDto,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDto {
    /// `UNKNOWN`, `ERROR`, `READY`, `UNSUPPORTED` or `OUTDATED`.
    #[serde(default, deserialize_with = "or_default")]
    pub status: String,
    /// `DIVINA` (image archives), `PDF`, `EPUB`, or `""` when Komga could not read the file.
    #[serde(default, deserialize_with = "or_default")]
    pub media_profile: String,
    /// An EPUB whose pages are images, so Komga can serve them like an image archive.
    #[serde(default, deserialize_with = "or_default")]
    pub epub_divina_compatible: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookMetadataDto {
    #[serde(default, deserialize_with = "or_default")]
    pub title: String,
    /// Position within the series; fractional for interludes such as `2.5`.
    #[serde(default)]
    pub number_sort: Option<f64>,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub release_date: Option<String>,
}

/// One entry of `GET /api/v1/books/{id}/pages`.
#[derive(Debug, Deserialize)]
pub struct PageDto {
    /// 1-based, as used in `.../pages/{number}`.
    pub number: i32,
}

/// The EPUB Web Publication manifest fields needed to read documents in spine order.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebPubManifestDto {
    pub reading_order: Vec<WebPubLinkDto>,
}

#[derive(Debug, Deserialize)]
pub struct WebPubLinkDto {
    pub href: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixtures;
    use serde_json::json;

    #[test]
    fn reading_order_preserves_server_order_and_ignores_optional_metadata() {
        let manifest: WebPubManifestDto = serde_json::from_str(
            r#"{
                "metadata":{"title":"A book","unknown":true},
                "readingOrder":[
                    {"href":"/api/v1/books/B1/resource/z.xhtml","type":"application/xhtml+xml"},
                    {"href":"/api/v1/books/B1/resource/a.xhtml","properties":{"page":"left"}}
                ],
                "resources":[{"href":"cover.jpg"}]
            }"#,
        )
        .unwrap();
        let hrefs: Vec<&str> = manifest
            .reading_order
            .iter()
            .map(|link| link.href.as_str())
            .collect();
        assert_eq!(
            hrefs,
            [
                "/api/v1/books/B1/resource/z.xhtml",
                "/api/v1/books/B1/resource/a.xhtml",
            ]
        );
    }

    #[test]
    fn reading_order_and_each_href_are_required_and_typed() {
        for payload in [
            r#"{}"#,
            r#"{"readingOrder":null}"#,
            r#"{"readingOrder":{}}"#,
            r#"{"readingOrder":[{}]}"#,
            r#"{"readingOrder":[{"href":null}]}"#,
            r#"{"readingOrder":[{"href":1}]}"#,
        ] {
            assert!(serde_json::from_str::<WebPubManifestDto>(payload).is_err());
        }
    }

    fn series_page(fixture: &str) -> Paged<SeriesDto> {
        serde_json::from_str(fixture).unwrap()
    }

    fn book_page(fixture: &str) -> Paged<BookDto> {
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn libraries_parse() {
        let libraries: Vec<LibraryDto> = serde_json::from_str(fixtures::LIBRARIES).unwrap();
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].id, "0RZXGDDWFJZ22");
        assert_eq!(libraries[0].name, "Comics");
    }

    #[test]
    fn a_series_parses_with_its_metadata_and_authors() {
        let series: SeriesDto = serde_json::from_str(fixtures::SERIES_NIGHT_HARBOR).unwrap();
        assert_eq!(series.id, "0RZXGDDYZJRQS");
        assert_eq!(series.name, "Night Harbor");
        assert!(!series.deleted);

        let metadata = &series.metadata;
        assert_eq!(metadata.status, "ONGOING");
        assert_eq!(metadata.title, "Night Harbor");
        assert_eq!(
            metadata.summary,
            "A detective returns to the harbor town she left behind."
        );
        assert_eq!(metadata.reading_direction, "RIGHT_TO_LEFT");
        assert_eq!(metadata.genres, ["drama", "mystery"]);
        assert_eq!(metadata.tags, ["drama", "harbor", "noir"]);

        let authors: Vec<(&str, &str)> = series
            .books_metadata
            .authors
            .iter()
            .map(|author| (author.name.as_str(), author.role.as_str()))
            .collect();
        assert_eq!(
            authors,
            [
                ("Aiko Tanaka", "writer"),
                ("Ren Ito", "penciller"),
                ("Mina Park", "colorist"),
                ("Aiko Tanaka", "penciller")
            ]
        );
        assert_eq!(
            series.books_metadata.summary,
            "The detective takes the night ferry home."
        );
    }

    #[test]
    fn a_page_of_series_knows_whether_more_follow() {
        let all = series_page(fixtures::SERIES_LIST);
        assert_eq!(all.content.len(), 5);
        assert_eq!(all.last, Some(true));
        assert!(!all.has_next());

        let first = series_page(fixtures::SERIES_LIST_FIRST_PAGE);
        assert_eq!(first.content.len(), 2);
        assert_eq!(first.last, Some(false));
        assert!(first.has_next());
    }

    #[test]
    fn a_series_komga_moved_to_the_trash_is_flagged() {
        let all = series_page(fixtures::SERIES_LIST);
        let flagged: Vec<(&str, bool)> = all
            .content
            .iter()
            .map(|series| (series.metadata.title.as_str(), series.deleted))
            .collect();
        assert_eq!(
            flagged,
            [
                ("Ghost Town", true),
                ("Glass Orchard", false),
                ("Night Harbor", false),
                ("Paper Lanterns", false),
                ("Vertical Rain", false)
            ]
        );
    }

    #[test]
    fn a_series_nobody_described_has_empty_strings_not_missing_fields() {
        let plain = series_page(fixtures::SERIES_LIST_PLAIN);
        let series = &plain.content[0];
        assert_eq!(series.metadata.reading_direction, "");
        assert_eq!(series.metadata.summary, "");
        assert!(series.metadata.genres.is_empty());
        assert!(series.books_metadata.authors.is_empty());
    }

    #[test]
    fn a_book_parses_with_its_number_title_and_dates() {
        let books = book_page(fixtures::BOOKS_NIGHT_HARBOR).content;
        assert_eq!(books.len(), 6);
        let first = &books[0];
        assert_eq!(first.id, "0RZXGDDZ3JRXJ");
        assert_eq!(first.series_id, "0RZXGDDYZJRQS");
        assert_eq!(first.series_title, "Night Harbor");
        assert_eq!(first.name, "Night Harbor v01");
        assert_eq!(first.created, "2026-10-21T06:31:43Z");
        assert!(!first.deleted);
        assert_eq!(first.metadata.title, "The Lighthouse Keeper");
        assert_eq!(first.metadata.number_sort, Some(1.0));
        assert_eq!(first.metadata.release_date.as_deref(), Some("2020-01-05"));

        // A book without a release date has null there.
        assert_eq!(books[3].metadata.release_date, None);
        // Fractional numbers survive.
        assert_eq!(books[2].metadata.number_sort, Some(2.5));
    }

    #[test]
    fn a_books_media_state_parses_for_every_kind_of_file() {
        let books = book_page(fixtures::BOOKS_VERTICAL_RAIN).content;
        let media: Vec<(&str, &str, &str, bool)> = books
            .iter()
            .map(|book| {
                (
                    book.name.as_str(),
                    book.media.status.as_str(),
                    book.media.media_profile.as_str(),
                    book.media.epub_divina_compatible,
                )
            })
            .collect();
        assert_eq!(
            media,
            [
                ("Vertical Rain v01", "READY", "DIVINA", false),
                ("Vertical Rain v02", "READY", "EPUB", false),
                ("Vertical Rain v03", "READY", "EPUB", true),
                // Komga could not read this file and sends an empty profile.
                ("Vertical Rain v04", "UNSUPPORTED", "", false),
                ("Vertical Rain v05", "READY", "PDF", false),
            ]
        );
    }

    #[test]
    fn a_trashed_book_is_flagged_but_still_reads_ready() {
        let books = book_page(fixtures::BOOKS_PAPER_LANTERNS).content;
        let trashed: Vec<&BookDto> = books.iter().filter(|book| book.deleted).collect();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].name, "Paper Lanterns v04");
        assert_eq!(trashed[0].media.status, "READY");
    }

    #[test]
    fn books_in_progress_and_on_deck_carry_their_series() {
        let in_progress = book_page(fixtures::BOOKS_IN_PROGRESS).content;
        let series: Vec<(&str, &str)> = in_progress
            .iter()
            .map(|book| (book.series_title.as_str(), book.name.as_str()))
            .collect();
        assert_eq!(
            series,
            [
                ("Glass Orchard", "Glass Orchard v01"),
                ("Night Harbor", "Night Harbor v04"),
                ("Night Harbor", "Night Harbor v02")
            ]
        );

        let on_deck = book_page(fixtures::BOOKS_ONDECK).content;
        assert_eq!(on_deck.len(), 1);
        assert_eq!(on_deck[0].series_title, "Paper Lanterns");
        assert_eq!(on_deck[0].series_id, "0RZXGDDYZJRQM");
    }

    #[test]
    fn the_pages_of_a_book_are_numbered_from_one() {
        let pages: Vec<PageDto> = serde_json::from_str(fixtures::PAGES_NIGHT_HARBOR_V01).unwrap();
        let numbers: Vec<i32> = pages.iter().map(|page| page.number).collect();
        assert_eq!(numbers, (1..=12).collect::<Vec<_>>());
    }

    #[test]
    fn the_feeds_parse_like_any_other_page_of_series() {
        assert_eq!(series_page(fixtures::SERIES_NEW).content.len(), 5);
        assert_eq!(series_page(fixtures::SERIES_UPDATED).content.len(), 3);
        assert_eq!(series_page(fixtures::SERIES_SEARCH_HARBOR).content.len(), 1);
    }

    #[test]
    fn a_page_without_content_or_last_is_an_empty_final_page() {
        for body in ["{}", r#"{"content":null,"last":null}"#] {
            let page: Paged<SeriesDto> = serde_json::from_str(body).unwrap();
            assert!(page.content.is_empty());
            assert!(!page.has_next());
        }
    }

    #[test]
    fn fields_the_plugin_does_not_read_are_ignored() {
        let series: SeriesDto = serde_json::from_value(json!({
            "id": "S",
            "somethingNew": { "nested": [1, 2, 3] },
            "metadata": { "title": "T", "futureField": true }
        }))
        .unwrap();
        assert_eq!(series.metadata.title, "T");
    }

    #[test]
    fn a_response_that_is_not_the_expected_shape_is_an_error() {
        assert!(
            serde_json::from_str::<SeriesDto>("{}").is_err(),
            "a series needs an id"
        );
        assert!(serde_json::from_str::<BookDto>("[]").is_err());
        assert!(serde_json::from_str::<Paged<BookDto>>("<html>").is_err());
    }
}
