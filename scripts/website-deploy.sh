#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/website"
npm ci --no-audit --no-fund
npm run build
exec ./node_modules/.bin/wrangler pages deploy dist --branch main "$@"
