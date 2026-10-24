//! Komga for Ito: read a self-hosted [Komga](https://komga.org) server's libraries in the app.
//!
//! The plugin has no configuration of its own. The user enters the server URL and an API key
//! (or a username and password) in the plugin's settings, and every call reads them afresh
//! from the host. The modules, from the outside in:
//!
//! - [`service`]: one function per `MangaProvider` method.
//! - [`api`]: Komga's REST endpoints, authenticated from the settings.
//! - [`mapping`]: Komga's series and books as Ito's titles, chapters and pages.
//! - [`dto`]: the JSON Komga sends.
//! - [`settings`], [`error`], [`time`]: the settings and auth, the sentences shown to the
//!   user, and timestamp parsing.
//! - [`host`]: the one seam to the Ito runtime, which lets the tests run without it.

use ito_rs::Result;
#[cfg(target_arch = "wasm32")]
use ito_rs::export_manga_plugin;
use ito_rs::models::manga::{Chapter, Manga, PageResult};
use ito_rs::models::{
    FilterItem, HomeLayout, Listing, Page, PluginImageRequest, SettingIssue, SettingsAction,
    SettingsSchema,
};
use ito_rs::provider::MangaProvider;

pub mod api;
pub mod dto;
pub mod error;
pub mod host;
pub mod mapping;
pub mod service;
pub mod settings;
pub mod time;

#[cfg(test)]
mod testing;

use host::ItoHost;

/// The settings action that checks the configured server without changing anything.
const TEST_CONNECTION: &str = service::TEST_CONNECTION;

pub struct Komga;

impl MangaProvider for Komga {
    fn get_home() -> Result<HomeLayout> {
        service::home(&ItoHost)
    }

    fn get_settings() -> Option<SettingsSchema> {
        Some(settings::schema())
    }

    fn get_settings_actions() -> Result<Vec<SettingsAction>> {
        Ok(vec![SettingsAction {
            id: TEST_CONNECTION.to_owned(),
            title: "Test connection".to_owned(),
            summary: Some(
                "Checks the server URL and sign-in shown here. Nothing is saved.".to_owned(),
            ),
        }])
    }

    fn run_settings_action(action_id: &str) -> Result<String> {
        service::run_settings_action(&ItoHost, action_id)
    }

    fn validate_settings() -> Result<Vec<SettingIssue>> {
        settings::validate(&ItoHost)
    }

    fn get_manga_list(listing: Listing, page: i32) -> Result<PageResult> {
        service::manga_list(&ItoHost, &listing, page)
    }

    fn get_search_manga_list(
        query: &str,
        page: i32,
        _filters: Vec<FilterItem>,
    ) -> Result<PageResult> {
        service::search(&ItoHost, query, page)
    }

    fn get_manga_update(manga: Manga, needs_details: bool, needs_chapters: bool) -> Result<Manga> {
        service::manga_update(&ItoHost, manga, needs_details, needs_chapters)
    }

    fn get_page_list(_manga: Manga, chapter: Chapter) -> Result<Vec<Page>> {
        service::page_list(&ItoHost, &chapter)
    }

    fn mark_chapter_read(_media_key: &str, chapter_key: &str) -> Result<()> {
        service::mark_chapter_read(&ItoHost, chapter_key)
    }

    fn get_image_origins() -> Result<Vec<String>> {
        service::image_origins(&ItoHost)
    }

    fn get_image_request(path: &str) -> Result<PluginImageRequest> {
        service::image_request(&ItoHost, path)
    }

    fn handle_image(url: &str) -> Result<Vec<u8>> {
        service::image(&ItoHost, url)
    }
}

// The exported functions call host imports that only exist inside Ito's runtime, so they are
// built for wasm only; unit tests run on the host and never link them.
#[cfg(target_arch = "wasm32")]
export_manga_plugin!(Komga);

#[cfg(test)]
mod tests {
    use super::*;
    use ito_rs::models::Setting;
    use serde_json::Value;

    fn manifest() -> Value {
        serde_json::from_str(include_str!("../manifest.json")).expect("manifest.json is valid JSON")
    }

    #[test]
    fn the_plugin_offers_the_settings_screen_through_the_provider() {
        let schema = Komga::get_settings().expect("the plugin has settings");
        assert_eq!(schema.settings.len(), 4);
        assert!(matches!(&schema.settings[0], Setting::Text { id, .. } if id == "server_url"));
    }

    #[test]
    fn the_settings_screen_offers_only_the_connection_check() {
        let ids: Vec<String> = Komga::get_settings_actions()
            .unwrap()
            .into_iter()
            .map(|action| action.id)
            .collect();
        assert_eq!(ids, [TEST_CONNECTION]);
    }

    #[test]
    fn covers_are_routed_by_the_id_ito_knows_the_plugin_by() {
        // Ito sends `ito://<id>/...` image loads to the plugin whose manifest has that id.
        assert_eq!(manifest()["id"], mapping::PLUGIN_ID);
        assert!(
            mapping::cover_url("S")
                .starts_with(&format!("ito://{}/", manifest()["id"].as_str().unwrap()))
        );
    }

    #[test]
    fn the_manifest_describes_a_manga_plugin_for_the_app_that_has_secret_settings() {
        let manifest = manifest();
        assert_eq!(manifest["name"], "Komga");
        assert_eq!(manifest["type"], "manga");
        assert_eq!(manifest["min_app_version"], "1.2.0");
        assert_eq!(manifest["author"], "AtiinLabs");
        assert_eq!(
            manifest["sourceUrl"],
            "https://github.com/AtiinLabs/sources"
        );
        assert_eq!(manifest["url"], "https://komga.org");
        assert_eq!(manifest["language"], "en");
        assert_eq!(manifest["languages"], serde_json::json!(["en"]));
        assert_eq!(manifest["contentRating"], 0);
        assert_eq!(manifest["nsfw"], 0);
    }

    #[test]
    fn the_manifest_and_the_crate_carry_the_same_version() {
        assert_eq!(manifest()["version"], env!("CARGO_PKG_VERSION"));
    }
}
