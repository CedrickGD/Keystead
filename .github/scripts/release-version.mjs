// Version of a CI build, written into the app and the browser extension
// before building (see "Releases & in-app updates" in docs/ARCHITECTURE.md).
//
//   node .github/scripts/release-version.mjs [--write] [--root <repo dir>]
//
// Environment (set by GitHub Actions): GITHUB_REF, GITHUB_REF_NAME,
// GITHUB_RUN_NUMBER; outputs `version` and `extension_version` go to
// $GITHUB_OUTPUT when set.
//
// * Tag `v<semver>` (e.g. v2.0.0, v2.0.1-rc.1): exactly that version.
// * Any other build: `<version in tauri.conf.json>-beta.<run number>`, e.g.
//   2.0.0-beta.57 – every beta gets its own version, so the updater (semver)
//   orders 2.0.0-beta.4 < 2.0.0-beta.5 < 2.0.0.
// * Extension (Chrome wants 1–4 integers ≤ 65535):
//   `<major>.<minor>.<patch>.<run number>`, also for tags – run numbers only
//   grow, so a stable release still counts as newer than the betas before it
//   and installed extensions reload into it.
//
// --write sets `version` in apps/desktop/src-tauri/tauri.conf.json (app,
// NSIS installer, updater) and extension/chrome/manifest.json.

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { parseSemver } from "./semver.mjs";

const TAURI_CONF = "apps/desktop/src-tauri/tauri.conf.json";
const EXT_MANIFEST = "extension/chrome/manifest.json";

/** The app version of a build. */
export function appVersion({ ref = "", refName = "", runNumber, baseVersion }) {
  if (ref.startsWith("refs/tags/v")) {
    const version = refName.replace(/^v/, "");
    if (!parseSemver(version)) throw new Error(`tag ${refName} is not v<semver>`);
    return version;
  }
  const base = parseSemver(baseVersion);
  if (!base || base.pre.length) throw new Error(`tauri.conf.json version must be x.y.z, is ${baseVersion}`);
  const run = Number(runNumber);
  if (!Number.isInteger(run) || run < 1) throw new Error(`invalid run number ${runNumber}`);
  return `${base.major}.${base.minor}.${base.patch}-beta.${run}`;
}

/** The extension manifest version of a build. */
export function extensionVersion(version, runNumber) {
  const v = parseSemver(version);
  if (!v) throw new Error(`invalid version ${version}`);
  const run = Number(runNumber);
  if (!Number.isInteger(run) || run < 0 || run > 65535) throw new Error(`run number ${runNumber} out of range`);
  for (const n of [v.major, v.minor, v.patch]) if (n > 65535) throw new Error(`version ${version} too large`);
  return `${v.major}.${v.minor}.${v.patch}.${run}`;
}

/** Sets the top-level "version" of a JSON file, keeping the rest. */
function setJsonVersion(file, version) {
  const json = JSON.parse(fs.readFileSync(file, "utf8"));
  json.version = version;
  fs.writeFileSync(file, `${JSON.stringify(json, null, 2)}\n`);
}

function main(argv) {
  const write = argv.includes("--write");
  const rootIdx = argv.indexOf("--root");
  const root = rootIdx >= 0 ? argv[rootIdx + 1] : process.cwd();
  const conf = JSON.parse(fs.readFileSync(path.join(root, TAURI_CONF), "utf8"));
  const runNumber = process.env.GITHUB_RUN_NUMBER;
  const version = appVersion({
    ref: process.env.GITHUB_REF,
    refName: process.env.GITHUB_REF_NAME,
    runNumber,
    baseVersion: conf.version,
  });
  const extVersion = extensionVersion(version, runNumber);
  if (write) {
    setJsonVersion(path.join(root, TAURI_CONF), version);
    setJsonVersion(path.join(root, EXT_MANIFEST), extVersion);
  }
  console.log(`version=${version}`);
  console.log(`extension_version=${extVersion}`);
  if (process.env.GITHUB_OUTPUT) {
    fs.appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\nextension_version=${extVersion}\n`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (err) {
    console.error(`::error::${err.message}`);
    process.exit(1);
  }
}
