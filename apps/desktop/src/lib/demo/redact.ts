// The mock backend's version of the redaction in
// apps/desktop/src-tauri/src/secrets.rs: what `list_items` / `save_item` /
// `set_favorite` answer, and the single secrets of `reveal_secret` /
// `copy_secret_field`. Keep both in sync.

import type { ItemListEntry, SecretField, VaultItem } from "../types";

/** "•••• 1234" for a number with at least 8 digits, "••••" for a shorter one, "" for none. */
export function maskedCardNumber(number: string): string {
  const digits = number.replace(/\D/g, "");
  if (digits.length >= 8) return `•••• ${digits.slice(-4)}`;
  return number.trim() ? "••••" : "";
}

/** A full item without its secrets. */
export function redactItem(item: VaultItem): ItemListEntry {
  return {
    id: item.id,
    type: item.type,
    name: item.name,
    folderId: item.folderId,
    favorite: item.favorite,
    notes: item.notes,
    login: item.login
      ? {
          username: item.login.username,
          password: "",
          hasPassword: item.login.password !== "",
          uris: item.login.uris.map((u) => ({ ...u })),
          totp: "",
          hasTotp: item.login.totp.trim() !== "",
          passwordRevisedAt: item.login.passwordRevisedAt,
        }
      : null,
    card: item.card
      ? {
          cardholderName: item.card.cardholderName,
          brand: item.card.brand,
          number: maskedCardNumber(item.card.number),
          hasNumber: item.card.number.trim() !== "",
          expMonth: item.card.expMonth,
          expYear: item.card.expYear,
          code: "",
          hasCode: item.card.code !== "",
        }
      : null,
    identity: item.identity ? { ...item.identity } : null,
    fields: item.fields.map((f) => ({
      name: f.name,
      value: f.kind === "hidden" ? "" : f.value,
      kind: f.kind,
      hasValue: f.value !== "",
    })),
    passwordHistory: item.passwordHistory.map((h) => ({ replacedAt: h.replacedAt })),
    createdAt: item.createdAt,
    updatedAt: item.updatedAt,
    deletedAt: item.deletedAt,
  };
}

/** Parses the `field` argument like serde does (`invalid_input:field` otherwise). */
export function parseSecretField(value: unknown): SecretField | null {
  if (value === "password" || value === "totp" || value === "totpCode" || value === "cardNumber" || value === "cardCode") {
    return value;
  }
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const entries = Object.entries(value as Record<string, unknown>);
    const [key, index] = entries[0] ?? [];
    if (entries.length === 1 && (key === "custom" || key === "history") && Number.isInteger(index) && (index as number) >= 0) {
      return key === "custom" ? { custom: index as number } : { history: index as number };
    }
  }
  return null;
}

/**
 * The stored value of `field` ("totpCode" is computed by the caller); null =
 * the item has no such field (`not_found`).
 */
export function storedSecret(item: VaultItem, field: Exclude<SecretField, "totpCode">): string | null {
  if (field === "password") return item.login?.password ?? null;
  if (field === "totp") return item.login?.totp ?? null;
  if (field === "cardNumber") return item.card?.number ?? null;
  if (field === "cardCode") return item.card?.code ?? null;
  if ("custom" in field) return item.fields[field.custom]?.value ?? null;
  return item.passwordHistory[field.history]?.password ?? null;
}
