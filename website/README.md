# Spotify for Island website

Standalone Astro site with a dark/green palette, deployed to [island-spotify.pages.dev](https://island-spotify.pages.dev). Requires Node 22.12+ and npm.

Run from this directory:

```sh
npm ci
npm run dev
npm run build
npx playwright install chromium
npm test
```

To deploy to the existing Cloudflare Pages project, sign in with `npx wrangler login`, then run `npm run deploy`. Deployment builds and uploads `dist` to the production `main` branch. For a preview, use `npm run build` followed by `npx wrangler pages deploy dist --branch preview`. No private Island scripts or website imports are required.

`plugin.json` follows the public [Plugin Website v1 standard](https://github.com/Sullivansome/island-plugin-sdk/blob/main/docs/plugin-website.md). It is published at `/.well-known/island-plugin.json` and feeds the page content. Changes to source metadata do not update the curated Market snapshot automatically: submit a reviewed change to the SDK repository's `registry/index/v1.json`. Website deployment does not publish plugin binaries or grant permissions.

For custom domains, update `plugin.json` homepage/documentation/preview URLs and `SITE_URL` together. The discovery origin must match the homepage and preview origins.

The site is MIT licensed, uses self-hosted Manrope/Geist fonts and original sample preview content, and makes no Spotify API calls. Third-party font licenses and Spotify trademarks remain with their owners.
