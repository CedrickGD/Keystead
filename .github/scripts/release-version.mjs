// Version of a CI build, written into the app and the browser extension
// before building (see "Releases & in-app updates" in docs/ARCHITECTURE.md).
//
//   node .github/scripts/release-version.mjs [--write] [--check-tags] [--root <repo dir>]
//
// Environment (set by GitHub Actions): GITHUB_REF, GITHUB_REF_NAME,
// GITHUB_RUN_NUMBER; KEYSTEAD_PUBLISH=true (set by the workflow) when the
// build publishes a release; outputs `version` and `extension_version` go to
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
//
// --check-tags (branch builds only) lists the repository's `v*` tags
// (`git ls-remote --tags origin`). After a release `vX.Y.Z`, tauri.conf.json
// must name the next version: a later `X.Y.Z-beta.<run>` sorts below
// `X.Y.Z`, and no updater would ever offer it. Then this warns – and fails
// when KEYSTEAD_PUBLISH=true (the build would publish such a pre-release).

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { compareSemver, parseSemver } from "./semver.mjs";

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

/**
 * The highest released version (`v<semver>` tag) that sorts above `version`,
 * or null. `tags` are tag names or `refs/tags/…` refs; other tags (e.g.
 * `updater-beta`) are ignored. The same version (a re-run) is not "above".
 */
export function newerReleasedVersion(version, tags) {
  let highest = null;
  for (const tag of tags) {
    const name = tag.trim().replace(/^refs\/tags\//, "");
    if (!name.startsWith("v") || !parseSemver(name.slice(1))) continue;
    const released = name.slice(1);
    if (compareSemver(released, version) > 0 && (!highest || compareSemver(released, highest) > 0)) highest = released;
  }
  return highest;
}

/**
 * Checks a branch build against the existing tags: null if fine, else
 * { level: "warning" | "error", message } – an error only when the build
 * publishes (`publish`).
 */
export function checkAgainstReleases({ version, baseVersion, tags, publish }) {
  const newer = newerReleasedVersion(version, tags);
  if (!newer) return null;
  const v = parseSemver(newer);
  const next = v.pre.length ? "" : ` (e.g. ${v.major}.${v.minor}.${v.patch + 1})`;
  return {
    level: publish ? "error" : "warning",
    message:
      `v${newer} is already released, but tauri.conf.json still says ${baseVersion}: this build (${version}) ` +
      `sorts below it and no updater will offer it. Set "version" in ${TAURI_CONF} to the next version${next}.`,
  };
}

/** The repository's tags (`git ls-remote`), or null if they cannot be listed. */
function remoteTags(root) {
  try {
    const out = execFileSync("git", ["ls-remote", "--tags", "--refs", "origin"], {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return out
      .split("\n")
      .map((line) => line.split("\t")[1])
      .filter(Boolean);
  } catch (err) {
    console.log(`::warning::could not list the repository's tags (${err.message.split("\n")[0]}); version check skipped`);
    return null;
  }
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
  const tagBuild = (process.env.GITHUB_REF ?? "").startsWith("refs/tags/v");
  if (argv.includes("--check-tags") && !tagBuild) {
    const tags = remoteTags(root);
    const problem = tags && checkAgainstReleases({ version, baseVersion: conf.version, tags, publish: process.env.KEYSTEAD_PUBLISH === "true" });
    if (problem?.level === "error") throw new Error(problem.message);
    if (problem) console.log(`::warning::${problem.message}`);
  }
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
