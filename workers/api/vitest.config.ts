import { defineWorkersConfig } from "@cloudflare/vitest-pool-workers/config";

export default defineWorkersConfig({
  test: {
    poolOptions: {
      workers: {
        wrangler: { configPath: "./wrangler.toml" },
        miniflare: {
          bindings: {
            ENVIRONMENT: "test",
            JWT_SECRET: "test-jwt-secret-key-for-unit-tests",
            GOOGLE_CLIENT_ID: "test-google-client-id",
            GOOGLE_CLIENT_SECRET: "test-google-client-secret",
            GITHUB_CLIENT_ID: "test-github-client-id",
            GITHUB_CLIENT_SECRET: "test-github-client-secret",
            STRIPE_SECRET_KEY: "sk_test_fake",
            STRIPE_WEBHOOK_SECRET: "whsec_test_fake",
            STRIPE_PRICE_ID: "price_test_fake",
            SLACK_BOT_TOKEN: "xoxb-test-fake",
            GITHUB_APP_TOKEN: "ghp_test_fake",
            GRAPH_PROXY_URL: "https://localhost:9999",
          },
          kvNamespaces: ["KV"],
          d1Databases: ["DB"],
        },
      },
    },
  },
});
