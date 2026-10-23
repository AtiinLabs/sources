//! Komga's data, shaped the way Ito wants it.
//!
//! Pure functions only: nothing here touches the network or the host.

use std::collections::HashSet;

use ito_rs::models::manga::{Chapter, ContentRating, Manga, Status, Viewer};
use ito_rs::models::{Page, PageContent};

use crate::dto::{BookDto, SeriesDto};
use crate::time;

pub const PLUGIN_ID: &str = "com.atiinlabs.komga";

/// A series cover as an `ito://` URL.
///
/// Ito resolves `ito://<plugin id>/<path>` through the native image-request descriptor,
/// or `handle_image` on older hosts. The locator contains neither a server nor credentials,
/// so a cover saved in the library keeps working after the server URL changes.
pub fn cover_url(series_id: &str) -> String {
    format!("ito://{PLUGIN_ID}/api/v1/series/{series_id}/thumbnail")
}

/// A stable plugin-owned book page locator. Komga numbers pages from 1.
pub fn page_url(book_id: &str, number: i32) -> String {
    format!("ito://{PLUGIN_ID}/api/v1/books/{book_id}/pages/{number}")
}

fn series_web_url(base: &str, series_id: &str) -> String {
    format!("{base}/series/{series_id}")
}

/// A series as Ito's title, with details and without chapters.
pub fn series_to_manga(series: &SeriesDto, base: &str) -> Manga {
    let metadata = &series.metadata;
    Manga {
        key: series.id.clone(),
        title: non_blank(&metadata.title)
            .or_else(|| non_blank(&series.name))
            .unwrap_or(&series.id)
            .to_owned(),
        authors: authors(series),
        artist: None,
        description: non_blank(&metadata.summary)
            .or_else(|| non_blank(&series.books_metadata.summary))
            .map(str::to_owned),
        tags: some_if_any(unique(
            metadata
                .genres
                .iter()
                .chain(&metadata.tags)
                .map(String::as_str),
        )),
        cover: Some(cover_url(&series.id)),
        url: Some(series_web_url(base, &series.id)),
        status: status_for(&metadata.status),
        content_rating: ContentRating::Safe,
        nsfw: 0,
        viewer: viewer_for(&metadata.reading_direction),
        chapters: None,
    }
}

/// A series card built from one of its books.
///
/// Books carry their series' id and title, which is all a home row needs, so "Keep reading"
/// and "On deck" don't cost a request per series. Opening the card loads the full series.
pub fn series_stub(book: &BookDto, base: &str) -> Option<Manga> {
    let id = non_blank(&book.series_id)?;
    Some(Manga {
        key: id.to_owned(),
        title: non_blank(&book.series_title).unwrap_or(id).to_owned(),
        authors: None,
        artist: None,
        description: None,
        tags: None,
        cover: Some(cover_url(id)),
        url: Some(series_web_url(base, id)),
        status: Status::Unknown,
        content_rating: ContentRating::Safe,
        nsfw: 0,
        viewer: Viewer::Default,
        chapters: None,
    })
}

/// Drops later entries that repeat an earlier entry's key. Ito lists identify titles by key,
/// and several in-progress books can belong to one series.
pub fn dedupe(entries: Vec<Manga>) -> Vec<Manga> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter(|manga| seen.insert(manga.key.clone()))
        .collect()
}

/// Writers and pencillers, each once, in the order Komga lists them.
fn authors(series: &SeriesDto) -> Option<Vec<String>> {
    some_if_any(unique(
        series
            .books_metadata
            .authors
            .iter()
            .filter(|author| {
                matches!(
                    author.role.trim().to_ascii_lowercase().as_str(),
                    "writer" | "penciller"
                )
            })
            .map(|author| author.name.as_str()),
    ))
}

pub fn status_for(status: &str) -> Status {
    match status.trim().to_ascii_uppercase().as_str() {
        "ONGOING" => Status::Ongoing,
        "ENDED" => Status::Completed,
        "ABANDONED" => Status::Cancelled,
        "HIATUS" => Status::Hiatus,
        _ => Status::Unknown,
    }
}

pub fn viewer_for(reading_direction: &str) -> Viewer {
    match reading_direction.trim().to_ascii_uppercase().as_str() {
        "LEFT_TO_RIGHT" => Viewer::Ltr,
        "RIGHT_TO_LEFT" => Viewer::Rtl,
        "VERTICAL" => Viewer::Vertical,
        "WEBTOON" => Viewer::Webtoon,
        _ => Viewer::Default,
    }
}

/// Whether Ito can read a book.
///
/// Komga only serves pages for media it has analysed (`READY`) and whose files still exist,
/// and a text EPUB has no page images at all. An EPUB whose pages are images
/// (`epubDivinaCompatible`) and a PDF (rendered by Komga) are fine.
pub fn is_readable(book: &BookDto) -> bool {
    let media = &book.media;
    let text_epub =
        media.media_profile.eq_ignore_ascii_case("EPUB") && !media.epub_divina_compatible;
    !book.deleted && media.status.eq_ignore_ascii_case("READY") && !text_epub
}

/// The readable books as chapters, newest first like Ito's other sources.
///
/// `books` is in reading order (Komga sorts by `metadata.numberSort`). Equal numbers keep
/// their relative order, reversed along with the rest.
pub fn chapters(books: &[BookDto]) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = books
        .iter()
        .rev()
        .filter(|book| is_readable(book))
        .map(chapter)
        .collect();
    chapters.sort_by(|a, b| sort_key(b).total_cmp(&sort_key(a)));
    chapters
}

/// Unnumbered chapters sort below every numbered one.
fn sort_key(chapter: &Chapter) -> f32 {
    chapter.chapter.unwrap_or(f32::NEG_INFINITY)
}

fn chapter(book: &BookDto) -> Chapter {
    let number = book.metadata.number_sort;
    Chapter {
        key: book.id.clone(),
        title: chapter_title(&book.metadata.title, number),
        volume: None,
        chapter: number.map(|number| number as f32),
        date_updated: chapter_date(book),
        scanlator: None,
        url: None,
        lang: None,
        paywalled: None,
    }
}

/// The release date if Komga knows it, otherwise when the file was added.
fn chapter_date(book: &BookDto) -> Option<f64> {
    book.metadata
        .release_date
        .as_deref()
        .and_then(time::epoch_seconds)
        .or_else(|| time::epoch_seconds(&book.created))
}

/// A book's title, unless it only restates the number.
///
/// Ito shows a chapter's title, or "Chapter N" when it has none. A title that says no more
/// than "1", "#1" or "Chapter 1" would hide nothing and add nothing, so it is dropped; any
/// other title ("Vol. 1", "Night Harbor v01", "Chapter 1: The Dock") is kept as written.
fn chapter_title(title: &str, number: Option<f64>) -> Option<String> {
    let title = non_blank(title)?;
    if restates_number(title, number) {
        None
    } else {
        Some(title.to_owned())
    }
}

fn restates_number(title: &str, number: Option<f64>) -> bool {
    let Some(number) = number else { return false };
    let title = title.to_ascii_lowercase();
    // The longest labels come first: "ch" is a prefix of "chapter".
    let digits = ["chapter", "chap.", "chap", "ch.", "ch", "#"]
        .iter()
        .find_map(|label| title.strip_prefix(label))
        .unwrap_or(&title)
        .trim_start();
    let numeric = digits.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && digits.bytes().any(|b| b.is_ascii_digit());
    numeric
        && digits
            .parse::<f64>()
            .is_ok_and(|n| (n - number).abs() < 1e-4)
}

/// A book page as a reader page. `index` counts from 0, as Ito's pages do.
pub fn page(index: i32, url: String) -> Page {
    Page {
        index,
        content: PageContent::Url(url),
        has_description: false,
        description: None,
        headers: None,
    }
}

fn non_blank(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.is_empty() { None } else { Some(text) }
}

/// The non-blank items, each once (ignoring case), in their original order.
fn unique<'a>(items: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter_map(non_blank)
        .filter(|item| seen.insert(item.to_lowercase()))
        .map(str::to_owned)
        .collect()
}

fn some_if_any(items: Vec<String>) -> Option<Vec<String>> {
    if items.is_empty() { None } else { Some(items) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::Paged;
    use crate::testing::{BASE, fixtures};
    use serde_json::{Value, json};

    fn series_list(fixture: &str) -> Vec<SeriesDto> {
        serde_json::from_str::<Paged<SeriesDto>>(fixture)
            .unwrap()
            .content
    }

    fn series_named(fixture: &str, title: &str) -> SeriesDto {
        series_list(fixture)
            .into_iter()
            .find(|series| series.metadata.title == title)
            .unwrap_or_else(|| panic!("no series titled {title}"))
    }

    fn books(fixture: &str) -> Vec<BookDto> {
        serde_json::from_str::<Paged<BookDto>>(fixture)
            .unwrap()
            .content
    }

    fn book_json(value: Value) -> BookDto {
        serde_json::from_value(value).unwrap()
    }

    fn keys(chapters: &[Chapter]) -> Vec<&str> {
        chapters
            .iter()
            .map(|chapter| chapter.key.as_str())
            .collect()
    }

    fn numbers(chapters: &[Chapter]) -> Vec<Option<f32>> {
        chapters.iter().map(|chapter| chapter.chapter).collect()
    }

    #[test]
    fn covers_are_server_relative_ito_urls() {
        assert_eq!(
            cover_url("0RZXGDDYZJRQS"),
            "ito://com.atiinlabs.komga/api/v1/series/0RZXGDDYZJRQS/thumbnail"
        );
        // What follows the plugin id is exactly the server-relative path `handle_image` accepts.
        assert_eq!(
            cover_url("S").strip_prefix("ito://com.atiinlabs.komga/"),
            Some("api/v1/series/S/thumbnail")
        );
    }

    #[test]
    fn a_series_maps_with_every_detail() {
        let series: SeriesDto = serde_json::from_str(fixtures::SERIES_NIGHT_HARBOR).unwrap();
        let manga = series_to_manga(&series, BASE);

        assert_eq!(manga.key, "0RZXGDDYZJRQS");
        assert_eq!(manga.title, "Night Harbor");
        assert_eq!(
            manga.description.as_deref(),
            Some("A detective returns to the harbor town she left behind.")
        );
        assert_eq!(
            manga.cover.as_deref(),
            Some("ito://com.atiinlabs.komga/api/v1/series/0RZXGDDYZJRQS/thumbnail")
        );
        assert_eq!(
            manga.url.as_deref(),
            Some("https://komga.test/series/0RZXGDDYZJRQS")
        );
        assert!(matches!(manga.status, Status::Ongoing));
        assert!(matches!(manga.viewer, Viewer::Rtl));
        assert!(matches!(manga.content_rating, ContentRating::Safe));
        assert_eq!(manga.nsfw, 0);
        assert!(manga.chapters.is_none());
        assert!(manga.artist.is_none());
    }

    #[test]
    fn authors_are_the_writers_and_pencillers_once_each() {
        // Komga lists Aiko Tanaka as writer and penciller, Ren Ito as penciller and Mina Park
        // as colorist.
        let series: SeriesDto = serde_json::from_str(fixtures::SERIES_NIGHT_HARBOR).unwrap();
        assert_eq!(series.books_metadata.authors.len(), 4);
        assert_eq!(
            series_to_manga(&series, BASE).authors,
            Some(vec!["Aiko Tanaka".to_owned(), "Ren Ito".to_owned()])
        );
    }

    #[test]
    fn author_roles_are_matched_ignoring_case_and_other_roles_are_left_out() {
        let series: SeriesDto = serde_json::from_value(json!({
            "id": "S",
            "booksMetadata": { "authors": [
                { "name": "A", "role": "Writer" },
                { "name": "B", "role": "PENCILLER " },
                { "name": "a", "role": "penciller" },
                { "name": "C", "role": "inker" },
                { "name": "D", "role": "" },
                { "name": "  ", "role": "writer" }
            ]}
        }))
        .unwrap();
        assert_eq!(
            series_to_manga(&series, BASE).authors,
            Some(vec!["A".to_owned(), "B".to_owned()])
        );
    }

    #[test]
    fn a_series_without_writers_or_pencillers_has_no_authors() {
        let series: SeriesDto = serde_json::from_value(json!({
            "id": "S",
            "booksMetadata": { "authors": [ { "name": "C", "role": "colorist" } ] }
        }))
        .unwrap();
        assert_eq!(series_to_manga(&series, BASE).authors, None);
    }

    #[test]
    fn tags_are_genres_then_tags_without_repeats() {
        // Komga lowercases both lists, so "drama" is in each.
        let series: SeriesDto = serde_json::from_str(fixtures::SERIES_NIGHT_HARBOR).unwrap();
        assert_eq!(series.metadata.genres, ["drama", "mystery"]);
        assert_eq!(series.metadata.tags, ["drama", "harbor", "noir"]);
        assert_eq!(
            series_to_manga(&series, BASE).tags,
            Some(vec![
                "drama".into(),
                "mystery".into(),
                "harbor".into(),
                "noir".into()
            ])
        );
    }

    #[test]
    fn tag_repeats_are_found_ignoring_case_and_blank_tags_are_dropped() {
        let series: SeriesDto = serde_json::from_value(json!({
            "id": "S",
            "metadata": { "genres": ["Drama", " "], "tags": ["drama", "Noir", ""] }
        }))
        .unwrap();
        assert_eq!(
            series_to_manga(&series, BASE).tags,
            Some(vec!["Drama".to_owned(), "Noir".to_owned()])
        );
    }

    #[test]
    fn a_series_without_tags_has_none() {
        let series = series_named(fixtures::SERIES_LIST, "Vertical Rain");
        // Its genre list is ["fantasy"] and tag list is empty.
        assert_eq!(
            series_to_manga(&series, BASE).tags,
            Some(vec!["fantasy".to_owned()])
        );

        let plain = series_named(fixtures::SERIES_LIST_PLAIN, "Night Harbor");
        assert_eq!(series_to_manga(&plain, BASE).tags, None);
    }

    #[test]
    fn the_series_summary_wins_over_the_summary_of_its_books() {
        let series: SeriesDto = serde_json::from_str(fixtures::SERIES_NIGHT_HARBOR).unwrap();
        assert_eq!(
            series.books_metadata.summary,
            "The detective takes the night ferry home."
        );
        assert_eq!(
            series_to_manga(&series, BASE).description.as_deref(),
            Some("A detective returns to the harbor town she left behind.")
        );
    }

    #[test]
    fn a_series_without_a_summary_borrows_its_books() {
        let series = series_named(fixtures::SERIES_LIST, "Paper Lanterns");
        assert_eq!(series.metadata.summary, "");
        assert_eq!(
            series_to_manga(&series, BASE).description.as_deref(),
            Some("Lanterns drift over the river at the festival.")
        );
    }

    #[test]
    fn a_series_with_no_summary_anywhere_has_no_description() {
        let series = series_named(fixtures::SERIES_LIST, "Glass Orchard");
        assert_eq!(series_to_manga(&series, BASE).description, None);
    }

    #[test]
    fn every_series_status_has_an_ito_equivalent() {
        for (komga, ito) in [
            ("ONGOING", "Ongoing"),
            ("ENDED", "Completed"),
            ("ABANDONED", "Cancelled"),
            ("HIATUS", "Hiatus"),
            ("", "Unknown"),
            ("SOMETHING_NEW", "Unknown"),
        ] {
            assert_eq!(format!("{:?}", status_for(komga)), ito, "{komga:?}");
        }
        assert_eq!(format!("{:?}", status_for("ongoing")), "Ongoing");
    }

    #[test]
    fn every_reading_direction_has_an_ito_viewer() {
        for (komga, ito) in [
            ("LEFT_TO_RIGHT", "Ltr"),
            ("RIGHT_TO_LEFT", "Rtl"),
            ("VERTICAL", "Vertical"),
            ("WEBTOON", "Webtoon"),
            ("", "Default"),
            ("SOMETHING_NEW", "Default"),
        ] {
            assert_eq!(format!("{:?}", viewer_for(komga)), ito, "{komga:?}");
        }
    }

    #[test]
    fn captured_series_map_to_the_status_and_viewer_komga_reports() {
        for (title, status, viewer) in [
            ("Night Harbor", "Ongoing", "Rtl"),
            ("Paper Lanterns", "Completed", "Ltr"),
            ("Glass Orchard", "Hiatus", "Webtoon"),
            ("Vertical Rain", "Cancelled", "Vertical"),
        ] {
            let manga = series_to_manga(&series_named(fixtures::SERIES_LIST, title), BASE);
            assert_eq!(format!("{:?}", manga.status), status, "{title}");
            assert_eq!(format!("{:?}", manga.viewer), viewer, "{title}");
        }
    }

    #[test]
    fn a_series_komga_has_not_described_still_maps() {
        // As captured from a fresh library: empty strings everywhere, status ONGOING.
        let series = series_named(fixtures::SERIES_LIST_PLAIN, "Glass Orchard");
        assert_eq!(series.metadata.reading_direction, "");
        let manga = series_to_manga(&series, BASE);
        assert_eq!(manga.title, "Glass Orchard");
        assert!(matches!(manga.status, Status::Ongoing));
        assert!(matches!(manga.viewer, Viewer::Default));
        assert_eq!(manga.authors, None);
        assert_eq!(manga.description, None);
        assert_eq!(manga.tags, None);
    }

    #[test]
    fn the_title_falls_back_to_the_series_name() {
        let series: SeriesDto = serde_json::from_value(json!({
            "id": "S", "name": "Folder Name", "metadata": { "title": "  " }
        }))
        .unwrap();
        assert_eq!(series_to_manga(&series, BASE).title, "Folder Name");

        let bare: SeriesDto = serde_json::from_value(json!({ "id": "S" })).unwrap();
        assert_eq!(series_to_manga(&bare, BASE).title, "S");
    }

    #[test]
    fn null_fields_read_as_unset() {
        let series: SeriesDto = serde_json::from_value(json!({
            "id": "S",
            "name": null,
            "deleted": null,
            "metadata": { "status": null, "summary": null, "readingDirection": null, "genres": null, "tags": null, "title": "T" },
            "booksMetadata": { "authors": null, "summary": null }
        }))
        .unwrap();
        let manga = series_to_manga(&series, BASE);
        assert_eq!(manga.title, "T");
        assert!(matches!(manga.status, Status::Unknown));
        assert!(matches!(manga.viewer, Viewer::Default));
        assert_eq!(manga.tags, None);
        assert_eq!(manga.authors, None);
    }

    #[test]
    fn a_book_gives_its_series_a_card_without_another_request() {
        let books = books(fixtures::BOOKS_IN_PROGRESS);
        let stub = series_stub(&books[0], BASE).unwrap();
        assert_eq!(stub.key, "0RZXGDDYQJWS7");
        assert_eq!(stub.title, "Glass Orchard");
        assert_eq!(
            stub.cover.as_deref(),
            Some("ito://com.atiinlabs.komga/api/v1/series/0RZXGDDYQJWS7/thumbnail")
        );
        assert_eq!(
            stub.url.as_deref(),
            Some("https://komga.test/series/0RZXGDDYQJWS7")
        );
        assert!(matches!(stub.status, Status::Unknown));
        assert!(stub.chapters.is_none());
    }

    #[test]
    fn a_book_without_a_series_id_gives_no_card() {
        let book = book_json(json!({ "id": "B", "seriesTitle": "Nameless" }));
        assert!(series_stub(&book, BASE).is_none());
    }

    #[test]
    fn dedupe_keeps_the_first_card_of_each_series() {
        let books = books(fixtures::BOOKS_IN_PROGRESS);
        // Glass Orchard, Night Harbor (v04), Night Harbor (v02).
        let stubs: Vec<Manga> = books
            .iter()
            .filter_map(|book| series_stub(book, BASE))
            .collect();
        assert_eq!(stubs.len(), 3);
        let unique = dedupe(stubs);
        let titles: Vec<&str> = unique.iter().map(|manga| manga.title.as_str()).collect();
        assert_eq!(titles, ["Glass Orchard", "Night Harbor"]);
    }

    #[test]
    fn chapters_are_listed_newest_first() {
        let chapters = chapters(&books(fixtures::BOOKS_NIGHT_HARBOR));
        assert_eq!(
            numbers(&chapters),
            [
                Some(6.0),
                Some(5.0),
                Some(4.0),
                Some(2.5),
                Some(2.0),
                Some(1.0)
            ]
        );
        // Night Harbor v06, then v05, v04, v03 (2.5), v02 and v01.
        assert_eq!(
            keys(&chapters),
            [
                "0RZXGDDZ7JWR5",
                "0RZXGDDZ3JRXM",
                "0RZXGDDZ3JRXK",
                "0RZXGDDZ3JRXH",
                "0RZXGDDZ3JRXG",
                "0RZXGDDZ3JRXJ"
            ]
        );
    }

    #[test]
    fn a_chapter_is_the_book_with_its_number_title_and_date() {
        let chapters = chapters(&books(fixtures::BOOKS_NIGHT_HARBOR));
        let first = chapters.last().unwrap();
        assert_eq!(first.key, "0RZXGDDZ3JRXJ");
        assert_eq!(first.title.as_deref(), Some("The Lighthouse Keeper"));
        assert_eq!(first.chapter, Some(1.0));
        // Released 2020-01-05.
        assert_eq!(first.date_updated, Some(1_578_182_400.0));
        assert!(first.volume.is_none());
        assert!(first.scanlator.is_none());
        assert!(first.url.is_none());
        assert!(first.lang.is_none());
        assert!(first.paywalled.is_none());
    }

    #[test]
    fn a_book_without_a_release_date_is_dated_by_when_it_was_added() {
        let chapters = chapters(&books(fixtures::BOOKS_NIGHT_HARBOR));
        let interlude = chapters.iter().find(|c| c.chapter == Some(2.5)).unwrap();
        // Added 2026-10-21T06:31:43Z.
        assert_eq!(interlude.date_updated, Some(1_792_564_303.0));
        // The release date, not the date added, wins when both exist.
        let v02 = chapters.iter().find(|c| c.chapter == Some(2.0)).unwrap();
        assert_eq!(v02.date_updated, Some(1_580_860_800.0)); // 2020-02-05
    }

    #[test]
    fn fractional_numbers_sit_between_their_neighbours() {
        let chapters = chapters(&books(fixtures::BOOKS_NIGHT_HARBOR));
        let interlude = chapters
            .iter()
            .position(|c| c.chapter == Some(2.5))
            .unwrap();
        assert_eq!(chapters[interlude - 1].chapter, Some(4.0));
        assert_eq!(chapters[interlude + 1].chapter, Some(2.0));
        assert_eq!(chapters[interlude].title.as_deref(), Some("Interlude"));
    }

    #[test]
    fn titles_are_kept_unless_they_only_repeat_the_number() {
        let chapters = chapters(&books(fixtures::BOOKS_NIGHT_HARBOR));
        let title = |number: f32| {
            chapters
                .iter()
                .find(|c| c.chapter == Some(number))
                .and_then(|c| c.title.clone())
        };
        assert_eq!(title(1.0).as_deref(), Some("The Lighthouse Keeper"));
        assert_eq!(title(2.0), None, "\"Chapter 2\" restates the number");
        assert_eq!(title(2.5).as_deref(), Some("Interlude"));
        // What Komga makes of a file name when nothing else is known.
        assert_eq!(title(4.0).as_deref(), Some("Night Harbor v04"));
    }

    #[test]
    fn restating_titles_are_recognised() {
        for (title, number) in [
            ("1", 1.0),
            ("01", 1.0),
            ("1.0", 1.0),
            ("12.5", 12.5),
            ("#3", 3.0),
            ("# 3", 3.0),
            ("Chapter 7", 7.0),
            ("chapter 7", 7.0),
            ("CHAPTER 07", 7.0),
            ("Chapter7", 7.0),
            ("Ch. 4", 4.0),
            ("ch 4", 4.0),
            ("Chap 4", 4.0),
            ("Chap. 4", 4.0),
            ("  Chapter 2  ", 2.0),
        ] {
            assert!(
                chapter_title(title, Some(number)).is_none(),
                "{title:?} restates {number}"
            );
        }
        assert!(chapter_title("", Some(1.0)).is_none());
        assert!(chapter_title("   ", None).is_none());
    }

    #[test]
    fn real_titles_are_never_dropped() {
        for (title, number) in [
            ("Interlude", Some(2.5)),
            ("Vol. 1", Some(1.0)),
            ("Volume 1", Some(1.0)),
            ("Night Harbor v01", Some(1.0)),
            ("Chapter 1: The Dock", Some(1.0)),
            ("Chapter 3", Some(2.5)), // a different number
            ("3", Some(2.0)),
            ("2001: A Space Odyssey", Some(2001.0)),
            ("1 2", Some(1.0)),
            ("1.2.3", Some(1.2)),
            ("Chapter", Some(1.0)),
            ("#", Some(1.0)),
            ("Chapter 1", None), // without a number there is nothing to restate
            ("1e3", Some(1000.0)),
        ] {
            assert_eq!(
                chapter_title(title, number).as_deref(),
                Some(title),
                "{title:?} stays"
            );
        }
    }

    #[test]
    fn titles_are_trimmed() {
        assert_eq!(
            chapter_title("  Interlude \n", Some(1.0)).as_deref(),
            Some("Interlude")
        );
    }

    #[test]
    fn only_readable_books_become_chapters() {
        // Vertical Rain: a CBZ, a text EPUB, an image EPUB, an unsupported file and a PDF.
        let books = books(fixtures::BOOKS_VERTICAL_RAIN);
        let names: Vec<&str> = books.iter().map(|book| book.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Vertical Rain v01",
                "Vertical Rain v02",
                "Vertical Rain v03",
                "Vertical Rain v04",
                "Vertical Rain v05"
            ]
        );

        let chapters = chapters(&books);
        let readable: Vec<&str> = chapters
            .iter()
            .map(|chapter| {
                books
                    .iter()
                    .find(|book| book.id == chapter.key)
                    .map(|book| book.name.as_str())
                    .unwrap()
            })
            .collect();
        // v02 is a text EPUB and v04 is unsupported.
        assert_eq!(
            readable,
            [
                "Vertical Rain v05",
                "Vertical Rain v03",
                "Vertical Rain v01"
            ]
        );
    }

    #[test]
    fn media_that_is_not_ready_is_not_a_chapter() {
        for status in [
            "UNKNOWN",
            "ERROR",
            "UNSUPPORTED",
            "OUTDATED",
            "",
            "SOMETHING_NEW",
        ] {
            let book = book_json(json!({
                "id": "B", "media": { "status": status, "mediaProfile": "DIVINA" }
            }));
            assert!(!is_readable(&book), "{status:?}");
        }
        let ready = book_json(
            json!({ "id": "B", "media": { "status": "READY", "mediaProfile": "DIVINA" } }),
        );
        assert!(is_readable(&ready));
    }

    #[test]
    fn a_text_epub_is_not_readable_but_an_image_epub_is() {
        let text = book_json(json!({
            "id": "B", "media": { "status": "READY", "mediaProfile": "EPUB", "epubDivinaCompatible": false }
        }));
        let images = book_json(json!({
            "id": "B", "media": { "status": "READY", "mediaProfile": "EPUB", "epubDivinaCompatible": true }
        }));
        assert!(!is_readable(&text));
        assert!(is_readable(&images));
    }

    #[test]
    fn pdfs_and_image_archives_are_readable() {
        for profile in ["PDF", "DIVINA"] {
            let book = book_json(json!({
                "id": "B", "media": { "status": "READY", "mediaProfile": profile, "epubDivinaCompatible": false }
            }));
            assert!(is_readable(&book), "{profile}");
        }
    }

    #[test]
    fn a_trashed_book_is_not_a_chapter_even_though_komga_still_calls_it_ready() {
        let books = books(fixtures::BOOKS_PAPER_LANTERNS);
        let trashed = books
            .iter()
            .find(|book| book.name == "Paper Lanterns v04")
            .unwrap();
        assert!(trashed.deleted);
        assert_eq!(trashed.media.status, "READY");
        assert!(!is_readable(trashed));

        let chapters = chapters(&books);
        assert_eq!(chapters.len(), 3);
        assert!(!chapters.iter().any(|chapter| chapter.key == trashed.id));
    }

    #[test]
    fn a_book_without_a_number_still_lists_after_the_numbered_ones() {
        let books = vec![
            book_json(
                json!({ "id": "A", "media": { "status": "READY" }, "metadata": { "numberSort": 1.0 } }),
            ),
            book_json(
                json!({ "id": "X", "media": { "status": "READY" }, "metadata": { "numberSort": null } }),
            ),
            book_json(
                json!({ "id": "B", "media": { "status": "READY" }, "metadata": { "numberSort": 2.0 } }),
            ),
        ];
        let chapters = chapters(&books);
        assert_eq!(keys(&chapters), ["B", "A", "X"]);
        assert_eq!(numbers(&chapters), [Some(2.0), Some(1.0), None]);
    }

    #[test]
    fn books_with_the_same_number_keep_a_stable_order() {
        let books = vec![
            book_json(
                json!({ "id": "first", "media": { "status": "READY" }, "metadata": { "numberSort": 1.0 } }),
            ),
            book_json(
                json!({ "id": "second", "media": { "status": "READY" }, "metadata": { "numberSort": 1.0 } }),
            ),
            book_json(
                json!({ "id": "third", "media": { "status": "READY" }, "metadata": { "numberSort": 2.0 } }),
            ),
        ];
        // Newest first: the later book of a tie is listed first, and the order is repeatable.
        assert_eq!(keys(&chapters(&books)), ["third", "second", "first"]);
        assert_eq!(keys(&chapters(&books)), ["third", "second", "first"]);
    }

    #[test]
    fn a_book_with_an_unreadable_date_has_no_date() {
        let book = book_json(json!({
            "id": "B", "created": "not a date", "media": { "status": "READY" },
            "metadata": { "releaseDate": "also not a date" }
        }));
        assert_eq!(chapter(&book).date_updated, None);
    }

    #[test]
    fn a_blank_release_date_falls_back_to_the_date_added() {
        let book = book_json(json!({
            "id": "B", "created": "2026-10-21T06:11:15Z", "media": { "status": "READY" },
            "metadata": { "releaseDate": "" }
        }));
        assert_eq!(chapter(&book).date_updated, Some(1_792_563_075.0));
    }

    #[test]
    fn pages_are_numbered_from_zero_and_addressed_from_one() {
        assert_eq!(
            page_url("B1", 1),
            "ito://com.atiinlabs.komga/api/v1/books/B1/pages/1"
        );
        let page = page(0, page_url("B1", 1));
        assert_eq!(page.index, 0);
        assert!(matches!(&page.content, PageContent::Url(url) if url.ends_with("/pages/1")));
        assert!(!page.has_description);
        assert!(page.description.is_none());
        assert!(page.headers.is_none());
    }
}
