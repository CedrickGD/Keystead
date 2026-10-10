// Extension settings (popup → "Einstellungen"), stored in
// chrome.storage.local under "settings". Pure: also loaded by
// extension/tests (node --test).

/** The settings and their defaults. */
export const SETTINGS_DEFAULTS = Object.freeze({
  /** Copy the current 2FA code (through the app) after filling a login that has one. */
  autoCopyTotp: true,
  /** Suggest a strong password in signup / password change forms. */
  suggestPasswords: true,
});

/** Known keys with values of the right type over the defaults; anything else is ignored. */
export function normalizeSettings(raw) {
  const settings = { ...SETTINGS_DEFAULTS };
  if (raw && typeof raw === "object") {
    for (const key of Object.keys(SETTINGS_DEFAULTS)) {
      if (typeof raw[key] === typeof SETTINGS_DEFAULTS[key]) settings[key] = raw[key];
    }
  }
  return settings;
}
