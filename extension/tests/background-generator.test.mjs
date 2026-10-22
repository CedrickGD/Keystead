// Run: node --test extension/tests/*.test.mjs
// The service worker's generator traffic (background.js on a fake chrome,
// helpers/fake-chrome.mjs): every password is a preview the app does not
// store (`generate_password` with `remember: false`); only one the user used –
// copied, filled into a page, taken from a suggestion or saved in a new
// login – is reported with `remember_generated`. Older apps answer that with
// `invalid_request`, which counts as "already stored".

import { test, before, beforeEach, after } from "node:test";
import assert from "node:assert/strict";
import { installFakeChrome } from "./helpers/fake-chrome.mjs";

const fake = installFakeChrome();
const PAGE = "https://example.com/signup";
const GENERATOR_TYPES = new Set(["generate_password", "remember_generated", "copy_secret", "save_login"]);

function appWith(overrides = {}) {
  return (request) => {
    if (overrides[request.type]) return overrides[request.type](request);
    switch (request.type) {
      case "status":
        return { appVersion: "2.0.1", paired: true, unlocked: true, vaultName: "Privat", vaultId: "v1", extensionVersion: null, extensionDir: null };
      case "generate_password":
        return "Preview-1";
      case "remember_generated":
      case "copy_secret":
        return null;
      case "save_login":
        return { id: "new" };
      default:
        throw { error: "invalid_request" };
    }
  };
}

/** The generator-related requests the app received. */
const sent = () => fake.requests.filter((r) => GENERATOR_TYPES.has(r.type));

/** Lets fire-and-forget work (context menu, commands) finish. */
async function idle() {
  for (let i = 0; i < 20; i += 1) await fake.settle();
}

before(async () => {
  await chrome.storage.local.set({ credentials: { clientId: "client", token: "token" } });
  await import("../chrome/background.js");
});

beforeEach(async () => {
  fake.app = appWith();
  fake.tabMessage = () => undefined;
  fake.tabs = [{ id: 7, url: PAGE, active: true }];
  await chrome.storage.local.remove("generatorOptions");
  await chrome.storage.session.set({ status: { state: "unlocked", vaultName: "Privat", vaultId: "v1", error: null, ts: Date.now() } });
  await idle();
  fake.requests.length = 0;
  fake.tabMessages.length = 0;
});

after(() => fake.shutdown());

test("the popup's generator only asks for previews", async () => {
  const own = await fake.fromPopup({ type: "popup:generate", options: { kind: "password", length: 30, wordlist: "de", bogus: 1, minDigits: -1 } });
  assert.deepEqual(own, { ok: true, data: "Preview-1" });
  // No own options (`{}`): the app's default preset.
  await fake.fromPopup({ type: "popup:generate", options: {} });
  // `null` without stored options: no options at all.
  await fake.fromPopup({ type: "popup:generate", options: null });
  // `null` with options the popup stored: those.
  await chrome.storage.local.set({ generatorOptions: { length: 12, symbolSet: "!?", extra: true } });
  await fake.fromPopup({ type: "popup:generate", options: null });
  assert.deepEqual(sent(), [
    { type: "generate_password", options: { kind: "password", length: 30, wordlist: "de" }, remember: false },
    { type: "generate_password", options: {}, remember: false },
    { type: "generate_password", remember: false },
    { type: "generate_password", options: { length: 12, symbolSet: "!?" }, remember: false },
  ]);
});

test("copying the preview has the app copy it and keep it in the history", async () => {
  const copied = await fake.fromPopup({ type: "popup:copy-generated", password: "Preview-1" });
  assert.deepEqual(copied, { ok: true, data: { remembered: true } });
  assert.deepEqual(sent(), [
    { type: "copy_secret", text: "Preview-1" },
    { type: "remember_generated", password: "Preview-1" },
  ]);
  assert.deepEqual(await fake.fromPopup({ type: "popup:copy-generated", password: "" }), { ok: false, error: "invalid_request" });
  assert.deepEqual(await fake.fromPopup({ type: "popup:copy-generated", password: "x".repeat(1025) }), { ok: false, error: "invalid_request" });
  assert.equal(sent().length, 2);
});

test("an older app without remember_generated: copying still works", async () => {
  fake.app = appWith({
    remember_generated: () => {
      throw { error: "invalid_request" };
    },
  });
  assert.deepEqual(await fake.fromPopup({ type: "popup:copy-generated", password: "Preview-1" }), { ok: true, data: { remembered: false } });
});

test("filling the preview stores it only once a field took it", async () => {
  fake.tabMessage = (_tabId, message) => (message.type === "bg:fill-generated" ? { filled: 1 } : undefined);
  const filled = await fake.fromPopup({ type: "popup:fill-generated", tabId: 7, password: "Preview-1" });
  assert.deepEqual(filled, { ok: true, data: { filled: 1, remembered: true } });
  assert.deepEqual(sent(), [{ type: "remember_generated", password: "Preview-1" }]);
  assert.equal(fake.tabMessages[0].message.password, "Preview-1");

  fake.requests.length = 0;
  fake.tabMessage = () => ({ filled: 0 });
  assert.deepEqual(await fake.fromPopup({ type: "popup:fill-generated", tabId: 7, password: "Preview-1" }), { ok: false, error: "no_field" });
  assert.deepEqual(sent(), []);
});

test("context menu and shortcut: a preview, stored after the fill only", async () => {
  fake.tabMessage = (_tabId, message) => (message.type === "bg:fill-generated" ? { filled: 1 } : undefined);
  for (const fn of chrome.contextMenus.onClicked.listeners) fn({ menuItemId: "keystead-generate", frameId: 0 }, fake.tabs[0]);
  await idle();
  assert.deepEqual(sent(), [
    { type: "generate_password", remember: false },
    { type: "remember_generated", password: "Preview-1" },
  ]);

  fake.requests.length = 0;
  fake.tabMessages.length = 0;
  fake.tabMessage = () => ({ filled: 0 });
  for (const fn of chrome.commands.onCommand.listeners) fn("generate", fake.tabs[0]);
  await idle();
  assert.deepEqual(sent(), [{ type: "generate_password", remember: false }]);
  assert.ok(fake.tabMessages.some((m) => m.message.type === "bg:toast" && m.message.key === "csNoField"));
});

test("the shortcut uses the popup's own options when it has some", async () => {
  await chrome.storage.local.set({ generatorOptions: { kind: "passphrase", words: 6 } });
  fake.tabMessage = () => ({ filled: 1 });
  for (const fn of chrome.commands.onCommand.listeners) fn("generate", fake.tabs[0]);
  await idle();
  assert.deepEqual(sent()[0], { type: "generate_password", options: { kind: "passphrase", words: 6 }, remember: false });
});

test("a page suggestion is a preview; only Verwenden stores it", async () => {
  const suggested = await fake.fromPage({ type: "cs:suggest-password" }, PAGE);
  assert.deepEqual(suggested, { ok: true, data: { password: "Preview-1" } });
  assert.deepEqual(sent(), [{ type: "generate_password", remember: false }]);
  // A password this tab was never shown is not remembered.
  assert.deepEqual(await fake.fromPage({ type: "cs:suggestion-used", password: "Other" }, PAGE), { ok: true, data: { remembered: false } });
  assert.deepEqual(await fake.fromPage({ type: "cs:suggestion-used", password: "Preview-1" }, PAGE), { ok: true, data: { remembered: true } });
  // Used once.
  assert.deepEqual(await fake.fromPage({ type: "cs:suggestion-used", password: "Preview-1" }, PAGE), { ok: true, data: { remembered: false } });
  assert.deepEqual(sent().slice(1), [{ type: "remember_generated", password: "Preview-1" }]);
});

test("a content script cannot use the popup's generator messages", async () => {
  for (const type of ["popup:generate", "popup:copy-generated", "popup:fill-generated"]) {
    assert.deepEqual(await fake.fromPage({ type, password: "x", tabId: 7 }, PAGE), { ok: false, error: "invalid_request" });
  }
  assert.deepEqual(sent(), []);
});

test("a new login keeps a generated password in the history, a typed one not", async () => {
  const login = { name: "Example", url: PAGE, username: "max", password: "Preview-1" };
  assert.deepEqual(await fake.fromPopup({ type: "popup:save-login", ...login, generated: true }), { ok: true, data: { id: "new" } });
  assert.deepEqual(await fake.fromPopup({ type: "popup:save-login", ...login, password: "typed", generated: false }), { ok: true, data: { id: "new" } });
  assert.deepEqual(
    sent().map((r) => r.type),
    ["save_login", "remember_generated", "save_login"],
  );
});
