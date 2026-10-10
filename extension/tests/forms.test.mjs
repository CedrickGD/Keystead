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

test("a login field marked new-password (to stop browser autofill) stays a login if the form says so", () => {
  const login = page(`<form action="/session"><input name="user" autocomplete="off">
    <input name="pw" type="password" autocomplete="new-password"><button>Anmelden</button></form>`);
  assert.equal(Forms.findLoginForms(login)[0].kind, "login");
  // A real signup form with the same field: no login words on its own buttons.
  const signup = page(`<form action="/join"><input name="user" autocomplete="username">
    <input name="pw" type="password" autocomplete="new-password"><button>Create account</button></form>`,
  { title: "Sign in or create an account" });
  assert.equal(Forms.findLoginForms(signup)[0].kind, "signup");
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

test("GitHub-style signup (e-mail, password, then username): no login step, no login icon, no stored login", () => {
  const doc = page(`
    <header><input type="text" name="q" placeholder="Search or jump to…"></header>
    <form id="signup" action="/signup" method="post">
      <label for="email">Enter your email</label><input type="email" name="user[email]" id="email" autocomplete="off">
      <label for="password">Create a password</label><input type="password" name="user[password]" id="password">
      <label for="login">Enter a username</label><input type="text" name="user[login]" id="login" autocomplete="off">
      <button type="submit">Create account</button>
    </form>`, { title: "Join GitHub · GitHub", path: "/signup" });
  const forms = Forms.findLoginForms(doc);
  assert.deepEqual(forms.map((f) => f.kind), ["signup"], "the username after the password is no username-only login step");
  assert.deepEqual(Forms.iconFields(forms), [], "no login icon anywhere on the signup form");
  const [target] = Forms.suggestionTargets(forms);
  assert.equal(target.field, byName(doc, "user[password]"));
  // Ctrl+Shift+L (Keystead picks the login): a signup form is no fill target, even with the focus in it.
  byName(doc, "user[email]").focus();
  assert.deepEqual(Forms.rankForms(forms, doc.activeElement), []);
  // And a stored password never goes into a field asking for a new one.
  assert.equal(Forms.fillForm(forms[0], { username: "tom@example.com", password: "Tom-Pw-7" }), 1);
  assert.equal(byName(doc, "user[email]").value, "tom@example.com");
  assert.equal(byName(doc, "user[password]").value, "");
  assert.equal(byName(doc, "user[login]").value, "");
});

/** gh-signup-div.html of the browser check: React-style, no <form>, the username in a section after the password. */
const GITHUB_DIV_SIGNUP = `
  <div class="card" id="root"><h2>Create your free account</h2>
    <div class="section" id="credentials">
      <div class="row"><label for="email">Email address</label><input type="email" id="email" name="email"></div>
      <div class="row"><label for="password">Password</label><input type="password" id="password" name="password"></div>
    </div>
    <div class="section" id="profile">
      <div class="row"><label for="username">Username</label><input type="text" id="username" name="username"></div>
    </div>
    <div><button type="button" id="create">Create account</button></div>
  </div>`;

test("div-based GitHub-style signup (no <form>, username in its own section after the password): no login step", () => {
  const doc = page(GITHUB_DIV_SIGNUP, { title: "Create your account", path: "/join" });
  const forms = Forms.findLoginForms(doc);
  assert.deepEqual(forms.map((f) => f.kind), ["signup"], "the username section is no username-only login step");
  assert.deepEqual(Forms.iconFields(forms), [], "no login icon (neither e-mail nor username)");
  assert.equal(Forms.suggestionTargets(forms)[0].field, byName(doc, "password"));
  byName(doc, "username").focus();
  assert.deepEqual(Forms.rankForms(forms, doc.activeElement), [], "Ctrl+Shift+L with the focus in the username: nothing to fill");

  // Even on a page without any signup words (but with "account", a login-step
  // word): the username's container – the card with the only button – holds
  // the form's password, so the username belongs to that form.
  const neutral = page(
    GITHUB_DIV_SIGNUP.replace("Create your free account", "Welcome")
      .replace("Create account", "Continue")
      .replace('id="password" name="password"', 'id="password" name="password" autocomplete="new-password"'),
    { title: "Your account", path: "/" },
  );
  const neutralForms = Forms.findLoginForms(neutral);
  assert.deepEqual(neutralForms.map((f) => f.kind), ["signup"]);
  assert.deepEqual(Forms.iconFields(neutralForms), []);
});

test("div-based signup whose username section has a button of its own: the page's signup words rule out a login step", () => {
  const doc = page(`
    <div id="root"><h1>Konto erstellen</h1>
      <div id="credentials"><input type="email" name="email" placeholder="E-Mail-Adresse">
        <input type="password" name="password" placeholder="Passwort"></div>
      <div id="profile"><input type="text" name="username" placeholder="Benutzername"><button type="button">Verfügbarkeit prüfen</button></div>
      <button type="button">Weiter</button>
    </div>`, { title: "Konto erstellen – Beispiel", path: "/konto" });
  const forms = Forms.findLoginForms(doc);
  assert.deepEqual(forms.map((f) => f.kind), ["signup", "signup"], "the username section is part of the registration, no login step");
  assert.deepEqual(Forms.iconFields(forms), []);
  byName(doc, "username").focus();
  assert.deepEqual(Forms.rankForms(forms, doc.activeElement), []);
});

/** The kinds of the forms found, and whether any field gets a login icon. */
const stepsOf = (doc) => {
  const forms = Forms.findLoginForms(doc);
  return { kinds: forms.map((f) => f.kind), icons: Forms.iconFields(forms).length, forms };
};

test("e-mail-first registration ('Konto erstellen' + 'Weiter') is no login step", () => {
  // ms-signup.html of the browser check: the first step of a registration –
  // no login icon, no Ctrl+Shift+L; only an explicit popup pick fills it.
  const ms = page(`
    <div class="card"><form id="signup" action="/signup" method="post"><h1>Konto erstellen</h1>
      <label for="MemberName">E-Mail-Adresse</label><input type="email" id="MemberName" name="MemberName" placeholder="jemand@example.com">
      <button type="submit" id="next">Weiter</button></form></div>`, { title: "Konto erstellen", path: "/ms-signup.html" });
  const { kinds, icons, forms } = stepsOf(ms);
  assert.deepEqual(kinds, ["signup"]);
  assert.equal(icons, 0, "no login icon");
  assert.deepEqual(Forms.suggestionTargets(forms), [], "nothing to suggest without a password field");
  assert.deepEqual(Forms.rankForms(forms, null), [], "Ctrl+Shift+L: no target");
  assert.deepEqual(Forms.rankForms(forms, null, { explicit: true }), forms, "popup pick: the step is a target");
  assert.equal(Forms.fillForm(forms[0], { username: "tom@example.com", password: "Tom-Pw-7" }, { explicit: true }), 1);
  assert.equal(byName(ms, "MemberName").value, "tom@example.com");
  assert.equal(Forms.readCredentials(forms[0]), null, "a registration's e-mail is not remembered as a login step");

  // Only the heading says so (neutral form; the page title "Mein Konto" alone would read like a login step).
  const heading = page(`
    <form><h2>Konto erstellen</h2><label>E-Mail-Adresse <input type="email" name="email"></label><button>Weiter</button></form>`,
  { title: "Mein Konto – Beispiel", path: "/start" });
  assert.deepEqual(stepsOf(heading).kinds, ["signup"]);
  const sameAsLogin = page(`
    <form><h2>Willkommen</h2><label>E-Mail-Adresse <input type="email" name="email"></label><button>Weiter</button></form>`,
  { title: "Mein Konto – Beispiel", path: "/start" });
  assert.deepEqual(stepsOf(sameAsLogin).kinds, ["username"], "the same page without the heading is a login step");

  // Only the page title says so; an explicit autocomplete=username does not make it a login step either.
  const title = page(`<form><input type="email" name="email" autocomplete="username"><button>Next</button></form>`,
    { title: "Create account – Example", path: "/" });
  assert.deepEqual(stepsOf(title).kinds, ["signup"]);
  assert.equal(stepsOf(title).icons, 0);

  // English wizard (GitHub's old e-mail step): action /signup, 'Join GitHub'.
  const wizard = page(`<form action="/signup?social=false"><label for="e">Enter your email*</label>
    <input type="email" id="e" name="user[email]" autocomplete="off"><button type="button">Continue</button></form>`,
  { title: "Join GitHub · GitHub", path: "/signup" });
  assert.deepEqual(stepsOf(wizard).kinds, ["signup"]);
  assert.equal(stepsOf(wizard).icons, 0);

  // More registration wordings: "Create your free account", "Account erstellen".
  for (const heading of ["Create your free account", "Account erstellen"]) {
    const doc = page(`<form><h1>${heading}</h1><input type="email" name="email" placeholder="E-Mail"><button>Continue</button></form>`,
      { title: "Your account", path: "/start" });
    assert.deepEqual(stepsOf(doc).kinds, ["signup"], heading);
  }
});

test("login steps that also mention creating an account stay login steps", () => {
  // "Anmelden oder Konto erstellen": login words win.
  const either = page(`
    <form><h1>Anmelden oder Konto erstellen</h1>
      <label>E-Mail-Adresse <input type="email" name="email"></label><button>Weiter</button></form>`,
  { title: "Anmelden oder Konto erstellen", path: "/start" });
  const [step] = Forms.findLoginForms(either);
  assert.equal(step?.kind, "username");

  // Google-style: a "Konto erstellen" button beside "Weiter" inside the form, "Anmelden" as heading.
  const googleHtml = `
    <form method="post"><h1>Anmelden</h1>
      <input type="email" id="identifierId" name="identifier" autocomplete="username" aria-label="E-Mail oder Telefonnummer">
      <button type="button">Konto erstellen</button><button type="button">Weiter</button></form>`;
  const google = page(googleHtml, { title: "Google Konten", path: "/v3/signin/identifier" });
  assert.deepEqual(Forms.findLoginForms(google).map((f) => f.kind), ["username"]);
  // The heading alone keeps it a login step (no login word in the title or path).
  const googleNoPath = page(googleHtml, { title: "Google Konten", path: "/v3/identifier" });
  assert.deepEqual(Forms.findLoginForms(googleNoPath).map((f) => f.kind), ["username"]);

  // Amazon's markup: submit <input> labelled by a span, "Erstellen Sie Ihr Amazon-Konto" outside the form.
  const amazon = page(`
    <div id="authportal-main-section">
      <form name="signIn" method="post" action="/ap/signin" class="auth-validate-form">
        <h1>Anmelden</h1>
        <label for="ap_email">E-Mail-Adresse oder Mobiltelefonnummer</label>
        <input type="email" maxlength="128" id="ap_email" name="email">
        <span id="continue"><input id="continue-input" type="submit" aria-labelledby="continue-announce"><span id="continue-announce">Weiter</span></span>
      </form>
      <div>Neu bei Amazon?</div><a id="createAccountSubmit" href="/register">Erstellen Sie Ihr Amazon-Konto</a>
    </div>`, { title: "Amazon Anmelden", path: "/ap/signin" });
  const forms = Forms.findLoginForms(amazon);
  assert.deepEqual(forms.map((f) => f.kind), ["username"]);
  assert.equal(forms[0].username, byName(amazon, "email"));
  assert.deepEqual(Forms.rankForms(forms, null), forms, "Ctrl+Shift+L fills the step");
});

test("German Google login step: 'Konto erstellen' beside 'Weiter', 'Anmelden' above the form, title 'Anmeldung …'", () => {
  // google-in.html of the browser check: the heading sits outside the <form>,
  // "Konto erstellen" is a type=button beside the "Weiter" submit button.
  const googleIn = `
    <div class="card" id="initialView"><h1 id="headingText"><span>Anmelden</span></h1><div id="headingSubtext">Weiter zu Gmail</div>
      <form method="post" novalidate action="/g/identifier">
        <label for="identifierId">E-Mail oder Telefonnummer</label>
        <input type="email" autocomplete="username" name="identifier" id="identifierId" aria-label="E-Mail oder Telefonnummer">
        <div><button type="button" id="forgot">E-Mail-Adresse vergessen?</button></div>
        <div><button type="button" id="create">Konto erstellen</button> <button type="submit" id="next">Weiter</button></div>
      </form></div>`;
  for (const title of ["Anmeldung – Google Konten", "Anmelden – Google Konten", "Google Konten"]) {
    const doc = page(googleIn, { title, path: "/google-in.html" });
    const { kinds, forms } = stepsOf(doc);
    assert.deepEqual(kinds, ["username"], title);
    assert.deepEqual(names(Forms.iconFields(forms)), ["identifier"], `${title}: the step gets the login icon`);
    assert.deepEqual(Forms.rankForms(forms, null), forms, `${title}: Ctrl+Shift+L fills the step`);
  }
  // Without autocomplete=username (no shortcut by the field's score): the heading above the form decides.
  const plain = page(googleIn.replace('autocomplete="username" ', ""), { title: "Google Konten", path: "/v3/x" });
  assert.deepEqual(stepsOf(plain).kinds, ["username"]);

  // google-split.html: the same step without a <form>, every button type=button.
  const split = page(`
    <div class="card" id="view"><div id="headingArea"><h1 id="headingText"><span>Anmelden</span></h1><div id="headingSubtext">Weiter zu Gmail</div></div>
      <div id="content"><div id="ident">
        <label for="identifierId">E-Mail oder Telefonnummer</label>
        <input type="email" autocomplete="username" name="identifier" id="identifierId" aria-label="E-Mail oder Telefonnummer">
        <div><button type="button" id="forgot">E-Mail-Adresse vergessen?</button></div>
        <div><button type="button" id="create">Konto erstellen</button> <button type="button" id="next">Weiter</button></div>
      </div></div></div>`, { title: "Anmeldung – Google Konten", path: "/google-split.html" });
  assert.deepEqual(stepsOf(split).kinds, ["username"]);
  assert.equal(stepsOf(split).icons, 1);

  // Two submit buttons, "Konto erstellen" and "Weiter": the one that only moves on speaks for the step.
  const twoSubmits = page(`<form action="/k"><input type="email" name="email" autocomplete="username">
    <button>Konto erstellen</button><button>Weiter</button></form>`, { title: "Anmeldung", path: "/k" });
  assert.deepEqual(stepsOf(twoSubmits).kinds, ["username"]);

  // ms-signup.html stays a registration step: "Konto erstellen" heading, only "Weiter".
  const ms = page(`<div class="card"><form id="signup" action="/signup" method="post"><h1>Konto erstellen</h1>
    <label for="MemberName">E-Mail-Adresse</label><input type="email" id="MemberName" name="MemberName" placeholder="jemand@example.com">
    <button type="submit" id="next">Weiter</button></form></div>`, { title: "Konto erstellen", path: "/ms-signup.html" });
  assert.deepEqual(stepsOf(ms).kinds, ["signup"]);
  // … also with a neutral action, id and title: its heading says so (role="heading" counts as one).
  const msNeutral = page(`<div class="card"><form action="/s" method="post"><div role="heading" aria-level="1">Create account</div>
    <input type="email" id="MemberName" name="MemberName" placeholder="someone@example.com" aria-label="New email">
    <input type="submit" id="iSignupAction" value="Next"></form></div>`, { title: "Microsoft", path: "/s" });
  assert.deepEqual(stepsOf(msNeutral).kinds, ["signup"]);
  assert.equal(stepsOf(msNeutral).icons, 0);
});

/** shop-dual.html of the browser check: a login form beside the first step of a registration. */
const SHOP_DUAL = `
  <div class="cols">
    <div class="card"><h2>Ich bin bereits Kunde</h2><form id="login" action="/login" method="post">
      <label for="lemail">E-Mail-Adresse</label><input type="email" id="lemail" name="lemail">
      <label for="lpw">Passwort</label><input type="password" id="lpw" name="lpw">
      <button type="submit">Anmelden</button></form></div>
    <div class="card"><h2>Neues Kundenkonto erstellen</h2><form id="reg" action="/register/start" method="post">
      <label for="remail">E-Mail-Adresse</label><input type="email" id="remail" name="remail">
      <button type="submit">Weiter</button></form></div>
  </div>`;

test("a registration step stays one on a page titled 'Anmelden': its own words decide before the page's", () => {
  for (const html of [SHOP_DUAL, SHOP_DUAL.replace("/register/start", "/checkout/b").replace('action="/login"', 'action="/checkout/a"')]) {
    // With a neutral action the heading above the registration form decides ("Neues Kundenkonto erstellen").
    const doc = page(html, { title: "Kasse – Anmelden", path: "/checkout" });
    const forms = Forms.findLoginForms(doc);
    assert.deepEqual(forms.map((f) => f.kind), ["login", "signup"]);
    assert.equal(forms[1].username, byName(doc, "remail"));
    assert.deepEqual(names(Forms.iconFields(forms)), ["lemail", "lpw"], "login icons on the login form only");
    // Ctrl+Shift+L and popup 'Ausfüllen' with the focus in the registration e-mail: the login form.
    byName(doc, "remail").focus();
    assert.deepEqual(Forms.rankForms(forms, doc.activeElement).map((f) => f.kind), ["login"]);
    const [first] = Forms.rankForms(forms, doc.activeElement, { explicit: true });
    assert.equal(first, forms[0]);
    assert.equal(Forms.fillForm(first, { username: "alice", password: "Alice-Pw-1" }), 2);
    assert.deepEqual(["lemail", "lpw", "remail"].map((n) => byName(doc, n).value), ["alice", "Alice-Pw-1", ""]);
  }

  // shop-reg-titled.html: "Anmelden oder registrieren" only in the title, the form itself registers.
  const titled = page(`<div class="card"><h2>Neu hier?</h2><form id="reg" action="/register/start" method="post">
    <label for="remail">E-Mail-Adresse</label><input type="email" id="remail" name="remail">
    <button type="submit">Konto erstellen</button></form></div>`, { title: "Anmelden oder registrieren – Shop", path: "/shop" });
  assert.deepEqual(stepsOf(titled).kinds, ["signup"]);
  assert.equal(stepsOf(titled).icons, 0);

  // A div-based registration step with a "Stattdessen anmelden" button beside "Weiter": still a registration.
  const switchToLogin = page(`<div class="card"><h1>Konto erstellen</h1>
    <div id="step"><input type="email" name="email" placeholder="E-Mail-Adresse">
      <button type="button">Stattdessen anmelden</button><button type="button">Weiter</button></div></div>`,
  { title: "Beispiel", path: "/" });
  assert.deepEqual(stepsOf(switchToLogin).kinds, ["signup"]);

  // "Neuanmeldung" is a registration, "Anmeldung" a login.
  const step = `<form action="/k"><input type="email" name="email" placeholder="E-Mail"><button>Weiter</button></form>`;
  assert.deepEqual(stepsOf(page(step, { title: "Neuanmeldung – Shop", path: "/k" })).kinds, ["signup"]);
  assert.deepEqual(stepsOf(page(step, { title: "Anmeldung – Shop", path: "/k" })).kinds, ["username"]);
});

test("a heading after the step (a signup column beside a login step) or above another form's fields says nothing about it", () => {
  const doc = page(`
    <div class="row">
      <div class="col"><form action="/step1"><label for="e">E-Mail-Adresse</label><input type="email" id="e" name="email"><button>Weiter</button></form></div>
      <div class="col"><h2>Neues Konto erstellen</h2><a href="/register">Jetzt registrieren</a></div>
    </div>`, { title: "Mein Konto", path: "/" });
  assert.deepEqual(stepsOf(doc).kinds, ["username"]);

  // The registration heading is above both columns, the login step sits beside a password form: not the step's heading.
  const shared = page(`
    <div class="wrap"><h1>Neues Konto erstellen</h1>
      <div class="cols">
        <form action="/a"><input type="email" name="a" placeholder="E-Mail"><input type="password" name="pw"><button>Los</button></form>
        <form action="/b"><input type="email" name="b" placeholder="E-Mail"><button>Weiter</button></form>
      </div></div>`, { title: "Mein Konto", path: "/" });
  assert.deepEqual(stepsOf(shared).kinds, ["login", "username"]);
});

test("popup 'Ausfüllen' (explicit pick) also fills a form taken for a registration; Ctrl+Shift+L does not", () => {
  // A login whose password field says new-password and whose button says nothing: taken for a signup.
  const doc = page(`<form action="/session"><input name="user" placeholder="Benutzername">
    <input name="pw" type="password" autocomplete="new-password"><button>Weiter</button></form>`, { title: "Mein Konto" });
  const forms = Forms.findLoginForms(doc);
  assert.deepEqual(forms.map((f) => f.kind), ["signup"]);
  assert.deepEqual(Forms.iconFields(forms), [], "no inline login icon / dropdown on it");
  assert.deepEqual(Forms.rankForms(forms, null), [], "Ctrl+Shift+L: no target");
  const [target] = Forms.rankForms(forms, null, { explicit: true });
  assert.equal(target, forms[0], "popup: the form is a target");
  // Without the explicit flag the password stays out of it …
  assert.equal(Forms.fillForm(target, { username: "alice", password: "Alice-Pw-1" }), 1);
  assert.equal(byName(doc, "pw").value, "");
  // … with it, username and password are filled.
  assert.equal(Forms.fillForm(target, { username: "alice", password: "Alice-Pw-1" }, { explicit: true }), 2);
  assert.equal(byName(doc, "user").value, "alice");
  assert.equal(byName(doc, "pw").value, "Alice-Pw-1");
  assert.deepEqual(byName(doc, "pw").events.map((e) => e.type), ["input", "change", "keyup"]);
});

test("explicit fill of a real signup form: the new password and its confirmation", () => {
  const doc = page(`<form action="/register"><input name="mail" type="email">
    <input name="n1" type="password" autocomplete="new-password"><input name="n2" type="password" autocomplete="new-password">
    <button>Registrieren</button></form>`);
  const [form] = Forms.findLoginForms(doc);
  assert.equal(form.kind, "signup");
  assert.equal(Forms.fillForm(form, { username: "neu@example.com", password: "Prepared-Pw-9" }, { explicit: true }), 3);
  assert.deepEqual([byName(doc, "mail").value, byName(doc, "n1").value, byName(doc, "n2").value], ["neu@example.com", "Prepared-Pw-9", "Prepared-Pw-9"]);
  // An empty stored password writes nothing into the password fields.
  const empty = page(`<form action="/register"><input name="mail" type="email"><input name="n1" type="password" autocomplete="new-password"></form>`);
  const [emptyForm] = Forms.findLoginForms(empty);
  assert.equal(Forms.fillForm(emptyForm, { username: "x@example.com", password: "" }, { explicit: true }), 1);
  assert.equal(byName(empty, "n1").value, "");
});

test("explicit fill on a page with a login and a signup form: the login form first, even with the focus in the signup", () => {
  const doc = page(`
    <form action="/login"><input name="user" autocomplete="username"><input name="pw" type="password"><button>Anmelden</button></form>
    <form action="/register"><input name="mail" type="email"><input name="n1" type="password" autocomplete="new-password">
      <button>Registrieren</button></form>`);
  const forms = Forms.findLoginForms(doc);
  byName(doc, "n1").focus();
  assert.deepEqual(Forms.rankForms(forms, doc.activeElement, { explicit: true }).map((f) => f.kind), ["login", "signup"]);
  // Two signup-like forms: the focused one first.
  const two = page(`
    <form action="/register"><input name="a" type="email"><input name="a1" type="password" autocomplete="new-password"><button>Registrieren</button></form>
    <form action="/join"><input name="b" type="email"><input name="b1" type="password" autocomplete="new-password"><button>Konto erstellen</button></form>`);
  const twoForms = Forms.findLoginForms(two);
  byName(two, "b").focus();
  assert.deepEqual(names(Forms.rankForms(twoForms, two.activeElement, { explicit: true }).map((f) => f.username)), ["b", "a"]);
});

test("a second identifier inside a login form is no separate login step", () => {
  const doc = page(`
    <form action="/portal">
      <label>Kundennummer <input name="kundennummer"></label>
      <label>Benutzername <input name="benutzername"></label>
      <label>Passwort <input name="passwort" type="password"></label>
      <button>Anmelden</button>
    </form>`);
  const forms = Forms.findLoginForms(doc);
  assert.deepEqual(forms.map((f) => f.kind), ["login"]);
  assert.deepEqual(names(Forms.iconFields(forms)), ["benutzername", "passwort"]);
});

test("pages with a login and a signup form: a stored login goes to the login form, whatever has the focus", () => {
  const doc = page(`
    <form action="/login"><input name="user" autocomplete="username"><input name="pw" type="password"><button>Anmelden</button></form>
    <form action="/register"><input name="mail" type="email"><input name="n1" type="password" autocomplete="new-password">
      <input name="n2" type="password" autocomplete="new-password"><button>Registrieren</button></form>`);
  const forms = Forms.findLoginForms(doc);
  assert.deepEqual(forms.map((f) => f.kind), ["login", "signup"]);
  byName(doc, "mail").focus();
  assert.deepEqual(Forms.rankForms(forms, doc.activeElement).map((f) => f.kind), ["login"]);
});

test("Amazon-style first login step: 'E-Mail-Adresse oder Mobiltelefonnummer' is a username, not a postal address", () => {
  const doc = page(`
    <form name="signIn" action="/ap/signin" method="post">
      <label for="ap_email">E-Mail-Adresse oder Mobiltelefonnummer</label>
      <input type="email" id="ap_email" name="email" maxlength="128">
      <button type="submit" id="continue">Weiter</button>
    </form>`, { title: "Amazon Anmelden" });
  const forms = Forms.findLoginForms(doc);
  assert.equal(forms.length, 1);
  assert.equal(forms[0].kind, "username");
  assert.equal(forms[0].username, byName(doc, "email"));
  assert.deepEqual(names(Forms.iconFields(forms)), ["email"], "the step gets the login icon");
  assert.deepEqual(Forms.rankForms(forms, null), forms, "Ctrl+Shift+L fills the step");
  byName(doc, "email").value = "tom@example.com";
  assert.deepEqual(Forms.readCredentials(forms[0]), { username: "tom@example.com", password: "", kind: "username" });
});

test("e-mail address wordings keep their username score; postal addresses still rule a field out", () => {
  const doc = page(`
    <input name="a" type="email" aria-label="E-Mail-Adresse">
    <input name="emailAddress" type="email">
    <input name="b" placeholder="Email address">
    <input name="c" placeholder="Adresse e-mail">
    <input name="d" placeholder="Mailadresse oder Benutzername">
    <input name="street_address" placeholder="Straße und Hausnummer">
    <input name="e" placeholder="Lieferadresse">
    <input name="f" placeholder="Mailing address">
    <input name="email_billing" placeholder="E-Mail-Adresse, Rechnungsadresse">`);
  const score = (name) => Forms.usernameScore(byName(doc, name));
  assert.equal(score("a"), 40 + 25, "type=email + label 'E-Mail-Adresse'");
  assert.equal(score("emailAddress"), 40 + 35, "name emailAddress");
  assert.equal(score("b"), 25);
  assert.equal(score("c"), 25);
  assert.equal(score("d"), 25);
  assert.ok(score("street_address") < 0, "street address");
  assert.ok(score("e") < 0, "Lieferadresse");
  assert.ok(score("f") < 0, "a mailing address is a postal address");
  assert.ok(score("email_billing") < 35, "an e-mail field that also mentions a billing address");
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

test("shadow-root sweep: full rate for 30 s, up to 60 s while custom elements wait for their definition, then every 4th", () => {
  let queries = 0;
  const undefinedTags = { querySelector: () => (queries++, {}) };
  const allDefined = { querySelector: () => (queries++, null) };
  const due = (count, doc) => Forms.shadowSweepDue(count, doc);
  // Sweeps 1–10 (30 s while visible): every one, without looking at the page.
  for (let i = 1; i <= 10; i += 1) assert.equal(due(i, allDefined), true, `sweep ${i}`);
  assert.equal(queries, 0);
  // Sweeps 11–20 (up to 60 s): full rate only while elements wait for their definition.
  assert.equal(due(11, allDefined), false);
  assert.equal(due(11, undefinedTags), true);
  assert.equal(due(20, undefinedTags), true);
  assert.equal(due(12, allDefined), true, "every 4th sweep runs anyway");
  // Later, elements that are never defined (<app-root>) no longer keep the full rate.
  queries = 0;
  const runs = [];
  for (let i = 21; i <= 60; i += 1) if (due(i, undefinedTags)) runs.push(i);
  assert.deepEqual(runs, [24, 28, 32, 36, 40, 44, 48, 52, 56, 60], "every 4th sweep (12 s)");
  assert.equal(queries, 0, "and the page is not even queried any more");
  const broken = { querySelector: () => { throw new Error(":defined unsupported"); } };
  assert.equal(due(13, broken), false);
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
