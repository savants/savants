import type { JwtPayload } from "../lib/types";

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

function base64UrlEncode(data: ArrayBuffer | Uint8Array): string {
  const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
  let binary = "";
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function base64UrlDecode(str: string): Uint8Array {
  let base64 = str.replace(/-/g, "+").replace(/_/g, "/");
  const pad = base64.length % 4;
  if (pad === 2) base64 += "==";
  else if (pad === 3) base64 += "=";
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

async function getSigningKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "raw",
    ENCODER.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign", "verify"]
  );
}

export async function signJwt(
  payload: Omit<JwtPayload, "iat" | "exp">,
  secret: string,
  expiresInSeconds: number = 86400 * 30
): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const fullPayload: JwtPayload = {
    ...payload,
    iat: now,
    exp: now + expiresInSeconds,
  };

  const header = { alg: "HS256", typ: "JWT" };
  const headerB64 = base64UrlEncode(ENCODER.encode(JSON.stringify(header)));
  const payloadB64 = base64UrlEncode(ENCODER.encode(JSON.stringify(fullPayload)));
  const signingInput = `${headerB64}.${payloadB64}`;

  const key = await getSigningKey(secret);
  const signature = await crypto.subtle.sign("HMAC", key, ENCODER.encode(signingInput));
  const signatureB64 = base64UrlEncode(signature);

  return `${signingInput}.${signatureB64}`;
}

export async function verifyJwt(
  token: string,
  secret: string
): Promise<JwtPayload | null> {
  const parts = token.split(".");
  if (parts.length !== 3) return null;

  const [headerB64, payloadB64, signatureB64] = parts;
  const signingInput = `${headerB64}.${payloadB64}`;

  const key = await getSigningKey(secret);
  const signatureBytes = base64UrlDecode(signatureB64);
  const valid = await crypto.subtle.verify("HMAC", key, signatureBytes, ENCODER.encode(signingInput));

  if (!valid) return null;

  const payloadJson = DECODER.decode(base64UrlDecode(payloadB64));
  const payload: JwtPayload = JSON.parse(payloadJson);

  const now = Math.floor(Date.now() / 1000);
  if (payload.exp < now) return null;

  return payload;
}
