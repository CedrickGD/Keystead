// node --test apps/desktop/tests/*.test.mjs – the phone layout's pure decisions.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  PHONE_MAX_WIDTH,
  PHONE_MEDIA_QUERY,
  isNarrowViewport,
  isPhoneLayout,
  isPhonePlatform,
  phoneBackStep,
} from "../src/lib/layout.mjs";

test("the phone platforms", () => {
  assert.equal(isPhonePlatform("android"), true);
  assert.equal(isPhonePlatform("ios"), true);
  for (const desktop of ["windows", "linux", "macos", "", null, undefined]) {
    assert.equal(isPhonePlatform(desktop), false, String(desktop));
  }
});

test("the phone app always gets the phone layout", () => {
  assert.equal(isPhoneLayout("android", false), true);
  assert.equal(isPhoneLayout("ios", false), true);
  assert.equal(isPhoneLayout("android", true), true);
});

test("elsewhere only a narrow viewport does", () => {
  assert.equal(isPhoneLayout("windows", true), true);
  assert.equal(isPhoneLayout("windows", false), false);
  assert.equal(isPhoneLayout(undefined, false), false);
  assert.equal(isPhoneLayout("linux", true), true);
});

test("the breakpoint", () => {
  assert.equal(PHONE_MAX_WIDTH, 700);
  assert.equal(PHONE_MEDIA_QUERY, "(max-width: 700px)");
  assert.equal(isNarrowViewport(360), true);
  assert.equal(isNarrowViewport(700), true);
  assert.equal(isNarrowViewport(701), false);
  // The desktop window's minimum width.
  assert.equal(isNarrowViewport(900), false);
  // Unknown width (before the first layout): not narrow.
  assert.equal(isNarrowViewport(0), false);
  assert.equal(isNarrowViewport(undefined), false);
  assert.equal(isNarrowViewport(null), false);
});

const base = { editing: false, detailOpen: false, searching: false, page: false, filtered: false };

test("back: editor before detail, detail before search, search before home", () => {
  assert.equal(phoneBackStep({ ...base, editing: true, detailOpen: true, searching: true, filtered: true }), "editor");
  assert.equal(phoneBackStep({ ...base, detailOpen: true, searching: true, filtered: true }), "detail");
  assert.equal(phoneBackStep({ ...base, searching: true, filtered: true }), "search");
  assert.equal(phoneBackStep({ ...base, filtered: true }), "home");
});

test("back from a page goes home, even with a query left in the list", () => {
  assert.equal(phoneBackStep({ ...base, page: true }), "home");
  assert.equal(phoneBackStep({ ...base, page: true, searching: true }), "home");
});

test("back on the unfiltered list is Android's (the app moves to the background)", () => {
  assert.equal(phoneBackStep(base), null);
});
