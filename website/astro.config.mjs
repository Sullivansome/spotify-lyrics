import { defineConfig } from 'astro/config';
import { existsSync } from 'node:fs';
import { loadEnvFile } from 'node:process';
const env = new URL('.env', import.meta.url);
if (existsSync(env)) loadEnvFile(env);
export default defineConfig({
  site: process.env.SITE_URL || 'https://island-spotify.pages.dev',
  output: 'static', trailingSlash: 'always', devToolbar: { enabled: false },
});
