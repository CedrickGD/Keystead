// Website icon keys, computed exactly like the Rust core
// (`keystead_core::icons::site_host` / `icon_host`): the lower-case host of
// a login's first http(s) URI (scheme-less URIs count as https://), without
// a trailing dot and a leading "www.", port ignored. The backend's
// `get_icons` answers { [host]: "data:image/png;base64,…" }.

import type { ItemType, LoginUri } from "./types";

/** The icon host of one stored URI, or null (other schemes, unparsable). */
export function siteHost(uri: string): string | null {
  const raw = uri.trim();
  if (!raw) return null;
  let url: URL;
  try {
    url = new URL(raw.includes("://") ? raw : `https://${raw}`);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  let host = url.hostname.toLowerCase().replace(/\.+$/, "");
  if (host.startsWith("www.")) host = host.slice(4);
  return host || null;
}

/** The icon host of an item (list entry or full item): logins only, the first URI that has one. */
export function iconHost(item: { type: ItemType; login: { uris: LoginUri[] } | null }): string | null {
  if (item.type !== "login") return null;
  for (const u of item.login?.uris ?? []) {
    const host = siteHost(u.uri);
    if (host) return host;
  }
  return null;
}

/** Only PNG data URLs are shown (what the backend stores). */
export function isIconDataUrl(value: unknown): value is string {
  return typeof value === "string" && value.startsWith("data:image/png;base64,");
}
