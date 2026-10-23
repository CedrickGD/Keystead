// Phone layout decisions – plain JavaScript without imports, so the node
// tests (apps/desktop/tests/layout.test.mjs) run them as they are; the types
// live in layout.d.mts. React and the DOM side: lib/phone.ts.

/** Viewports up to this width (CSS px) get the phone layout, whatever the platform. */
export const PHONE_MAX_WIDTH = 700;

/** The media query for `PHONE_MAX_WIDTH`. */
export const PHONE_MEDIA_QUERY = `(max-width: ${PHONE_MAX_WIDTH}px)`;

/** The phone app (Android, later iOS): `AppInfo.platform`. */
export function isPhonePlatform(platform) {
  return platform === "android" || platform === "ios";
}

/** A viewport this wide (CSS px) is narrow (`PHONE_MEDIA_QUERY` matches); 0 or unknown is not. */
export function isNarrowViewport(width) {
  return typeof width === "number" && width > 0 && width <= PHONE_MAX_WIDTH;
}

/**
 * The phone layout (`data-layout="phone"` on <html>): always in the phone
 * app, elsewhere in a narrow viewport (`isNarrowViewport`) – the desktop
 * window never gets that narrow (min. 900 px), the browser preview with the
 * mock does.
 */
export function isPhoneLayout(platform, narrow) {
  return isPhonePlatform(platform) || narrow === true;
}

/**
 * What the Android back gesture does on the main screen once no dialog,
 * menu or drawer is open (those close first, as with Escape – lib/layers.ts):
 *   "editor" – leave the editor (asks first when there are unsaved changes),
 *   "detail" – back from the item to the list,
 *   "search" – clear the search,
 *   "home"   – from a page (settings, report, generator) or a filter to all items,
 *   null     – nothing left: the gesture goes to Android (the app moves to the background).
 */
export function phoneBackStep({ editing, detailOpen, searching, page, filtered }) {
  if (editing) return "editor";
  if (detailOpen) return "detail";
  if (searching && !page) return "search";
  if (page || filtered) return "home";
  return null;
}
