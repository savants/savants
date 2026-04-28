import { SELF } from "cloudflare:test";

/**
 * JWT helper: signs a minimal HS256 JWT for test auth.
 * Mirrors the logic in src/auth/jwt.ts using the test secret.
 */
const ENCODER = new TextEncoder();

function base64UrlEncode(data: ArrayBuffer | Uint8Array): string {
  const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
  let binary = "";
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export const TEST_JWT_SECRET = "test-jwt-secret-key-for-unit-tests";
export const TEST_USER_ID = "usr_test_00000000";
export const TEST_ORG_ID = "org_test_00000000";
export const TEST_EMAIL = "test@example.com";

export async function generateTestJwt(overrides?: {
  sub?: string;
  org?: string;
  email?: string;
  exp?: number;
}): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const payload = {
    sub: overrides?.sub ?? TEST_USER_ID,
    org: overrides?.org ?? TEST_ORG_ID,
    email: overrides?.email ?? TEST_EMAIL,
    iat: now,
    exp: overrides?.exp ?? now + 3600,
  };

  const header = { alg: "HS256", typ: "JWT" };
  const headerB64 = base64UrlEncode(ENCODER.encode(JSON.stringify(header)));
  const payloadB64 = base64UrlEncode(ENCODER.encode(JSON.stringify(payload)));
  const signingInput = `${headerB64}.${payloadB64}`;

  const key = await crypto.subtle.importKey(
    "raw",
    ENCODER.encode(TEST_JWT_SECRET),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  const signature = await crypto.subtle.sign("HMAC", key, ENCODER.encode(signingInput));
  const signatureB64 = base64UrlEncode(signature);

  return `${signingInput}.${signatureB64}`;
}

/**
 * Convenience: fetch against the worker under test via SELF binding.
 */
export function workerFetch(path: string, init?: RequestInit): Promise<Response> {
  return SELF.fetch(`https://api.savants.cloud${path}`, init);
}
