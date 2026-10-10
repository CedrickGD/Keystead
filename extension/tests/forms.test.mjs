// Run: node --test extension/tests/*.test.mjs
// Form detection of the content script (lib/forms.js) on a small fake DOM
// (helpers/mini-dom.mjs): new-password vs. login classification, one-time
// code fields, and fields inside open shadow roots (web components).

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import { createDocument, installGlobals } from "./helpers/mini-dom.mjs";

installGlobals();
vm.runInThisContext(fs.readFileSync(new URL("../chrome/lib/forms.js", import.meta.url), "utf8"), { filename: "forms.js" });
const Forms = globalThis.KeysteadForms;

const page = (html, options) => createDocument(html, options).document;
const byName = (doc, name) => {
  const all = [doc, ...Forms.openShadowRoots(doc, 1000)].flatMap((root) => root.querySelectorAll("input"));
  return all.find((el) => el.getAttribute("name") === name) ?? null;
};
const names = (fields) => fields.map((f) => f?.getAttribute("name") ?? null);

// ---------------------------------------------------------------------------
// Login vs. signup vs. change
// ---------------------------------------------------------------------------

test("a login form gets login icons and no password suggestion", () => {
  const doc = page(`
    <form action="/login">
      <label for="u">Benutzername</label><input id="u" name="user" autocomplete="username">
      <label for="p">Passwort</label><input id="p" name="pass" type="password" autocomplete="current-password">
      <button>Anmelden</button>
    </form>`);
  const forms = Forms.findLoginForms(doc);
  assert.equal(forms.length, 1);
  assert.equal(forms[0].kind, "login");
  assert.deepEqual(names([forms[0].username, forms[0].password]), ["user", "pass"]);
  assert.deepEqual(forms[0].newPasswords, []);
  assert.deepEqual(names(Forms.iconFields(forms)), ["user", "pass"]);
  assert.deepEqual(Forms.suggestionTargets(forms), []);
});

test("a login form without autocomplete hints stays a login form", () => {
  const doc = page(`
    <form><input name="email" type="email"><input name="password" type="password"><button>Log in</button></form>`,
  { title: "Anmelden oder registrieren", path: "/account" });
  const [form] = Forms.findLoginForms(doc);
  assert.equal(form.kind, "login");
  assert.deepEqual(Forms.suggestionTargets([form]), []);
});

test("signup with autocomplete=new-password: suggestion on the first field, the second confirms it", () => {
  const doc = page(`
    <form id="signup" action="/signup">
      <input name="email" type="email" autocomplete="email">
      <input name="new1" type="password" autocomplete="new-password">
      <input name="new2" type="password" autocomplete="new-password">
      <button>Registrieren</button>
    </form>`);
  const forms = Forms.findLoginForms(doc);
  assert.equal(forms.length, 1);
  assert.equal(forms[0].kind, "signup");
  assert.equal(forms[0].password, null);
  assert.deepEqual(names(forms[0].newPasswords), ["new1", "new2"]);
  assert.deepEqual(Forms.iconFields(forms), [], "signup forms get no login icon");
  const [target] = Forms.suggestionTargets(forms);
  assert.equal(target.field, byName(doc, "new1"));
  assert.deepEqual(names(target.confirm), ["new2"]);
});

test("signup without autocomplete: a second password field that repeats the first", () => {
  const doc = page(`
    <form>
      <label>E-Mail <input name="mail"></label>
      <label>Passwort <input name="pw" type="password"></label>
      <label>Passwort wiederholen <input name="pw_repeat" type="password"></label>
      <button>Weiter</button>
    </form>`);
  const [form] = Forms.findLoginForms(doc);
  assert.equal(form.kind, "signup");
  assert.deepEqual(names(form.newPasswords), ["pw", "pw_repeat"]);
});

test("a single unmarked password field is a signup field only if the form reads like a registration", () => {
  const signup = page(`
    <form action="/register"><input name="email" type="email"><input name="password" type="password">
    <button type="submit">Konto erstellen</button></form>`);
  assert.equal(Forms.findLoginForms(signup)[0].kind, "signup");

  const byPath = page(`<form><input name="email" type="email"><input name="password" type="password"><button>Weiter</button></form>`,
    { title: "Registrierung", path: "/signup" });
  assert.equal(Forms.findLoginForms(byPath)[0].kind, "signup");

  // Login words in the form win over the page title.
  const login = page(`<form><input name="email" type="email"><input name="password" type="password"><button>Anmelden</button></form>`,
    { title: "Registrieren", path: "/register" });
  assert.equal(Forms.findLoginForms(login)[0].kind, "login");
});

test("password change: current, new and confirmation", () => {
  const doc = page(`
    <form>
      <label>Aktuelles Passwort <input name="a" type="password"></label>
      <label>Neues Passwort <input name="b" type="password"></label>
      <label>Neues Passwort bestätigen <input name="c" type="password"></label>
      <button>Speichern</button>
    </form>`);
  const [form] = Forms.findLoginForms(doc);
  assert.equal(form.kind, "change");
  assert.equal(form.password, byName(doc, "a"));
  assert.deepEqual(names(form.newPasswords), ["b", "c"]);
  assert.deepEqual(names(Forms.iconFields([form])), ["a"], "the current password field keeps the login icon");
  const [target] = Forms.suggestionTargets([form]);
  assert.equal(target.field, byName(doc, "b"));
  assert.deepEqual(names(target.confirm), ["c"]);

  const two = page(`<form><input name="old" type="password" autocomplete="current-password">
    <input name="neu" type="password" autocomplete="new-password"></form>`);
  const [change] = Forms.findLoginForms(two);
  assert.equal(change.kind, "change");
  assert.deepEqual(names(change.newPasswords), ["neu"]);
});

test("password roles from autocomplete, names, ids and labels", () => {
  const doc = page(`
    <input name="a" type="password" autocomplete="new-password">
    <input name="b" type="password" autocomplete="current-password">
    <input name="password_confirmation" type="password">
    <input name="c" id="newPassword" type="password">
    <input name="d" type="password" placeholder="Passwort wiederholen">
    <input name="e" type="password" aria-label="Kennwort bestätigen">
    <input name="password_old" type="password">
    <label>Altes Passwort <input name="f" type="password"></label>
    <input name="pass2" type="password">
    <input name="g" type="password" placeholder="Passwort">
    <input name="h" type="password" placeholder="Enter your password">`);
  const role = (name) => Forms.passwordRole(byName(doc, name));
  assert.equal(role("a"), "new");
  assert.equal(role("b"), "current");
  assert.equal(role("password_confirmation"), "new");
  assert.equal(role("c"), "new");
  assert.equal(role("d"), "new");
  assert.equal(role("e"), "new");
  assert.equal(role("password_old"), "current");
  assert.equal(role("f"), "current");
  assert.equal(role("pass2"), "new");
  assert.equal(role("g"), null);
  assert.equal(role("h"), null);
});

test("search fields are never login or suggestion fields", () => {
  const doc = page(`
    <form role="search"><input name="q" type="text"><button>Suchen</button></form>
    <div role="search"><input name="user_search" placeholder="Benutzer suchen"></div>
    <input type="search" name="login">`);
  assert.deepEqual(Forms.findLoginForms(doc), []);
  assert.equal(Forms.isUsernameCandidate(byName(doc, "login")), false);
});

test("captured signup credentials use the confirmed new password", () => {
  const doc = page(`<form><input name="mail" type="email"><input name="n1" type="password" autocomplete="new-password">
    <input name="n2" type="password" autocomplete="new-password"></form>`);
  byName(doc, "mail").value = "neu@example.com";
  byName(doc, "n1").value = "S3cret!";
  byName(doc, "n2").value = "S3cret!";
  const [form] = Forms.findLoginForms(doc);
  assert.deepEqual(Forms.readCredentials(form), { username: "neu@example.com", password: "S3cret!", kind: "signup" });
  byName(doc, "n2").value = "different";
  assert.equal(Forms.readCredentials(form), null, "a confirmation that does not match is not offered");
});

test("fillSuggestion writes the field and its confirmation with input/change events", () => {
  const doc = page(`<form><input name="n1" type="password" autocomplete="new-password">
    <input name="n2" type="password" autocomplete="new-password"></form>`);
  const [target] = Forms.suggestionTargets(Forms.findLoginForms(doc));
  const filled = Forms.fillSuggestion(target, "Gen-3rated!pw");
  assert.deepEqual(names(filled), ["n1", "n2"]);
  assert.equal(byName(doc, "n1").value, "Gen-3rated!pw");
  assert.equal(byName(doc, "n2").value, "Gen-3rated!pw");
  assert.deepEqual(byName(doc, "n2").events.map((e) => e.type), ["input", "change", "keyup"]);
  assert.equal(doc.activeElement, byName(doc, "n1"), "the focus goes back to the suggestion field");
});

// ---------------------------------------------------------------------------
// One-time code (2FA) fields
// ---------------------------------------------------------------------------

test("one-time code fields: autocomplete, otp/totp/2fa/code names with 6–8 characters", () => {
  const doc = page(`
    <input name="a" autocomplete="one-time-code">
    <input name="otp" maxlength="6" inputmode="numeric">
    <input name="code" maxlength="6">
    <input name="totp_token" maxlength="8">
    <input name="login_2fa" maxlength="6">
    <input name="mfa" id="mfa-code" inputmode="numeric">
    <input name="security" aria-label="Authenticator-Code" pattern="\\d{6}">`);
  for (const name of ["a", "otp", "code", "totp_token", "login_2fa", "mfa", "security"]) {
    assert.equal(Forms.isOtpField(byName(doc, name)), true, name);
  }
  const found = Forms.findOtpFields(doc);
  assert.deepEqual(names(found.map((t) => t.field)), ["a", "otp", "code", "totp_token", "login_2fa", "mfa", "security"]);
  assert.ok(found.every((t) => t.group.length === 1 && t.group[0] === t.field));
});

test("code fields that are not 2FA codes", () => {
  const doc = page(`
    <input name="code">
    <input name="code" id="short" maxlength="4">
    <input name="otp" maxlength="12">
    <input name="zip_code" maxlength="6">
    <input name="promo_code" maxlength="8">
    <input name="gutscheincode" maxlength="8">
    <input name="tan" maxlength="6">
    <input name="pin" type="password" maxlength="6">
    <input name="captcha_code" maxlength="6">
    <input name="otp" maxlength="6" type="email">
    <input name="otp" maxlength="6" disabled>
    <input name="otp" maxlength="6" hidden>`);
  assert.deepEqual(Forms.findOtpFields(doc), []);
});

test("a bank PIN stays the password of the login form", () => {
  const doc = page(`<form><input name="kontonummer" autocomplete="username"><input name="pin" type="password" maxlength="6">
    <button>Anmelden</button></form>`);
  const [form] = Forms.findLoginForms(doc);
  assert.equal(form.kind, "login");
  assert.equal(form.password, byName(doc, "pin"));
  assert.deepEqual(Forms.findOtpFields(doc), []);
});

test("an OTP page is no login form, and a masked 2FA field is no password", () => {
  const doc = page(`<form><input name="otp" autocomplete="one-time-code" maxlength="6"><button>Bestätigen</button></form>
    <form><input name="totp" type="password" maxlength="6"><button>Weiter</button></form>`);
  assert.deepEqual(Forms.findLoginForms(doc), []);
  assert.deepEqual(names(Forms.findOtpFields(doc).map((t) => t.field)), ["otp", "totp"]);
});

test("split code inputs (one box per digit) are one target", () => {
  const boxes = Array.from({ length: 6 }, (_, i) => `<span><input name="d${i}" maxlength="1" inputmode="numeric"></span>`).join("");
  const doc = page(`<form><div class="code">${boxes}</div><button>Bestätigen</button></form>
    <div><input name="x1" maxlength="1"><input name="x2" maxlength="1"><input name="x3" maxlength="1"></div>`);
  const found = Forms.findOtpFields(doc);
  assert.equal(found.length, 1, "three plain boxes are not a 6-digit code");
  assert.deepEqual(names(found[0].group), ["d0", "d1", "d2", "d3", "d4", "d5"]);
  assert.equal(Forms.fillOtp(found[0], "123 456"), true);
  assert.deepEqual(found[0].group.map((b) => b.value), ["1", "2", "3", "4", "5", "6"]);
  assert.equal(Forms.fillOtp(found[0], "12345678"), false, "more digits than boxes");

  const single = page(`<input name="otp" maxlength="6">`);
  const [target] = Forms.findOtpFields(single);
  assert.equal(Forms.fillOtp(target, "654321"), true);
  assert.equal(byName(single, "otp").value, "654321");
});

// ---------------------------------------------------------------------------
// Open shadow roots (web components)
// ---------------------------------------------------------------------------

test("a login form inside an open shadow root is found and filled", () => {
  const doc = page(`
    <x-login>
      <template shadowrootmode="open">
        <form>
          <input name="user" autocomplete="username">
          <input name="pass" type="password">
          <button>Anmelden</button>
        </form>
      </template>
    </x-login>`);
  const forms = Forms.findLoginForms(doc);
  assert.equal(forms.length, 1);
  assert.equal(forms[0].kind, "login");
  assert.deepEqual(names([forms[0].username, forms[0].password]), ["user", "pass"]);
  assert.equal(Forms.fillForm(forms[0], { username: "alice", password: "pw-1" }), 2);
  assert.equal(byName(doc, "pass").value, "pw-1");
  const input = byName(doc, "pass").events.find((e) => e.type === "input");
  const change = byName(doc, "pass").events.find((e) => e.type === "change");
  assert.ok(input.composed && input.bubbles, "input is composed: frameworks outside the shadow root notice it");
  assert.ok(change.composed && change.bubbles, "change is composed too");
});

test("fields in separate nested components form one login (scope crosses shadow roots)", () => {
  const doc = page(`
    <main>
      <bank-login>
        <template shadowrootmode="open">
          <div class="card">
            <bank-input name="kennung" label="Zugangsnummer">
              <template shadowrootmode="open"><div class="wrap"><input type="text"></div></template>
            </bank-input>
            <bank-input name="pin" label="PIN">
              <template shadowrootmode="open"><div class="wrap"><input type="password"></div></template>
            </bank-input>
            <bank-button><template shadowrootmode="open"><button type="button"><span>Anmelden</span></button></template></bank-button>
          </div>
        </template>
      </bank-login>
    </main>`);
  const roots = Forms.openShadowRoots(doc);
  assert.equal(roots.length, 4, "nested open roots are found");
  const [form] = Forms.findLoginForms(doc, { shadowRoots: roots });
  assert.equal(form.kind, "login");
  const [user] = roots[1].querySelectorAll("input");
  const [pass] = roots[2].querySelectorAll("input");
  assert.equal(form.username, user, "the username comes from a sibling component (host name/label hints)");
  assert.equal(form.password, pass);
  assert.equal(Forms.formForElement([form], pass), form);
  // A click on the text inside a component's button counts as a submit.
  const span = roots[3].querySelector("span");
  assert.equal(Forms.submitButtonFor(span), roots[3].querySelector("button"));
  // Only the roots the caller knows are searched.
  assert.deepEqual(Forms.findLoginForms(doc, { shadowRoots: [] }), []);
});

test("signup web component: suggestion target and confirmation across shadow roots", () => {
  const field = (name, label) =>
    `<ks-field name="${name}" label="${label}"><template shadowrootmode="open"><input type="password"></template></ks-field>`;
  const doc = page(`
    <ks-signup><template shadowrootmode="open">
      <ks-field name="email" label="E-Mail"><template shadowrootmode="open"><input type="email"></template></ks-field>
      ${field("password", "Passwort")}
      ${field("password_confirm", "Passwort bestätigen")}
      <button>Registrieren</button>
    </template></ks-signup>`);
  const forms = Forms.findLoginForms(doc);
  assert.equal(forms.length, 1);
  assert.equal(forms[0].kind, "signup");
  const [target] = Forms.suggestionTargets(forms);
  assert.equal(target.confirm.length, 1);
  assert.equal(Forms.fillSuggestion(target, "Strong-Pw-123!").length, 2);
  assert.equal(target.confirm[0].value, "Strong-Pw-123!");
});

test("closed shadow roots are not traversed", () => {
  const doc = page(`
    <x-closed><template shadowrootmode="closed"><form><input name="u" autocomplete="username"><input name="p" type="password"></form></template></x-closed>
    <x-open><template shadowrootmode="open"><x-inner><template shadowrootmode="closed"><input name="otp" autocomplete="one-time-code"></template></x-inner></template></x-open>`);
  assert.equal(Forms.openShadowRoots(doc).length, 1);
  assert.deepEqual(Forms.findLoginForms(doc), []);
  assert.deepEqual(Forms.findOtpFields(doc), []);
});

test("one-time code field in a web component", () => {
  const doc = page(`<x-2fa><template shadowrootmode="open"><label for="c">Code aus der Authenticator-App</label>
    <input id="c" name="verification" inputmode="numeric" maxlength="6"></template></x-2fa>`);
  const [target] = Forms.findOtpFields(doc);
  assert.ok(target, "found inside the shadow root");
  assert.equal(Forms.fillOtp(target, "112233"), true);
  assert.equal(target.field.value, "112233");
});

test("composed document order: a host precedes its shadow content, siblings keep their order", () => {
  const doc = page(`
    <input name="before">
    <x-a><template shadowrootmode="open"><input name="inA"><x-b><template shadowrootmode="open"><input name="inB"></template></x-b></template></x-a>
    <input name="after">`);
  const [before, inA, inB, after] = ["before", "inA", "inB", "after"].map((n) => byName(doc, n));
  const host = doc.querySelector("x-a");
  const sorted = [after, inB, before, inA].sort(Forms.compareComposed);
  assert.deepEqual(sorted, [before, inA, inB, after]);
  assert.equal(Forms.compareComposed(host, inA), -1);
  assert.equal(Forms.compareComposed(inB, host), 1);
  assert.equal(Forms.composedContains(host, inB), true);
  assert.equal(Forms.composedContains(host, after), false);
  assert.equal(Forms.composedParent(inA), host);
});
