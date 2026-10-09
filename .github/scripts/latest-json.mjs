// Writes the updater manifest (`latest.json`) of a signed Windows build, in
// the static format tauri-plugin-updater reads:
//   { version, notes, pub_date (RFC 3339), platforms: { "windows-x86_64": { signature, url } } }
//
//   node .github/scripts/latest-json.mjs --version <v> --signature-file <setup.exe.sig>
//        --url <download URL of the setup> --out <latest.json>
//
// `signature` is the content of the .sig file `tauri build` writes next to
// the NSIS setup (base64 minisign signature, bound to the version). The URL
// must name the asset exactly as it is uploaded. The notes are the first
// line of $RELEASE_NOTES (the commit message) without CI markers.

import fs from "node:fs";
import { pathToFileURL } from "node:url";
import { parseSemver } from "./semver.mjs";

/** First line of a commit message without `[release]`, `[skip ci]` & co. */
export function releaseNotes(message, version) {
  const first = String(message ?? "")
    .split(/\r?\n/)[0]
    .replace(/\[(release|skip ci|ci skip|no ci)\]/gi, "")
    .replace(/\s+/g, " ")
    .trim();
  return first || `Keystead ${version}`;
}

/** RFC 3339 without fractional seconds, e.g. 2026-10-09T17:00:00Z. */
export function rfc3339(date) {
  return date.toISOString().replace(/\.\d{3}Z$/, "Z");
}

export function latestManifest({ version, signature, url, notes, date = new Date() }) {
  if (!parseSemver(version)) throw new Error(`invalid version ${version}`);
  const sig = String(signature ?? "").trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(sig)) throw new Error("signature is not base64 (empty .sig file?)");
  const parsed = new URL(url);
  if (parsed.protocol !== "https:") throw new Error(`download URL must be https: ${url}`);
  return {
    version,
    notes,
    pub_date: rfc3339(date),
    platforms: {
      "windows-x86_64": { signature: sig, url: parsed.toString() },
    },
  };
}

function arg(argv, name) {
  const i = argv.indexOf(name);
  if (i < 0 || !argv[i + 1]) throw new Error(`missing ${name}`);
  return argv[i + 1];
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const argv = process.argv.slice(2);
    const version = arg(argv, "--version");
    const manifest = latestManifest({
      version,
      signature: fs.readFileSync(arg(argv, "--signature-file"), "utf8"),
      url: arg(argv, "--url"),
      notes: releaseNotes(process.env.RELEASE_NOTES, version),
    });
    fs.writeFileSync(arg(argv, "--out"), `${JSON.stringify(manifest, null, 2)}\n`);
    console.log(JSON.stringify(manifest, null, 2));
  } catch (err) {
    console.error(`::error::${err.message}`);
    process.exit(1);
  }
}
