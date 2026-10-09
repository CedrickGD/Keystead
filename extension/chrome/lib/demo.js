// Demo API for popup screenshots and design work: `popup.html?demo=<state>`.
// Renders fake data only – it never touches chrome.storage, the service
// worker or the native messaging host.
//
// States: host_missing, app_unavailable, not_paired, pairing, paired, denied,
// locked, unlocked (default), unlocked_empty, not_web, insecure.

import { ApiError } from "./popup-api.js";

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
  const unlockedStates = new Set(["unlocked", "unlocked_empty", "not_web", "insecure"]);
  let current = unlockedStates.has(state) ? "unlocked" : state;
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
      return { state: shown, vaultName: shown === "unlocked" ? "Privat" : null, appVersion: "2.0.0", error: null };
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

    async unlock(password) {
      await delay(250);
      if (password !== "demo") throw new ApiError("wrong_password");
      current = "unlocked";
      return { vaultName: "Privat" };
    },
    async lock() {
      current = "locked";
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
      return state === "unlocked_empty" || state === "not_web" ? [] : LOGINS;
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
