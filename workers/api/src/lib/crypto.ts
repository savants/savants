const ENCODER = new TextEncoder();

export async function hashKey(raw: string): Promise<string> {
  const data = ENCODER.encode(raw);
  const digest = await crypto.subtle.digest("SHA-256", data);
  return bufToHex(digest);
}

export async function verifyKeyHash(
  raw: string,
  storedHash: string
): Promise<boolean> {
  const computed = await hashKey(raw);
  return timingSafeEqual(computed, storedHash);
}

export async function hmacSign(
  key: string,
  data: string
): Promise<ArrayBuffer> {
  const cryptoKey = await crypto.subtle.importKey(
    "raw",
    ENCODER.encode(key),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"]
  );
  return crypto.subtle.sign("HMAC", cryptoKey, ENCODER.encode(data));
}

export async function hmacVerify(
  key: string,
  data: string,
  signature: ArrayBuffer
): Promise<boolean> {
  const cryptoKey = await crypto.subtle.importKey(
    "raw",
    ENCODER.encode(key),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["verify"]
  );
  return crypto.subtle.verify("HMAC", cryptoKey, signature, ENCODER.encode(data));
}

export function bufToHex(buf: ArrayBuffer): string {
  return Array.from(new Uint8Array(buf))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

export function hexToBuf(hex: string): ArrayBuffer {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = parseInt(hex.substring(i, i + 2), 16);
  }
  return bytes.buffer;
}

function timingSafeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let result = 0;
  for (let i = 0; i < a.length; i++) {
    result |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return result === 0;
}

export function generateApiKey(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  const raw = bufToHex(bytes.buffer);
  return `sk_live_${raw}`;
}

export function generateAgentKey(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  const raw = bufToHex(bytes.buffer);
  return `svt_agent_${raw}`;
}

export function generateUserCode(): string {
  const chars = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  let code = "";
  for (let i = 0; i < 8; i++) {
    code += chars[bytes[i] % chars.length];
  }
  return code;
}

export function extractKeyPrefix(key: string): string {
  if (key.startsWith("sk_live_")) {
    return key.substring(0, 20);
  }
  if (key.startsWith("svt_agent_")) {
    return key.substring(0, 22);
  }
  return key.substring(0, 12);
}
