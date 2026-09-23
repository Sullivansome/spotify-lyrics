# Spotify Lyrics for Island

Spotify Desktop playback and synchronized lyrics in an Island plugin. Uses macOS Automation to read and control your local Spotify session, with lyrics from LRCLIB and NetEase. No extra Spotify OAuth login is required. Runtime integration supports macOS; availability of lyrics depends on external providers.

## Build and install

Requires stable Rust with edition 2024 support. The public API and SDK are pinned Git dependencies; this repository has no private source dependencies.

```sh
cargo build --locked
cargo test --locked
./scripts/package.sh
island-cli plugin install ./dist/com.sullivansome.spotify
```

An installed compatible Island app and its CLI are required to display the plugin. They are proprietary and not included here. Start Spotify and allow macOS Automation when prompted. The package script only builds local files; it does not publish or install anything.

## Previews and website

```sh
cargo run --locked -- --export-market-previews market
cd website
npm ci
npm run dev
```

The standalone [website](https://island-spotify.pages.dev) and its Astro source are included. See [website/README.md](website/README.md) for Cloudflare Pages deployment and the standard Market metadata endpoint. Sample previews contain original example text.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

The live NetEase API test is ignored by default; automated tests do not prove a live Spotify session or macOS Automation approval.

## License and provenance

MIT; see [LICENSE](LICENSE). Extracted from the Island workspace on 2026-09-23, without proprietary app code or private history. Uses the public [Island Plugin SDK](https://github.com/Sullivansome/island-plugin-sdk). Integration patterns were inspired by [Lyrisland](https://github.com/EurFelux/Lyrisland); no reference checkout is bundled. This independent plugin is not affiliated with Spotify, whose name and trademarks belong to their owner.
