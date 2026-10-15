// Thin wrapper around chrome.i18n (strings live in _locales/<lang>/messages.json).

/** Translated message; falls back to the key so a missing string is visible, not empty. */
export function t(key, substitutions) {
  const args = substitutions === undefined ? undefined : [].concat(substitutions).map(String);
  return chrome.i18n.getMessage(key, args) || key;
}

/** Language of the message file Chrome picked ("de" or "en"), for <html lang>. */
export function uiLanguage() {
  return chrome.i18n.getMessage("langCode") || "de";
}

/** Fills elements marked with data-i18n / data-i18n-title / data-i18n-placeholder. */
export function localize(root) {
  for (const node of root.querySelectorAll("[data-i18n]")) node.textContent = t(node.dataset.i18n);
  for (const node of root.querySelectorAll("[data-i18n-title]")) node.title = t(node.dataset.i18nTitle);
  for (const node of root.querySelectorAll("[data-i18n-placeholder]")) node.placeholder = t(node.dataset.i18nPlaceholder);
}
