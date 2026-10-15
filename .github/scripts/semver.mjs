// Semantic versions (semver.org 2.0, without build metadata) as the in-app
// updater orders them: 2.0.0-beta.4 < 2.0.0-beta.10 < 2.0.0 < 2.0.1-beta.1.
//
//   node .github/scripts/semver.mjs newer <a> <b>   exit 0 if a > b, else 1
//   node .github/scripts/semver.mjs valid <v>       exit 0 if v is a valid version

import { pathToFileURL } from "node:url";

const IDENT = String.raw`(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)`;
const SEMVER = new RegExp(String.raw`^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(${IDENT}(?:\.${IDENT})*))?$`);

/** { major, minor, patch, pre: string[] } or null. */
export function parseSemver(value) {
  if (typeof value !== "string") return null;
  const m = SEMVER.exec(value.trim());
  if (!m) return null;
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]), pre: m[4] ? m[4].split(".") : [] };
}

/** -1 / 0 / 1; throws on invalid versions. */
export function compareSemver(a, b) {
  const x = parseSemver(a);
  const y = parseSemver(b);
  if (!x || !y) throw new Error(`invalid version: ${!x ? a : b}`);
  for (const key of ["major", "minor", "patch"]) {
    if (x[key] !== y[key]) return x[key] < y[key] ? -1 : 1;
  }
  // A pre-release is lower than the release itself.
  if (!x.pre.length || !y.pre.length) return x.pre.length === y.pre.length ? 0 : x.pre.length ? -1 : 1;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i += 1) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    if (p === q) continue;
    const pn = /^\d+$/.test(p);
    const qn = /^\d+$/.test(q);
    if (pn && qn) return Number(p) < Number(q) ? -1 : 1;
    if (pn !== qn) return pn ? -1 : 1; // numeric identifiers sort first
    return p < q ? -1 : 1;
  }
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [command, a, b] = process.argv.slice(2);
  try {
    if (command === "newer") process.exit(compareSemver(a, b) > 0 ? 0 : 1);
    if (command === "valid") process.exit(parseSemver(a) ? 0 : 1);
    console.error("usage: semver.mjs newer <a> <b> | valid <v>");
    process.exit(2);
  } catch (err) {
    console.error(err.message);
    process.exit(2);
  }
}
