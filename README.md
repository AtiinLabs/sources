# AtiinLabs Sources

First-party source plugins for [Ito](https://github.com/AtiinLabs/Ito): plugins that connect the app to reading services you run or subscribe to.

Each plugin is a Rust crate compiled to WebAssembly (`wasm32-unknown-unknown`) with the [`ito-rs`](https://github.com/AtiinLabs/ito-rs) SDK and packaged by [`ito-pkg`](https://github.com/AtiinLabs/ito-pkg).

## Add the repository to Ito

In Ito, open **Browse**, tap the globe button (**Repositories**), choose **Add Repository** and enter:

```
https://atiinlabs.github.io/sources
```

Then install the plugins you want from the repository.

## Plugins

| Plugin | Type | Service |
|---|---|---|
| [Komga](plugins/komga) | manga | [Komga](https://komga.org), a self-hosted comics and manga server |

### Komga

Reads the libraries of your own Komga server.

1. Install **Komga** from this repository.
2. Open it in Browse and tap the settings button.
3. Enter your **Server URL** (for example `https://komga.example.com`) and an **API key**. Create the key in Komga under *Account settings → API keys*. If you would rather sign in with an account, leave the API key empty and fill in **Username** and **Password**; an API key wins when both are set.

The API key and password are stored as secrets, and a change in the settings applies to the next request without reinstalling the plugin. The plugin needs Komga 1.19 or later (1.20 for API keys) and is tested against Komga 1.29.

What it does:

- **Home**: Keep reading, On deck, Recently added, Recently updated and All series, plus one section per library when the server has several. Empty sections are left out.
- **Browse and search**: every library, and full-text search across the series you can see.
- **Details**: title, summary, authors (writers and pencillers), genres and tags, status and reading direction.
- **Chapters**: a series' books, newest first. Books Komga has not analysed, books whose files are gone, and text EPUBs (which have no page images) are left out; image archives, PDFs and image-based EPUBs are readable.
- **Covers and pages** use stable, plugin-owned `ito://` locators and your current credentials, so a server-address change needs no plugin reinstall. Hosts supporting native image requests fetch bytes outside WASM using transient URL/header descriptors and the configured server origin; older hosts retain the legacy byte handler. Credentials are not stored in page metadata.

Not included yet: syncing your reading progress back to Komga.

## Build locally

You need Rust with the WebAssembly target and `ito-pkg`, pinned to the revision CI uses:

```sh
rustup target add wasm32-unknown-unknown
cargo install --git https://github.com/AtiinLabs/ito-pkg --rev cde8969873d09db50fe33f1e82754b8e7fcd946c --locked
```

Then, in a plugin's directory:

```sh
cd plugins/komga
cargo test          # unit tests; they run on your machine and need neither Ito nor a server
ito-pkg pack        # builds the wasm and writes komga.ito
ito-pkg verify komga.ito
```

To try a build in Ito, serve it as a local repository:

```sh
mkdir -p /tmp/ito-repo/packages
cp plugins/komga/komga.ito /tmp/ito-repo/packages/
ito-pkg repo --input /tmp/ito-repo/packages --output /tmp/ito-repo/public --url http://localhost:8080 --name "AtiinLabs (local)"
ito-pkg serve --path /tmp/ito-repo/public --port 8080
```

Add `http://localhost:8080` as a repository in Ito. Use `localhost` in the iOS Simulator and your computer's LAN address on a device; `--url` must match the address you add.

## Publishing

Pushing to `main` runs the tests, packs every plugin under `plugins/`, builds the repository index and deploys it to GitHub Pages (the `gh-pages` branch) with [`.github/workflows/build.yml`](.github/workflows/build.yml). Pull requests run the same steps without deploying.

When you change a plugin, bump `version` in its `manifest.json` and in its `Cargo.toml`: Ito only offers an update when the version changes.

## License

[MIT](LICENSE)
