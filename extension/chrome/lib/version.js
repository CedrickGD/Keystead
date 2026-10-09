// Extension versions and the self-update after an app update.
//
// The desktop app keeps a copy of this extension in a folder of its own and
// reports its version in the bridge `status` (`extensionVersion`, for paired
// clients). If that is newer than the running extension, the folder the
// browser loads us from was most likely updated by the app: the service
// worker reloads the extension once for that version. If it is still older
// afterwards, the extension was loaded from another folder and the popup
// asks the user to load it from the app's folder once.
//
// Chrome extension versions are 1–4 dot-separated integers (0–65535 each).
// Pure functions: also loaded by extension/tests (node --test).

/** [1, 2, 3, 4] for "1.2.3.4"; null if `value` is not a valid extension version. */
export function parseVersion(value) {
  if (typeof value !== "string") return null;
  const parts = value.trim().split(".");
  if (parts.length > 4) return null;
  const numbers = [];
  for (const part of parts) {
    if (!/^\d{1,5}$/.test(part)) return null;
    const n = Number(part);
    if (n > 65535) return null;
    numbers.push(n);
  }
  return numbers;
}

/** -1, 0 or 1 (missing parts count as 0: "2.0" = "2.0.0.0"); null if either version is invalid. */
export function compareVersions(a, b) {
  const x = parseVersion(a);
  const y = parseVersion(b);
  if (!x || !y) return null;
  for (let i = 0; i < 4; i += 1) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d !== 0) return d < 0 ? -1 : 1;
  }
  return 0;
}

/** True if `candidate` is a valid version newer than `current`. */
export function isNewerVersion(candidate, current) {
  return compareVersions(candidate, current) === 1;
}

/**
 * What to do with the version the app delivers (`appExtensionVersion`), given
 * the running version and the version a reload was last attempted for:
 * "current" – nothing; "reload" – reload once; "notice" – a reload for this
 * version did not help: ask the user to load the app's folder.
 */
export function extensionUpdateAction(appExtensionVersion, runningVersion, reloadedFor) {
  if (!isNewerVersion(appExtensionVersion, runningVersion)) return "current";
  return reloadedFor === appExtensionVersion ? "notice" : "reload";
}
