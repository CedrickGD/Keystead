// RFC 6238 TOTP (and RFC 4226 HOTP) implemented with WebCrypto.
// Used by the mock backend; the real app computes codes in keystead-core.

import type { TotpCode } from "../types";

export type TotpAlgorithm = "SHA-1" | "SHA-256" | "SHA-512";

export interface TotpParams {
  secret: Uint8Array<ArrayBuffer>;
  algorithm: TotpAlgorithm;
  digits: number;
  period: number;
}

const BASE32_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/** RFC 4648 base32 decoding (case-insensitive, ignores spaces, dashes and padding). */
export function base32Decode(input: string): Uint8Array<ArrayBuffer> {
  const clean = input.replace(/[\s-]/g, "").replace(/=+$/, "").toUpperCase();
  if (!clean) throw new Error("empty secret");
  const out: number[] = [];
  let buffer = 0;
  let bits = 0;
  for (const ch of clean) {
    const value = BASE32_ALPHABET.indexOf(ch);
    if (value < 0) throw new Error("invalid base32 character");
    buffer = (buffer << 5) | value;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      out.push((buffer >> bits) & 0xff);
    }
  }
  return new Uint8Array(out);
}

function parseAlgorithm(raw: string | null): TotpAlgorithm {
  switch ((raw ?? "SHA1").toUpperCase().replace("-", "")) {
    case "SHA1":
      return "SHA-1";
    case "SHA256":
      return "SHA-256";
    case "SHA512":
      return "SHA-512";
    default:
      throw new Error("unsupported algorithm");
  }
}

/** Parses a bare base32 secret or an `otpauth://totp/...` URI. */
export function parseTotpSeed(seed: string): TotpParams {
  const trimmed = seed.trim();
  if (/^otpauth:\/\//i.test(trimmed)) {
    const url = new URL(trimmed);
    if (url.host.toLowerCase() !== "totp") throw new Error("only TOTP is supported");
    const secret = url.searchParams.get("secret");
    if (!secret) throw new Error("missing secret");
    const digits = Number(url.searchParams.get("digits") ?? "6");
    const period = Number(url.searchParams.get("period") ?? "30");
    if (!Number.isInteger(digits) || digits < 6 || digits > 8) throw new Error("invalid digits");
    if (!Number.isInteger(period) || period < 1 || period > 300) throw new Error("invalid period");
    return { secret: base32Decode(secret), algorithm: parseAlgorithm(url.searchParams.get("algorithm")), digits, period };
  }
  return { secret: base32Decode(trimmed), algorithm: "SHA-1", digits: 6, period: 30 };
}

/** HOTP value for the given counter (RFC 4226, dynamic truncation). */
export async function hotp(params: Omit<TotpParams, "period">, counter: number): Promise<string> {
  const counterBytes = new Uint8Array(8);
  let value = counter;
  for (let i = 7; i >= 0; i--) {
    counterBytes[i] = value & 0xff;
    value = Math.floor(value / 256);
  }
  const key = await crypto.subtle.importKey(
    "raw",
    params.secret,
    { name: "HMAC", hash: params.algorithm },
    false,
    ["sign"],
  );
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, counterBytes));
  const offset = (mac[mac.length - 1] ?? 0) & 0x0f;
  const binary =
    (((mac[offset] ?? 0) & 0x7f) << 24) |
    (((mac[offset + 1] ?? 0) & 0xff) << 16) |
    (((mac[offset + 2] ?? 0) & 0xff) << 8) |
    ((mac[offset + 3] ?? 0) & 0xff);
  const code = binary % 10 ** params.digits;
  return code.toString().padStart(params.digits, "0");
}

export async function totpAt(seed: string, unixSeconds: number): Promise<TotpCode> {
  const params = parseTotpSeed(seed);
  const counter = Math.floor(unixSeconds / params.period);
  const code = await hotp(params, counter);
  return { code, period: params.period, remaining: params.period - (Math.floor(unixSeconds) % params.period) };
}

export function totpNow(seed: string): Promise<TotpCode> {
  return totpAt(seed, Date.now() / 1000);
}
