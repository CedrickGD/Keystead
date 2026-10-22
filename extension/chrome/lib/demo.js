// Demo API for popup screenshots and design work: `popup.html?demo=<state>`.
// Renders fake data only – it never touches chrome.storage, the service
// worker or the native messaging host.
//
// States: host_missing, app_unavailable, not_paired, pairing, paired, denied,
// locked, locked_single, unlocked (default), unlocked_single, unlocked_empty,
// not_web, insecure. "_single": the app has only one vault (else three).
// `&ext=notice` / `&ext=reloading` adds the "new extension version" notice.

import { ApiError } from "./popup-api.js";
import { normalizeSettings } from "./settings.js";
import { DEMO_ICONS } from "./demo-icons.js";
import { CHAR_CLASSES, MIN_KEYS, generatorIssue, lenientOptions } from "./generator.js";

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

// Demo word lists (the app embeds the EFF list and a German list of 7724 words).
const WORDS = {
  de: ["anker", "birke", "dachs", "eule", "fjord", "gipfel", "hafen", "insel", "jolle", "kompass", "lerche", "mond", "nebel", "otter", "pfad", "quelle", "regen", "sturm", "tanne", "ufer", "vogel", "welle", "zeder"],
  en: ["anchor", "birch", "badger", "owl", "fjord", "summit", "harbor", "island", "dinghy", "compass", "lark", "moon", "fog", "otter", "path", "spring", "rain", "storm", "pine", "shore", "bird", "wave", "cedar"],
};

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function randomInt(max) {
  const limit = Math.floor(0x100000000 / max) * max;
  const buf = new Uint32Array(1);
  do crypto.getRandomValues(buf);
  while (buf[0] >= limit);
  return buf[0] % max;
}

/** Like the app's generator (minimums, symbol set, exclusions, first letter); invalid options → invalid_request. */
function fakeGenerate(options) {
  if (generatorIssue(options)) throw new ApiError("invalid_request");
  if (options.kind === "passphrase") {
    const list = WORDS[options.wordlist] ?? WORDS.en;
    const words = Array.from({ length: options.words }, () => {
      const word = list[randomInt(list.length)];
      return options.capitalize ? word[0].toUpperCase() + word.slice(1) : word;
    });
    if (options.includeNumber) words[randomInt(words.length)] += String(randomInt(10));
    return words.join(options.separator);
  }
  const usable = (set) =>
    [...new Set(Array.from(set))].filter((c) => !options.exclude.includes(c) && !(options.avoidAmbiguous && "Il1O0".includes(c)));
  const sets = {
    uppercase: usable("ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
    lowercase: usable("abcdefghijklmnopqrstuvwxyz"),
    digits: usable("0123456789"),
    symbols: usable(options.symbolSet),
  };
  const enabled = CHAR_CLASSES.filter((cls) => options[cls]);
  const all = enabled.flatMap((cls) => sets[cls]);
  const letters = [...(options.uppercase ? sets.uppercase : []), ...(options.lowercase ? sets.lowercase : [])];
  const chars = enabled.flatMap((cls) => Array.from({ length: options[MIN_KEYS[cls]] }, () => sets[cls][randomInt(sets[cls].length)]));
  if (options.startWithLetter && !chars.some((c) => letters.includes(c))) chars.push(letters[randomInt(letters.length)]);
  while (chars.length < options.length) chars.push(all[randomInt(all.length)]);
  for (let i = chars.length - 1; i > 0; i -= 1) {
    const j = randomInt(i + 1);
    [chars[i], chars[j]] = [chars[j], chars[i]];
  }
  if (options.startWithLetter && !letters.includes(chars[0])) {
    const positions = chars.map((c, i) => (letters.includes(c) ? i : -1)).filter((i) => i >= 0);
    const pos = positions[randomInt(positions.length)];
    [chars[0], chars[pos]] = [chars[pos], chars[0]];
  }
  return chars.join("");
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
  let settings = normalizeSettings(null);

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
    /** `options`: own options; `null`: the stored ones; without any (or `{}`) the "app's default preset" – here the generator defaults. */
    async generate(options) {
      await delay(60);
      return fakeGenerate(lenientOptions(options ?? generatorOptions ?? {}));
    },
    async fillGenerated() {
      return { filled: 1, remembered: current === "unlocked" };
    },
    async totpTimers(itemIds) {
      requireUnlocked();
      await delay(60);
      // GitHub: 30 s codes, Sparkasse: 60 s (the countdown follows the clock like real TOTP).
      return itemIds.map((id) => {
        const period = id === "d6" ? 60 : 30;
        return { id, period, remaining: period - (Math.floor(Date.now() / 1000) % period) };
      });
    },
    async loadSettings() {
      return { ...settings };
    },
    async saveSettings(next) {
      settings = normalizeSettings(next);
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
    async clearGeneratorOptions() {
      generatorOptions = null;
    },
    async copyField(_itemId, field) {
      requireUnlocked();
      return { remaining: field === "totp" ? 23 : null };
    },
    async copyGenerated() {
      requireUnlocked();
      return { remembered: true };
    },
    copy: async () => undefined,
    close: () => undefined,
  };
}
