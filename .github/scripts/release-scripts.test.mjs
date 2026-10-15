// Run: node --test .github/scripts/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { compareSemver, parseSemver } from "./semver.mjs";
import { appVersion, checkAgainstReleases, extensionVersion, newerReleasedVersion } from "./release-version.mjs";
import { latestManifest, releaseNotes, rfc3339 } from "./latest-json.mjs";

test("semver order matches the updater", () => {
  const ordered = ["1.1.0", "2.0.0-beta.4", "2.0.0-beta.5", "2.0.0-beta.10", "2.0.0-rc.1", "2.0.0", "2.0.1-beta.1", "2.0.1", "10.0.0"];
  for (let i = 0; i + 1 < ordered.length; i += 1) {
    assert.equal(compareSemver(ordered[i], ordered[i + 1]), -1, `${ordered[i]} < ${ordered[i + 1]}`);
    assert.equal(compareSemver(ordered[i + 1], ordered[i]), 1);
  }
  assert.equal(compareSemver("2.0.0-beta.5", "2.0.0-beta.5"), 0);
  assert.equal(compareSemver("1.0.0-alpha", "1.0.0-alpha.1"), -1);
  assert.equal(compareSemver("1.0.0-1", "1.0.0-alpha"), -1);
  for (const bad of ["2.0", "v2.0.0", "02.0.0", "2.0.0-", "2.0.0+build.1", "", null]) assert.equal(parseSemver(bad), null, String(bad));
  assert.throws(() => compareSemver("2.0", "2.0.0"));
});

test("every build gets its own version", () => {
  const base = { baseVersion: "2.0.0" };
  assert.equal(appVersion({ ...base, ref: "refs/heads/main", refName: "main", runNumber: "57" }), "2.0.0-beta.57");
  assert.equal(appVersion({ ...base, ref: "refs/heads/claude/x", refName: "claude/x", runNumber: 58 }), "2.0.0-beta.58");
  assert.equal(appVersion({ ...base, ref: "refs/tags/v2.0.0", refName: "v2.0.0", runNumber: 60 }), "2.0.0");
  assert.equal(appVersion({ ...base, ref: "refs/tags/v2.0.1-rc.1", refName: "v2.0.1-rc.1", runNumber: 61 }), "2.0.1-rc.1");
  assert.equal(appVersion({ baseVersion: "2.1.0", ref: "refs/heads/main", runNumber: 70 }), "2.1.0-beta.70");
  assert.throws(() => appVersion({ ...base, ref: "refs/tags/vNext", refName: "vNext", runNumber: 1 }));
  assert.throws(() => appVersion({ baseVersion: "2.0.0-beta.1", ref: "refs/heads/main", runNumber: 1 }));
  assert.throws(() => appVersion({ ...base, ref: "refs/heads/main", runNumber: undefined }));
});

test("a beta of an already released version is caught", () => {
  const tags = ["refs/tags/v1.4.2", "refs/tags/updater-beta", "refs/tags/v2.0.0-beta.57", "refs/tags/v2.0.0-beta.70", "refs/tags/vNext"];
  // Before the stable release: the next beta is above every tag.
  assert.equal(newerReleasedVersion("2.0.0-beta.71", tags), null);
  // A re-run keeps the run number: its own tag is not "newer".
  assert.equal(newerReleasedVersion("2.0.0-beta.70", tags), null);
  // v2.0.0 released, tauri.conf.json still says 2.0.0.
  const released = [...tags, "refs/tags/v2.0.0"];
  assert.equal(newerReleasedVersion("2.0.0-beta.71", released), "2.0.0");
  assert.equal(newerReleasedVersion("2.0.0-beta.71", [...released, "v2.0.1-rc.1"]), "2.0.1-rc.1");
  // Bumped to 2.0.1: fine again.
  assert.equal(newerReleasedVersion("2.0.1-beta.72", released), null);
  // "beta" < "rc": a release candidate tag also hides later betas of that version.
  assert.equal(newerReleasedVersion("2.1.0-beta.80", ["v2.1.0-rc.1"]), "2.1.0-rc.1");

  assert.equal(checkAgainstReleases({ version: "2.0.1-beta.72", baseVersion: "2.0.1", tags: released, publish: true }), null);
  const warning = checkAgainstReleases({ version: "2.0.0-beta.71", baseVersion: "2.0.0", tags: released, publish: false });
  assert.equal(warning.level, "warning");
  assert.match(warning.message, /v2\.0\.0 is already released/);
  assert.match(warning.message, /e\.g\. 2\.0\.1/);
  assert.match(warning.message, /apps\/desktop\/src-tauri\/tauri\.conf\.json/);
  assert.equal(checkAgainstReleases({ version: "2.0.0-beta.71", baseVersion: "2.0.0", tags: released, publish: true }).level, "error");
  assert.doesNotMatch(checkAgainstReleases({ version: "2.1.0-beta.80", baseVersion: "2.1.0", tags: ["v2.1.0-rc.1"], publish: false }).message, /e\.g\./);
});

test("extension versions are dotted integers and keep growing", () => {
  assert.equal(extensionVersion("2.0.0-beta.57", 57), "2.0.0.57");
  assert.equal(extensionVersion("2.0.0", 60), "2.0.0.60");
  assert.equal(extensionVersion("2.0.1-rc.1", "61"), "2.0.1.61");
  assert.throws(() => extensionVersion("2.0.0", 70000));
  assert.throws(() => extensionVersion("nope", 1));
  // A stable release after betas (higher run number) is newer for Chrome too.
  const chromeCompare = (a, b) => {
    const x = a.split(".").map(Number);
    const y = b.split(".").map(Number);
    for (let i = 0; i < 4; i += 1) if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) < (y[i] ?? 0) ? -1 : 1;
    return 0;
  };
  assert.equal(chromeCompare(extensionVersion("2.0.0-beta.57", 57), extensionVersion("2.0.0", 60)), -1);
});

test("latest.json has the updater's static format", () => {
  const manifest = latestManifest({
    version: "2.0.0-beta.57",
    signature: "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQ==\n",
    url: "https://github.com/CedrickGD/Keystead/releases/download/v2.0.0-beta.57/Keystead-2.0.0-beta.57-windows-setup.exe",
    notes: "Choose the vault from the browser extension",
    date: new Date("2026-10-09T17:00:00.123Z"),
  });
  assert.deepEqual(manifest, {
    version: "2.0.0-beta.57",
    notes: "Choose the vault from the browser extension",
    pub_date: "2026-10-09T17:00:00Z",
    platforms: {
      "windows-x86_64": {
        signature: "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQ==",
        url: "https://github.com/CedrickGD/Keystead/releases/download/v2.0.0-beta.57/Keystead-2.0.0-beta.57-windows-setup.exe",
      },
    },
  });
  assert.throws(() => latestManifest({ version: "2.0.0", signature: "", url: "https://x/y" }));
  assert.throws(() => latestManifest({ version: "2.0.0", signature: "abc=", url: "http://x/y" }));
  assert.throws(() => latestManifest({ version: "2.0", signature: "abc=", url: "https://x/y" }));
});

test("release notes come from the commit subject", () => {
  assert.equal(releaseNotes("Choose the vault from the browser extension [release]\n\nDetails …", "2.0.0"), "Choose the vault from the browser extension");
  assert.equal(releaseNotes("[release]", "2.0.0-beta.3"), "Keystead 2.0.0-beta.3");
  assert.equal(releaseNotes(undefined, "2.0.0"), "Keystead 2.0.0");
  assert.equal(rfc3339(new Date("2026-01-02T03:04:05.678Z")), "2026-01-02T03:04:05Z");
});
