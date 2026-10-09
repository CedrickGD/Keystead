// Demo API for popup screenshots and design work: `popup.html?demo=<state>`.
// Renders fake data only – it never touches chrome.storage, the service
// worker or the native messaging host.
//
// States: host_missing, app_unavailable, not_paired, pairing, paired, denied,
// locked, locked_single, unlocked (default), unlocked_single, unlocked_empty,
// not_web, insecure. "_single": the app has only one vault (else three).
// `&ext=notice` / `&ext=reloading` adds the "new extension version" notice.

import { ApiError } from "./popup-api.js";
import { DEMO_ICONS } from "./demo-icons.js";

const LOGINS = [
  { id: "d1", type: "login", name: "GitHub", subtitle: "max.mustermann@example.com", uri: "https://github.com", favorite: true, hasTotp: true, folderId: null },
  { id: "d2", type: "login", name: "GitHub (Arbeit)", subtitle: "m.mustermann@firma.de", uri: "https://github.com", favorite: false, hasTotp: false, folderId: null },
  { id: "d3", type: "login", name: "GitHub Enterprise", subtitle: "mmustermann", uri: "https://github.com/enterprise", favorite: false, hasTotp: true, folderId: null },
];

const ALL = [
  ...LOGINS,
  { id: "d4", type: "login", name: "Amazon", subtitle: "max.mustermann@example.com", uri: "https://amazon.de", favorite: false, hasTotp: false, folderId: null },
  { id: "d5", type: "login", name: "Deutsche Bahn", subtitle: "max.m", uri: "https://bahn.de", favorite: false, hasTotp: false, folderId: null },
  { id: "d6", type: "login", name: "Sparkasse Online-Banking", subtitle: "12345678", uri: "https://sparkasse.de", favorite: true, hasTotp: true, folderId: null },
  { id: "d7", type: "login", name: "Netflix", subtitle: "familie@example.com", uri: "https://netflix.com", favorite: false, hasTotp: false, folderId: null },
  { id: "d8", type: "login", name: "Gmail", subtitle: "max.mustermann@gmail.com", uri: "https://mail.google.com", favorite: false, hasTotp: true, folderId: null },
];

// Rows carry the website icon the app stored (`icon`); "Deutsche Bahn" has
// none yet and shows the letter.
for (const item of ALL) {
  const icon = DEMO_ICONS[new URL(item.uri).hostname];
  if (icon && item.id !== "d5") item.icon = icon;
}

const VAULTS = [
  { id: "demo-arbeit", name: "Arbeit" },
  { id: "demo-familie", name: "Familie" },
  { id: "demo-privat", name: "Privat" },
];

const WORDS = ["anker", "birke", "dachs", "eule", "fjord", "gipfel", "hafen", "insel", "jolle", "kompass", "lerche", "mond", "nebel", "otter", "pfad", "quelle", "regen", "sturm", "tanne", "ufer", "vogel", "welle", "zeder"];

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function randomInt(max) {
  const limit = Math.floor(0x100000000 / max) * max;
  const buf = new Uint32Array(1);
  do crypto.getRandomValues(buf);
  while (buf[0] >= limit);
  return buf[0] % max;
}

function fakeGenerate(options) {
  if (options.kind === "passphrase") {
    const words = Array.from({ length: options.words || 5 }, () => {
      const word = WORDS[randomInt(WORDS.length)];
      return options.capitalize ? word[0].toUpperCase() + word.slice(1) : word;
    });
    if (options.includeNumber) words[randomInt(words.length)] += String(randomInt(10));
    return words.join(options.separator ?? "-");
  }
  let chars = "";
  if (options.uppercase) chars += options.avoidAmbiguous ? "ABCDEFGHJKLMNPQRSTUVWXYZ" : "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
  if (options.lowercase) chars += options.avoidAmbiguous ? "abcdefghijkmnopqrstuvwxyz" : "abcdefghijklmnopqrstuvwxyz";
  if (options.digits) chars += options.avoidAmbiguous ? "23456789" : "0123456789";
  if (options.symbols) chars += "!@#$%^&*-_=+?";
  if (!chars) chars = "abcdefghijklmnopqrstuvwxyz";
  return Array.from({ length: options.length || 20 }, () => chars[randomInt(chars.length)]).join("");
}

export function createDemoApi(state) {
  const unlockedStates = new Set(["unlocked", "unlocked_single", "unlocked_empty", "not_web", "insecure"]);
  const lockedStates = new Set(["locked", "locked_single"]);
  let current = unlockedStates.has(state) ? "unlocked" : lockedStates.has(state) ? "locked" : state;
  const vaults = state.endsWith("_single") ? VAULTS.filter((v) => v.id === "demo-privat") : VAULTS;
  let openVault = current === "unlocked" ? "demo-privat" : null;
  let chosenVault = null;
  const vaultName = (id) => vaults.find((v) => v.id === id)?.name ?? null;
  let pairing = null;
  if (state === "pairing") pairing = { state: "pending", code: "482913", clientName: "Chrome – Windows", startedAt: Date.now() };
  if (state === "paired") pairing = { state: "success", code: "482913", clientName: "Chrome – Windows", startedAt: Date.now() };
  if (state === "denied") pairing = { state: "denied", code: "482913", clientName: "Chrome – Windows", startedAt: Date.now() };
  let generatorOptions = null;

  const requireUnlocked = () => {
    if (current !== "unlocked") throw new ApiError("locked");
  };

  return {
    demo: true,
    extensionId: "imfndemblnaalppnmdplagajjielnaok",
    version: "2.0.0",

    async status() {
      await delay(120);
      const known = ["host_missing", "app_unavailable", "not_paired", "locked", "unlocked"];
      const shown = known.includes(current) ? current : "not_paired";
      const open = shown === "unlocked" ? openVault : null;
      const ext = new URLSearchParams(location.search).get("ext");
      const extensionUpdate =
        ext === "notice" || ext === "reloading"
          ? { version: "2.0.0.42", dir: "C:\\Users\\Max\\AppData\\Local\\Keystead\\browser-extension", reloading: ext === "reloading" }
          : null;
      return { state: shown, vaultName: vaultName(open), vaultId: open, appVersion: "2.0.0", error: null, extensionUpdate };
    },
    async listVaults() {
      await delay(60);
      return { vaults, currentVaultId: current === "unlocked" ? openVault : null, lastVaultId: "demo-privat" };
    },
    async loadChosenVault() {
      return chosenVault;
    },
    async saveChosenVault(id) {
      chosenVault = id;
    },
    async pairStart(code, clientName) {
      pairing = { state: "pending", code, clientName, startedAt: Date.now() };
      return pairing;
    },
    async pairState() {
      return pairing;
    },
    async pairReset() {
      if (pairing?.state === "success") current = "locked";
      pairing = null;
      return null;
    },
    async pairCancel() {
      pairing = null;
      return null;
    },
    onPairingChange() {
      return () => undefined;
    },

    async unlock(password, vaultId = null) {
      await delay(250);
      const id = vaultId ?? "demo-privat";
      if (!vaultName(id)) throw new ApiError("not_found");
      if (password !== "demo") throw new ApiError("wrong_password");
      current = "unlocked";
      openVault = id;
      return { vaultName: vaultName(id), vaultId: id };
    },
    async lock() {
      current = "locked";
      openVault = null;
      return null;
    },
    async focusApp() {
      return null;
    },

    async tabInfo() {
      if (state === "not_web") return { tabId: 1, web: false };
      const insecure = state === "insecure";
      return {
        tabId: 1,
        web: true,
        url: insecure ? "http://intranet.example.com/login" : "https://github.com/login",
        origin: insecure ? "http://intranet.example.com" : "https://github.com",
        host: insecure ? "intranet.example.com" : "github.com",
        hostname: insecure ? "intranet.example.com" : "github.com",
        insecure,
        neverSave: insecure,
      };
    },
    async matches() {
      requireUnlocked();
      await delay(80);
      if (state === "unlocked_empty" || state === "not_web" || openVault === "demo-familie") return [];
      return openVault === "demo-arbeit" ? LOGINS.filter((l) => l.id === "d2") : LOGINS;
    },
    async search(query) {
      requireUnlocked();
      await delay(80);
      const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
      return ALL.filter((item) => terms.every((term) => `${item.name} ${item.subtitle} ${item.uri}`.toLowerCase().includes(term)));
    },
    async fill() {
      requireUnlocked();
      await delay(150);
      return { filled: true };
    },
    async generate(options) {
      await delay(60);
      return fakeGenerate(options || {});
    },
    async fillGenerated() {
      return { filled: 1 };
    },
    async saveLogin() {
      requireUnlocked();
      await delay(150);
      return { id: "new" };
    },
    async neverRemove() {
      return null;
    },
    async loadGeneratorOptions() {
      return generatorOptions;
    },
    async saveGeneratorOptions(options) {
      generatorOptions = options;
    },
    async copyField(_itemId, field) {
      requireUnlocked();
      return { remaining: field === "totp" ? 23 : null };
    },
    async copySecret() {
      requireUnlocked();
      return null;
    },
    copy: async () => undefined,
    close: () => undefined,
  };
}
