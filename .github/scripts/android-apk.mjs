// Android release steps of CI (see "Mobile (Android)" in docs/ARCHITECTURE.md).
//
//   node .github/scripts/android-apk.mjs keystore --keystore <file> --properties <file>
//
//     Writes the release keystore (decoded from $ANDROID_KEYSTORE_BASE64) to
//     <file> – outside the repository – and the Gradle signing properties
//     (`gen/android/keystore.properties`: storeFile, password, keyAlias) from
//     $ANDROID_KEYSTORE_PASSWORD and $ANDROID_KEY_ALIAS. The secrets come from
//     the environment only, never from the command line; both files get mode
//     0600. A missing secret is an error: a publishing build must never fall
//     back to the debug key.
//
//   node .github/scripts/android-apk.mjs verify --apk <file> --build-tools <dir>
//        --version-name <v> --version-code <n> [--abi arm64-v8a] [--signer <SHA-256>]
//
//     The gates every CI APK passes: a valid signature (`apksigner verify
//     --print-certs`; with --signer it must be exactly that certificate – the
//     release key, so a wrong keystore secret never produces an APK that
//     cannot update installed apps), 16 KB page alignment of the native
//     libraries (`zipalign -c -P 16 -v 4`) and the manifest (`aapt2 dump
//     badging`): package, versionName, versionCode, minSdk, no INTERNET
//     permission, not debuggable, the ABI. Prints the APK's SHA-256 and the
//     signer certificate (also into $GITHUB_STEP_SUMMARY when set).

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

/** The Android application id (never changes, see CLAUDE.md). */
export const ANDROID_PACKAGE = "io.github.cedrickgd.keystead";
/** `bundle.android.minSdkVersion` in tauri.android.conf.json (Android 10). */
export const ANDROID_MIN_SDK = 29;

/** A SHA-256 certificate fingerprint as 64 lowercase hex digits ("B7:3D:…" or "b73d…"). */
export function normalizeFingerprint(text) {
  const hex = String(text ?? "")
    .replace(/[\s:]/g, "")
    .toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(hex)) throw new Error(`not a SHA-256 fingerprint: ${text}`);
  return hex;
}

/** "B7:3D:…" – the spelling of keytool and the README. */
export function formatFingerprint(hex) {
  return normalizeFingerprint(hex).toUpperCase().match(/../g).join(":");
}

/** The SHA-256 digests of the signer certificates in `apksigner verify --print-certs` output. */
export function signerDigests(output) {
  const digests = [];
  const pattern = /^Signer (?:#\d+|\([^)]*\)) certificate SHA-256 digest: ([0-9a-fA-F]{64})\s*$/gm;
  for (const match of String(output ?? "").matchAll(pattern)) {
    const digest = match[1].toLowerCase();
    if (!digests.includes(digest)) digests.push(digest);
  }
  return digests;
}

/** The fields of `aapt2 dump badging` output the gates look at. */
export function parseBadging(output) {
  const text = String(output ?? "");
  const pkg = /^package: (.*)$/m.exec(text)?.[1] ?? "";
  const attr = (name) => new RegExp(`(?:^|\\s)${name}='([^']*)'`).exec(pkg)?.[1] ?? null;
  const sdk = (name) => {
    const value = new RegExp(`^${name}:'(\\d+)'`, "m").exec(text)?.[1];
    return value === undefined ? null : Number(value);
  };
  const nativeLine = /^native-code: (.*)$/m.exec(text)?.[1] ?? "";
  return {
    packageName: attr("name"),
    versionCode: attr("versionCode"),
    versionName: attr("versionName"),
    minSdk: sdk("sdkVersion") ?? sdk("minSdkVersion"),
    targetSdk: sdk("targetSdkVersion"),
    permissions: [...text.matchAll(/^uses-permission: name='([^']+)'/gm)].map((m) => m[1]),
    debuggable: /^application-debuggable\s*$/m.test(text),
    nativeCode: [...nativeLine.matchAll(/'([^']+)'/g)].map((m) => m[1]),
  };
}

/**
 * Everything wrong with an APK, as messages (empty = all gates pass).
 * `expect`: { packageName, versionName, versionCode, minSdk, abi, signer }
 * – `signer` (normalized fingerprint) only for publishing builds.
 */
export function apkProblems({ badging, signers, expect }) {
  const problems = [];
  const differs = (what, actual, wanted) => {
    if (String(actual) !== String(wanted)) problems.push(`${what} is ${actual ?? "missing"}, expected ${wanted}`);
  };
  differs("package", badging.packageName, expect.packageName);
  differs("versionName", badging.versionName, expect.versionName);
  differs("versionCode", badging.versionCode, expect.versionCode);
  differs("minSdk", badging.minSdk, expect.minSdk);
  if (badging.permissions.includes("android.permission.INTERNET")) {
    problems.push("the APK requests android.permission.INTERNET – the phone app is offline (only debug builds may have it)");
  }
  if (badging.debuggable) problems.push("the APK is debuggable");
  if (expect.abi && !badging.nativeCode.includes(expect.abi)) {
    problems.push(`no native code for ${expect.abi} (has: ${badging.nativeCode.join(", ") || "none"})`);
  }
  if (signers.length === 0) problems.push("no signer certificate found");
  else if (signers.length > 1) problems.push(`more than one signer certificate (${signers.map(formatFingerprint).join(", ")})`);
  else if (expect.signer && signers[0] !== expect.signer) {
    problems.push(
      `signed with ${formatFingerprint(signers[0])}, expected the release certificate ${formatFingerprint(expect.signer)} ` +
        "(check the ANDROID_KEYSTORE_* secrets)",
    );
  }
  return problems;
}

/** A value for a java.util.Properties file (Gradle reads keystore.properties with `load`). */
export function propertiesValue(text) {
  let out = "";
  for (const [i, ch] of [...String(text)].entries()) {
    const code = ch.codePointAt(0);
    if (ch === "\\") out += "\\\\";
    else if (ch === " " && i === 0) out += "\\ ";
    else if (ch === "\t") out += "\\t";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (ch === "\f") out += "\\f";
    else if (code < 0x20 || code > 0x7e) {
      // ISO-8859-1 file: everything else as \uXXXX (UTF-16 code units).
      for (const unit of ch.split("")) out += `\\u${unit.charCodeAt(0).toString(16).padStart(4, "0")}`;
    } else out += ch;
  }
  return out;
}

/** The content of gen/android/keystore.properties (keys read by app/build.gradle.kts). */
export function keystoreProperties({ storeFile, password, keyAlias }) {
  for (const [name, value] of Object.entries({ storeFile, password, keyAlias })) {
    if (!value) throw new Error(`keystore.properties: ${name} is empty`);
  }
  return [
    "# Written by CI (.github/scripts/android-apk.mjs) – never commit this file.",
    `storeFile=${propertiesValue(storeFile)}`,
    `password=${propertiesValue(password)}`,
    `keyAlias=${propertiesValue(keyAlias)}`,
    "",
  ].join("\n");
}

/** The keystore bytes of a base64 secret; refuses anything that is not a PKCS12 or JKS keystore. */
export function decodeKeystore(base64) {
  const text = String(base64 ?? "").replace(/\s+/g, "");
  if (!text || !/^[A-Za-z0-9+/]+={0,2}$/.test(text)) throw new Error("ANDROID_KEYSTORE_BASE64 is not base64");
  const bytes = Buffer.from(text, "base64");
  const pkcs12 = bytes.length > 32 && bytes[0] === 0x30;
  const jks = bytes.length > 32 && bytes.readUInt32BE(0) === 0xfeedfeed;
  if (!pkcs12 && !jks) throw new Error("ANDROID_KEYSTORE_BASE64 does not decode to a PKCS12 or JKS keystore");
  return bytes;
}

function arg(argv, name, { optional = false } = {}) {
  const i = argv.indexOf(name);
  if (i < 0 || !argv[i + 1]) {
    if (optional) return null;
    throw new Error(`missing ${name}`);
  }
  return argv[i + 1];
}

function writeKeystore(argv) {
  const missing = ["ANDROID_KEYSTORE_BASE64", "ANDROID_KEYSTORE_PASSWORD", "ANDROID_KEY_ALIAS"].filter((name) => !process.env[name]);
  if (missing.length) {
    throw new Error(`the secret(s) ${missing.join(", ")} are not set – a publishing build must be signed with the release key`);
  }
  const keystore = path.resolve(arg(argv, "--keystore"));
  const properties = path.resolve(arg(argv, "--properties"));
  const bytes = decodeKeystore(process.env.ANDROID_KEYSTORE_BASE64);
  fs.writeFileSync(keystore, bytes, { mode: 0o600 });
  const content = keystoreProperties({
    storeFile: keystore,
    password: process.env.ANDROID_KEYSTORE_PASSWORD,
    keyAlias: process.env.ANDROID_KEY_ALIAS,
  });
  fs.writeFileSync(properties, content, { mode: 0o600 });
  console.log(`Release keystore (${bytes.length} bytes) and ${path.basename(properties)} written.`);
}

/** Runs a build tool; `{ status, output }` with stdout and stderr together. */
function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (result.error) throw new Error(`${path.basename(command)}: ${result.error.message}`);
  return { status: result.status, output: `${result.stdout ?? ""}${result.stderr ?? ""}` };
}

function tool(buildTools, name) {
  const file = path.join(buildTools, process.platform === "win32" ? `${name}.exe` : name);
  if (!fs.existsSync(file)) throw new Error(`${file} not found (Android build-tools)`);
  return file;
}

function verify(argv) {
  const apk = arg(argv, "--apk");
  const buildTools = arg(argv, "--build-tools");
  const signerArg = arg(argv, "--signer", { optional: true });
  const expect = {
    packageName: ANDROID_PACKAGE,
    versionName: arg(argv, "--version-name"),
    versionCode: arg(argv, "--version-code"),
    minSdk: ANDROID_MIN_SDK,
    abi: arg(argv, "--abi", { optional: true }),
    signer: signerArg ? normalizeFingerprint(signerArg) : null,
  };
  if (!fs.existsSync(apk)) throw new Error(`${apk} not found`);

  const java = process.env.JAVA_HOME ? path.join(process.env.JAVA_HOME, "bin", process.platform === "win32" ? "java.exe" : "java") : "java";
  const apksignerJar = path.join(buildTools, "lib", "apksigner.jar");
  if (!fs.existsSync(apksignerJar)) throw new Error(`${apksignerJar} not found (Android build-tools)`);

  const problems = [];
  const signature = run(java, ["-jar", apksignerJar, "verify", "--print-certs", apk]);
  if (signature.status !== 0) problems.push(`apksigner verify failed:\n${signature.output.trim()}`);
  const signers = signerDigests(signature.output);

  const alignment = run(tool(buildTools, "zipalign"), ["-c", "-P", "16", "-v", "4", apk]);
  if (alignment.status !== 0) {
    const tail = alignment.output.trim().split(/\r?\n/).slice(-30).join("\n");
    problems.push(`zipalign -c -P 16 -v 4 failed (native libraries not 16 KB aligned?):\n${tail}`);
  }

  const dump = run(tool(buildTools, "aapt2"), ["dump", "badging", apk]);
  if (dump.status !== 0) problems.push(`aapt2 dump badging failed:\n${dump.output.trim()}`);
  const badging = parseBadging(dump.output);
  problems.push(...apkProblems({ badging, signers, expect }));

  const sha256 = crypto.createHash("sha256").update(fs.readFileSync(apk)).digest("hex");
  const summary = [
    `### ${path.basename(apk)}`,
    "",
    `* Package \`${badging.packageName}\`, versionName \`${badging.versionName}\`, versionCode \`${badging.versionCode}\`, minSdk ${badging.minSdk}, targetSdk ${badging.targetSdk}, ABI ${badging.nativeCode.join(", ") || "none"}`,
    `* Size ${(fs.statSync(apk).size / 1024 / 1024).toFixed(1)} MiB, SHA-256 \`${sha256}\``,
    `* Signer certificate SHA-256 ${signers.length ? signers.map((d) => `\`${formatFingerprint(d)}\``).join(", ") : "none"}${
      expect.signer ? " (release key required)" : " (not a publishing build: the debug key is fine)"
    }`,
    `* Permissions: ${badging.permissions.join(", ") || "none"}`,
    "",
  ].join("\n");
  console.log(summary);
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${summary}\n`);
  if (problems.length) {
    // The first line becomes the job's annotation.
    const first = problems[0].split("\n")[0];
    throw new Error(`the APK fails ${problems.length} release gate(s): ${first}\n- ${problems.join("\n- ")}`);
  }
  console.log("All APK gates passed.");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [command, ...argv] = process.argv.slice(2);
    if (command === "keystore") writeKeystore(argv);
    else if (command === "verify") verify(argv);
    else throw new Error("usage: android-apk.mjs keystore … | verify …");
  } catch (err) {
    // The details in the log, the first line as the job's annotation.
    console.error(err.message);
    console.error(`::error::${err.message.split("\n")[0]}`);
    process.exit(1);
  }
}
