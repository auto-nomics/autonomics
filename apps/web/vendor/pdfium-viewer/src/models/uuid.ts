/*─────────────────────────────────────────────────────────────────────────────
  🆔  UUID helpers  – no external deps
──────────────────────────────────────────────────────────────────────────────*/
/**
 * Canonical RFC‑4122 **version‑4** UUID format:
 *   8-4-4-4-12 hexadecimal digits with
 *   – the version nibble fixed to **4**
 *   – the variant nibble in the range **8‑b** (binary 10xx)
 */
const V4_REGEX = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

/**
 * Check whether the supplied string is a **valid RFC‑4122 v4 UUID**.
 * Works in every runtime (browser / Node / Deno) because it is just
 * string validation – no crypto required.
 */
export function isUuidV4(str: string): boolean {
  return V4_REGEX.test(str);
}

/*────────────────────────────  rng helpers  ───────────────────────────────*/
function getRandomBytes(len: number): Uint8Array {
  // Modern browsers & Node ≥ 15 expose Web Crypto
  if (typeof globalThis.crypto?.getRandomValues === 'function') {
    return globalThis.crypto.getRandomValues(new Uint8Array(len));
  }

  // Fallback for old Node versions (< 15) – use node:crypto.randomBytes
  if (typeof require === 'function') {
    try {
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const { randomBytes } = require('crypto') as typeof import('crypto');
      return randomBytes(len);
    } catch {
      /* ignore – continue to final fallback */
    }
  }

  // ⚠️  LAST‑RESORT non‑cryptographic RNG – should never be used in prod
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i++) bytes[i] = Math.floor(Math.random() * 256);
  return bytes;
}

/*────────────────────────────  main API  ───────────────────────────────────*/
/**
 * Generate a **version‑4 UUID** (random).
 *
 * • Uses the native `crypto.randomUUID()` when available (all modern
 *   browsers, Deno, Node ≥ 16.9).
 * • Falls back to a tiny, standards‑compliant implementation that uses
 *   `crypto.getRandomValues` / `crypto.randomBytes` for entropy.
 *
 * @example
 * ```ts
 * import { uuidV4 } from ".";
 * const id = uuidV4();
 * // → "36b8f84d-df4e-4d49-b662-bcde71a8764f"
 * ```
 */
export function uuidV4(): string {
  if (typeof globalThis.crypto?.randomUUID === 'function') {
    return globalThis.crypto.randomUUID();
  }

  const bytes = getRandomBytes(16);

  // Per RFC 4122 §4.4: set version (0100) and variant (10xx)
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10xx

  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');

  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
