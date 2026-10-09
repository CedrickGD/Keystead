// URL helpers shared by the service worker and the popup.

/** Parses an http(s) URL; null for anything else (chrome://, file:, about:blank, data: …). */
export function parseWebUrl(url) {
  if (typeof url !== "string" || !url) return null;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed : null;
  } catch {
    return null;
  }
}

function isIpOrLocal(host) {
  return host === "localhost" || /^[\d.]+$/.test(host) || host.includes(":") || !host.includes(".");
}

// Second-level labels that are public suffixes under many country TLDs
// (example.co.uk, example.com.au …). A small approximation of the Public
// Suffix List; it only decides which pages count as "the same site" for a
// pending save prompt, never which logins match (the app does that).
const SECOND_LEVEL = new Set(["co", "com", "org", "net", "gov", "edu", "ac", "or", "ne", "go", "gv", "ltd", "plc", "nom"]);

/** Approximate registrable domain ("accounts.example.co.uk" → "example.co.uk"). */
export function siteKey(hostname) {
  const host = String(hostname || "").toLowerCase().replace(/\.$/, "");
  if (isIpOrLocal(host)) return host;
  const labels = host.split(".");
  if (labels.length <= 2) return host;
  const tld = labels[labels.length - 1];
  const second = labels[labels.length - 2];
  const take = tld.length === 2 && SECOND_LEVEL.has(second) ? 3 : 2;
  return labels.slice(-take).join(".");
}

/** Host for display and as default login name ("www.example.com" → "example.com"). */
export function displayHost(hostname) {
  return String(hostname || "").toLowerCase().replace(/^www\./, "");
}
