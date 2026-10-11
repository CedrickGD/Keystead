// Breach counts of the demo backend. The real app asks Have I Been Pwned's
// range API (see "Leaked-password check" in docs/ARCHITECTURE.md); the mock
// answers from this table so the security report and the editor's check
// have something to show: the weak sample passwords are "leaked" with large
// counts, everything else is clean.

import type { LeakedItem, PwnedReport, VaultData } from "../types";

const COUNTS = new Map<string, number>([
  // Sample vaults (see sampleData.ts).
  ["Sommer2019!", 1_872],
  ["hallo123", 164_233],
  ["zalando2020", 23],
  ["passwort", 158_014],
  ["maxi1984", 311],
  ["Winter2023!", 4_419],
  ["admin", 2_911_085],
  // Likely typed into the editor.
  ["password", 52_372_427],
  ["123456", 50_219_874],
  ["qwertz", 1_041_560],
  ["hallo", 1_205_712],
  ["demo", 93_507],
]);

/** How many known breaches hold `password` according to the demo table (0 = none). */
export function mockBreachCount(password: string): number {
  return COUNTS.get(password) ?? 0;
}

/** `check_pwned_vault` of the demo backend: like the real one, the non-trashed logins with a password, grouped by identical password. */
export function mockPwnedReport(data: VaultData): PwnedReport {
  const byPassword = new Map<string, string[]>();
  for (const item of data.items) {
    if (item.type !== "login" || item.deletedAt !== null) continue;
    const password = item.login?.password ?? "";
    if (!password) continue;
    byPassword.set(password, [...(byPassword.get(password) ?? []), item.id]);
  }
  const leaked: LeakedItem[] = [];
  for (const [password, itemIds] of byPassword) {
    const count = mockBreachCount(password);
    if (count > 0) for (const itemId of itemIds) leaked.push({ itemId, count });
  }
  // Like the backend: by count desc, then item id.
  leaked.sort((a, b) => b.count - a.count || (a.itemId < b.itemId ? -1 : a.itemId > b.itemId ? 1 : 0));
  return { checkedAt: Date.now(), checked: byPassword.size, leaked, failed: 0 };
}
