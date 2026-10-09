// Simulated import files and a simplified duplicate check for the mock
// backend (the real one is `keystead_core::import::plan_import`). The file
// is recognised by its name: `*.csv` = Chrome export of the "Privat" sample
// vault (duplicates, two changed passwords, new logins, one invalid row),
// `*encrypted*.json` = encrypted Bitwarden export (refused), other `*.json`
// = Bitwarden JSON, `vault_*.json` / VaultX paths = VaultX 1.x (password
// "demo"), `*.keystead` = Keystead export (password "demo"), anything else =
// unknown format.

import type { ImportConflict, ImportFormat, ImportMatch, ImportPreview, VaultItem } from "../types";
import { legacyImportItems } from "./sampleData";

const DAY = 86_400_000;

export interface MockImportFile {
  format: ImportFormat;
  needsPassword: boolean;
  items: VaultItem[];
  invalid: number;
  warnings: string[];
}

function login(now: number, name: string, url: string, username: string, password: string, daysAgo = 30): VaultItem {
  return {
    id: "",
    type: "login",
    name,
    folderId: null,
    favorite: false,
    notes: "",
    login: { username, password, uris: url ? [{ uri: url, match: "domain" }] : [], totp: "", passwordRevisedAt: null },
    card: null,
    identity: null,
    fields: [],
    passwordHistory: [],
    createdAt: now - daysAgo * DAY,
    updatedAt: now - daysAgo * DAY,
    deletedAt: null,
  };
}

const MAIL = "max.mustermann@gmail.com";

/** A Chrome/Edge password export of someone who also uses the "Privat" sample vault. */
function chromeExport(now: number): VaultItem[] {
  return [
    login(now, "amazon.de", "https://www.amazon.de/ap/signin", MAIL, "Sommer2019!"),
    login(now, "netflix.com", "https://www.netflix.com/login", MAIL, "Sommer2019!"),
    login(now, "instagram.com", "https://www.instagram.com/accounts/login/", "max.muster", "In5ta!Gram#2025"),
    login(now, "kleinanzeigen.de", "https://www.kleinanzeigen.de/m-einloggen.html", "max.mustermann@web.de", "Kl3in#Anz!7q"),
    login(now, "zalando.de", "https://www.zalando.de/login", MAIL, "zalando2020"),
    login(now, "lieferando.de", "https://www.lieferando.de/", MAIL, "Pizza!Fr3itag#22"),
    login(now, "accounts.spotify.com", "https://accounts.spotify.com/de/login", MAIL, "S9!pot#Fy2xQ7mLw"),
    login(now, "dhl.de", "https://www.dhl.de/de/privatkunden.html", MAIL, "Pak3t#Stat!on9"),
    login(now, "store.steampowered.com", "https://store.steampowered.com/login/", "maxgamer84", "St3am#Neu!2025x"),
    login(now, "bahn.de", "https://www.bahn.de/", MAIL, "Zug#Fahrt7!Rhein2Mosel"),
    login(now, "booking.com", "https://account.booking.com/sign-in", MAIL, "B00king!Urlaub#7"),
    login(now, "linkedin.com", "https://www.linkedin.com/login", MAIL, "Ln#8vB2!qZ5mX7rT"),
    login(now, "lieferando.de", "https://www.lieferando.de/", MAIL, "Pizza!Fr3itag#22"),
    login(now, "ikea.com", "https://www.ikea.com/de/de/profile/login/", MAIL, "Bill!Regal#4Kallax"),
  ];
}

function bitwardenExport(now: number): VaultItem[] {
  return [
    login(now, "GitHub", "https://github.com/login", "maxmuster", "gh$Wq9!nVb3@Lr7tYp2"),
    login(now, "Proton Mail", "https://account.proton.me/login", "max.muster@proton.me", "Pr0ton!Mail#Sicher"),
    login(now, "Mastodon", "https://chaos.social/auth/sign_in", "maxmuster", "T00t!Toot#Fedi"),
  ];
}

function keysteadExport(now: number): VaultItem[] {
  return [
    login(now, "Nextcloud", "https://cloud.mustermann.de", "max", "N3xt!Cl0ud#Home"),
    login(now, "Home Assistant", "http://homeassistant.local:8123", "max", "H0me!Assist#2024"),
  ];
}

/** The simulated content of `path`, or an error code. */
export function mockImportFile(path: string, now: number): MockImportFile | { error: string } {
  const name = (path.split(/[\\/]/).pop() ?? path).toLowerCase();
  const lowerPath = path.toLowerCase();
  if (name.endsWith(".keystead")) {
    return { format: "keystead", needsPassword: true, items: keysteadExport(now), invalid: 0, warnings: [] };
  }
  if (name.endsWith(".json") && (name.startsWith("vault") || lowerPath.includes("vaultx"))) {
    return {
      format: "legacy",
      needsPassword: true,
      items: legacyImportItems(now),
      invalid: 0,
      warnings: ["The vault's own two-factor unlock (TotpSecret) is not imported."],
    };
  }
  if (name.endsWith(".json") && name.includes("encrypted")) return { error: "unsupported:bitwarden_encrypted" };
  if (name.endsWith(".json")) {
    return { format: "bitwarden_json", needsPassword: false, items: bitwardenExport(now), invalid: 0, warnings: [] };
  }
  if (name.endsWith(".csv") || name.endsWith(".tsv")) {
    return {
      format: "csv",
      needsPassword: false,
      items: chromeExport(now),
      invalid: 1,
      warnings: ["Row 16 skipped: no usable data."],
    };
  }
  return { error: "unsupported:unknown_format" };
}

// ---------------------------------------------------------------------------
// Simplified duplicate check (login: site + username; other types: name)
// ---------------------------------------------------------------------------

function site(item: VaultItem): string {
  const raw = item.login?.uris[0]?.uri.trim() ?? "";
  if (!raw) return "";
  try {
    const url = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(raw) ? raw : `https://${raw}`);
    return url.hostname.toLowerCase().replace(/\.$/, "").replace(/^www\./, "");
  } catch {
    return "";
  }
}

function key(item: VaultItem): string {
  const name = item.name.trim().toLowerCase().replace(/\s+/g, " ");
  if (item.type !== "login") return `${item.type}|${name}`;
  return `login|${site(item) || name}|${(item.login?.username ?? "").trim().toLowerCase()}`;
}

function samePassword(a: VaultItem, b: VaultItem): boolean {
  return (a.login?.password ?? "") === (b.login?.password ?? "");
}

export function describe(item: VaultItem, existing: { id: string; name: string } | null): ImportMatch {
  return {
    incomingName: item.name,
    username: item.login?.username.trim() ?? "",
    site: site(item),
    itemType: item.type,
    existingId: existing?.id ?? "",
    existingName: existing?.name ?? "",
  };
}

export interface MockPlan {
  newItems: VaultItem[];
  duplicates: ImportMatch[];
  conflicts: { conflict: ImportConflict; item: VaultItem }[];
}

/** Sorts `incoming` into new items, duplicates and conflicts against the non-trashed `existing` items. */
export function planMockImport(existing: VaultItem[], incoming: VaultItem[]): MockPlan {
  const active = existing.filter((i) => i.deletedAt === null);
  const plan: MockPlan = { newItems: [], duplicates: [], conflicts: [] };
  const accepted: VaultItem[] = [];
  for (const item of incoming) {
    const k = key(item);
    const matches = active.filter((e) => e.type === item.type && key(e) === k);
    const equal = matches.find((e) => item.type !== "login" || samePassword(e, item));
    if (equal) {
      plan.duplicates.push(describe(item, equal));
      continue;
    }
    const earlier = accepted.find((e) => key(e) === k && (item.type !== "login" || samePassword(e, item)));
    if (earlier) {
      plan.duplicates.push(describe(item, { id: "", name: earlier.name }));
      continue;
    }
    accepted.push(item);
    const target = [...matches].sort((a, b) => b.updatedAt - a.updatedAt)[0];
    if (item.type === "login" && target) {
      const conflict: ImportConflict = {
        ...describe(item, target),
        conflictId: `conflict-${plan.conflicts.length + 1}`,
        reason: "password",
      };
      plan.conflicts.push({ conflict, item });
    } else {
      plan.newItems.push(item);
    }
  }
  return plan;
}

export function mockPreview(plan: MockPlan, file: MockImportFile): ImportPreview {
  return {
    newCount: plan.newItems.length,
    duplicates: plan.duplicates,
    conflicts: plan.conflicts.map((c) => c.conflict),
    invalid: file.invalid,
    warnings: file.warnings,
  };
}
