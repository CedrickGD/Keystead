// Run: node --test .github/scripts/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ANDROID_MIN_SDK,
  ANDROID_PACKAGE,
  ANDROID_PERMISSIONS,
  apkProblems,
  decodeKeystore,
  formatFingerprint,
  keystoreProperties,
  normalizeFingerprint,
  parseBadging,
  propertiesValue,
  signerDigests,
} from "./android-apk.mjs";

// The release certificate (README, build.yml) and some other one.
const RELEASE = "B7:3D:75:D6:F7:F4:60:5B:0B:4A:7A:3A:8E:00:6E:E8:E3:D0:81:F1:C3:37:85:BC:79:98:A8:8F:02:1F:3C:AA";
const RELEASE_HEX = "b73d75d6f7f4605b0b4a7a3a8e006ee8e3d081f1c33785bc7998a88f021f3caa";
const DEBUG_HEX = "0f".repeat(32);

const apksigner = (digest) => `Signer #1 certificate DN: C=US, O=Android, CN=Android Debug
Signer #1 certificate SHA-256 digest: ${digest}
Signer #1 certificate SHA-1 digest: 0123456789abcdef0123456789abcdef01234567
Signer #1 certificate MD5 digest: 0123456789abcdef0123456789abcdef
`;

const BADGING = `package: name='io.github.cedrickgd.keystead' versionCode='4242' versionName='2.0.1-beta.4242' platformBuildVersionName='17' platformBuildVersionCode='37' compileSdkVersion='37' compileSdkVersionCodename='17'
minSdkVersion:'29'
targetSdkVersion:'37'
uses-permission: name='io.github.cedrickgd.keystead.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'
application-label:'Keystead'
application-icon-160:'res/9w.png'
application: label='Keystead' icon='res/9w.png'
launchable-activity: name='io.github.cedrickgd.keystead.MainActivity'  label='Keystead' icon=''
leanback-launchable-activity: name='io.github.cedrickgd.keystead.MainActivity'  label='Keystead' icon='' banner=''
feature-group: label=''
  uses-feature-not-required: name='android.software.leanback'
main
other-receivers
supports-screens: 'small' 'normal' 'large' 'xlarge'
supports-any-density: 'true'
locales: '--_--'
densities: '160' '240' '320' '480' '640'
native-code: 'arm64-v8a'
`;

const expect = {
  packageName: ANDROID_PACKAGE,
  versionName: "2.0.1-beta.4242",
  versionCode: "4242",
  minSdk: ANDROID_MIN_SDK,
  abi: "arm64-v8a",
  signer: null,
};

test("certificate fingerprints in both spellings", () => {
  assert.equal(normalizeFingerprint(RELEASE), RELEASE_HEX);
  assert.equal(normalizeFingerprint(RELEASE_HEX.toUpperCase()), RELEASE_HEX);
  assert.equal(formatFingerprint(RELEASE_HEX), RELEASE);
  for (const bad of ["", "B7:3D", `${RELEASE}:00`, "zz".repeat(32), null]) assert.throws(() => normalizeFingerprint(bad), undefined, String(bad));
});

test("signer digests are read from apksigner's output", () => {
  assert.deepEqual(signerDigests(apksigner(RELEASE_HEX)), [RELEASE_HEX]);
  assert.deepEqual(signerDigests(apksigner(RELEASE_HEX.toUpperCase())), [RELEASE_HEX]);
  // Key rotation (v3.1) lines name the same certificate again.
  const rotated = `${apksigner(RELEASE_HEX)}Signer (minSdkVersion=33, maxSdkVersion=2147483647) certificate SHA-256 digest: ${RELEASE_HEX}\n`;
  assert.deepEqual(signerDigests(rotated), [RELEASE_HEX]);
  assert.deepEqual(signerDigests(`${apksigner(RELEASE_HEX)}${apksigner(DEBUG_HEX).replaceAll("#1", "#2")}`), [RELEASE_HEX, DEBUG_HEX]);
  assert.deepEqual(signerDigests("DOES NOT VERIFY\nERROR: Missing META-INF/MANIFEST.MF"), []);
  assert.deepEqual(signerDigests(undefined), []);
});

test("badging fields", () => {
  assert.deepEqual(parseBadging(BADGING), {
    packageName: "io.github.cedrickgd.keystead",
    versionCode: "4242",
    versionName: "2.0.1-beta.4242",
    minSdk: 29,
    targetSdk: 37,
    permissions: ["io.github.cedrickgd.keystead.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"],
    debuggable: false,
    nativeCode: ["arm64-v8a"],
  });
  const debug = parseBadging(
    // Older aapt2 versions print `sdkVersion:` instead of `minSdkVersion:`.
    BADGING.replace("minSdkVersion:'29'", "sdkVersion:'29'")
      .replace("application-label", "uses-permission: name='android.permission.INTERNET'\napplication-debuggable\napplication-label")
      .replace("native-code: 'arm64-v8a'", "native-code: 'arm64-v8a' 'x86_64'"),
  );
  assert.equal(debug.minSdk, 29);
  assert.ok(debug.permissions.includes("android.permission.INTERNET"));
  assert.equal(debug.debuggable, true);
  assert.deepEqual(debug.nativeCode, ["arm64-v8a", "x86_64"]);
  // A library's `<uses-permission-sdk-23>` (merged into the APK) counts too.
  const sdk23 = parseBadging(
    BADGING.replace("application-label", "uses-permission-sdk-23: name='android.permission.INTERNET'\napplication-label"),
  );
  assert.deepEqual(sdk23.permissions, [
    "io.github.cedrickgd.keystead.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION",
    "android.permission.INTERNET",
  ]);
  assert.deepEqual(apkProblems({ badging: sdk23, signers: [DEBUG_HEX], expect }), []);
  const storage = parseBadging(
    BADGING.replace("application-label", "uses-permission-sdk-23: name='android.permission.READ_EXTERNAL_STORAGE'\napplication-label"),
  );
  assert.match(apkProblems({ badging: storage, signers: [DEBUG_HEX], expect })[0], /READ_EXTERNAL_STORAGE/);
  // `maxSdkVersion` after the name does not hide a permission either.
  assert.deepEqual(
    parseBadging("uses-permission: name='android.permission.INTERNET' maxSdkVersion='32'\n").permissions,
    ["android.permission.INTERNET"],
  );
  assert.deepEqual(parseBadging(""), {
    packageName: null,
    versionCode: null,
    versionName: null,
    minSdk: null,
    targetSdk: null,
    permissions: [],
    debuggable: false,
    nativeCode: [],
  });
});

test("the release gates", () => {
  const badging = parseBadging(BADGING);
  // Not publishing: any single valid signer (the debug key) passes.
  assert.deepEqual(apkProblems({ badging, signers: [DEBUG_HEX], expect }), []);
  // Publishing: only the release certificate.
  assert.deepEqual(apkProblems({ badging, signers: [RELEASE_HEX], expect: { ...expect, signer: RELEASE_HEX } }), []);
  const wrongKey = apkProblems({ badging, signers: [DEBUG_HEX], expect: { ...expect, signer: RELEASE_HEX } });
  assert.equal(wrongKey.length, 1);
  assert.match(wrongKey[0], /expected the release certificate B7:3D:75/);
  assert.match(apkProblems({ badging, signers: [], expect })[0], /no signer/);
  assert.match(apkProblems({ badging, signers: [RELEASE_HEX, DEBUG_HEX], expect })[0], /more than one signer/);

  const problems = (changed) => apkProblems({ badging: { ...badging, ...changed }, signers: [DEBUG_HEX], expect });
  assert.match(problems({ packageName: "com.keystead.desktop" })[0], /package is com\.keystead\.desktop, expected io\.github\.cedrickgd\.keystead/);
  assert.match(problems({ versionName: "2.0.1" })[0], /versionName is 2\.0\.1, expected 2\.0\.1-beta\.4242/);
  assert.match(problems({ versionCode: "2000001" })[0], /versionCode is 2000001, expected 4242/);
  assert.match(problems({ versionCode: null })[0], /versionCode is missing/);
  assert.match(problems({ minSdk: 24 })[0], /minSdk is 24, expected 29/);
  // Only the allowed permissions (stage 2c: the network for the server
  // sync, the fingerprint unlock's) – the APK of 2.0.1-beta.20 had the last
  // three of them.
  const allowed = [
    "android.permission.INTERNET",
    "android.permission.ACCESS_NETWORK_STATE",
    "android.permission.USE_BIOMETRIC",
    "android.permission.USE_FINGERPRINT",
    "io.github.cedrickgd.keystead.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION",
  ];
  assert.deepEqual(ANDROID_PERMISSIONS, allowed);
  assert.deepEqual(problems({ permissions: allowed }), []);
  for (const extra of [
    "android.permission.READ_CONTACTS",
    "android.permission.ACCESS_WIFI_STATE",
    "android.permission.CHANGE_NETWORK_STATE",
    "android.permission.FOREGROUND_SERVICE",
    "com.example.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION",
  ]) {
    const found = problems({ permissions: [...allowed, extra] });
    assert.equal(found.length, 1, extra);
    assert.ok(found[0].includes(extra), found[0]);
  }
  assert.match(problems({ debuggable: true })[0], /debuggable/);
  assert.match(problems({ nativeCode: ["x86_64"] })[0], /no native code for arm64-v8a \(has: x86_64\)/);
  assert.deepEqual(apkProblems({ badging, signers: [DEBUG_HEX], expect: { ...expect, abi: null } }), []);
});

test("keystore.properties survives every password", () => {
  // java.util.Properties: backslash escapes, ISO-8859-1, leading blanks dropped.
  assert.equal(propertiesValue("plain-Pass_123"), "plain-Pass_123");
  assert.equal(propertiesValue("a=b:c#d!e"), "a=b:c#d!e");
  assert.equal(propertiesValue("back\\slash\\"), "back\\\\slash\\\\");
  assert.equal(propertiesValue(" lead trail "), "\\ lead trail ");
  assert.equal(propertiesValue("tab\tnl\ncr\rff\f"), "tab\\tnl\\ncr\\rff\\f");
  assert.equal(propertiesValue("Grüße€"), "Gr\\u00fc\\u00dfe\\u20ac");
  assert.equal(propertiesValue("🔑"), "\\ud83d\\udd11");
  assert.equal(propertiesValue("\u0001"), "\\u0001");

  const text = keystoreProperties({ storeFile: "/home/runner/work/_temp/keystead-android-release.jks", password: "p\\w d", keyAlias: "keystead" });
  assert.deepEqual(text.split("\n").slice(1), [
    "storeFile=/home/runner/work/_temp/keystead-android-release.jks",
    "password=p\\\\w d",
    "keyAlias=keystead",
    "",
  ]);
  assert.match(text.split("\n")[0], /^# .*never commit/);
  assert.throws(() => keystoreProperties({ storeFile: "/k.jks", password: "", keyAlias: "keystead" }), /password is empty/);
  assert.throws(() => keystoreProperties({ storeFile: "/k.jks", password: "x", keyAlias: undefined }), /keyAlias is empty/);
});

test("only a real keystore is accepted from the secret", () => {
  const pkcs12 = Buffer.concat([Buffer.from([0x30, 0x82, 0x0a, 0x00]), Buffer.alloc(60, 7)]);
  assert.deepEqual(decodeKeystore(pkcs12.toString("base64")), pkcs12);
  // Line-wrapped base64 (`base64` without -w0) is fine.
  assert.deepEqual(decodeKeystore(pkcs12.toString("base64").replace(/(.{20})/g, "$1\n")), pkcs12);
  const jks = Buffer.concat([Buffer.from([0xfe, 0xed, 0xfe, 0xed]), Buffer.alloc(60, 1)]);
  assert.deepEqual(decodeKeystore(jks.toString("base64")), jks);
  for (const bad of ["", "   ", "not base64!", Buffer.from("hello world, this is no keystore at all").toString("base64"), undefined]) {
    assert.throws(() => decodeKeystore(bad), /ANDROID_KEYSTORE_BASE64/, String(bad));
  }
});

test("the release certificate and the app id are the same everywhere", async () => {
  const fs = await import("node:fs");
  const read = (file) => fs.readFileSync(new URL(`../../${file}`, import.meta.url), "utf8");
  const workflow = read(".github/workflows/build.yml");
  const pinned = /^\s*ANDROID_RELEASE_CERT_SHA256:\s*"([0-9A-F:]+)"\s*$/m.exec(workflow)?.[1];
  assert.ok(pinned, "build.yml pins ANDROID_RELEASE_CERT_SHA256");
  assert.equal(normalizeFingerprint(pinned), RELEASE_HEX);
  // README (German and English part) and ARCHITECTURE publish the same value.
  const fingerprints = (text) => [...text.matchAll(/\b(?:[0-9A-F]{2}:){31}[0-9A-F]{2}\b/g)].map((m) => normalizeFingerprint(m[0]));
  const readme = fingerprints(read("README.md"));
  assert.equal(readme.length, 2);
  assert.deepEqual(new Set([...readme, ...fingerprints(read("docs/ARCHITECTURE.md"))]), new Set([RELEASE_HEX]));
  // The gates check what tauri.android.conf.json configures.
  const android = JSON.parse(read("apps/desktop/src-tauri/tauri.android.conf.json"));
  assert.equal(android.identifier, ANDROID_PACKAGE);
  assert.equal(android.bundle.android.minSdkVersion, ANDROID_MIN_SDK);
  // The committed config carries no versionCode – CI writes the run number.
  assert.equal(android.bundle.android.versionCode, undefined);
});
