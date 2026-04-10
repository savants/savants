#!/bin/sh
# Deploy savants.dev to Cloudflare Pages
# Run: CLOUDFLARE_API_TOKEN=xxx ./deploy.sh

set -e

cd "$(dirname "$0")"

# Build
bunx astro build

# Deploy
npx wrangler pages deploy dist \
  --project-name savants-dev \
  --branch main \
  --commit-dirty=true

echo ""
echo "Deployed to Cloudflare Pages."
echo "Set custom domain: savants.dev → savants-dev.pages.dev in Cloudflare dashboard"
