//! Which field of a form is the username and which the password – from the
//! signals Android and the browser give (autofill hints, HTML `type` and
//! `autocomplete`, the input type, the view id), never from a value.
//!
//! * **Password**: an HTML `type=password`, or a text input type of the
//!   password variations; a number password only with a `password` hint. A
//!   password hint on a plain text field is not enough (a password never
//!   lands in a visible field). Sign-up and change fields (`new-password`)
//!   and one-time codes are never filled.
//! * **Username**: a `username`/`email` hint or `autocomplete`, an HTML
//!   `type=email`, an e-mail input type, or a view id or name with a word
//!   like `user`, `login`, `email`, `konto`, `benutzer`; never a search, a
//!   card or a one-time code field.
//! * The password: the focused one, else the first; the username: the
//!   focused one, else the nearest before the password, else the nearest
//!   plain text field before it. No password field: only a focused username
//!   with an explicit signal (step 1 of a two-step login). The focused field
//!   must be one of them (unless the user asked: a manual request).
//! * **Origin**: a field's web domain and scheme, its own or its nearest
//!   ancestor's; the password's (or the username's) is the form's, and a
//!   username of another origin (a cross-origin frame) is dropped.

use super::request::{FillRequest, Node};

/// The fields to fill (indices into Kotlin's `AutofillId`s) and the form's
/// web origin (`(domain, scheme)`; `None` for a native form).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    pub username: Option<u32>,
    pub password: Option<u32>,
    pub origin: Option<(String, Option<String>)>,
}

const CLASS_MASK: i32 = 0x0f;
const VARIATION_MASK: i32 = 0xff0;
const CLASS_TEXT: i32 = 1;
const CLASS_NUMBER: i32 = 2;
const TEXT_PASSWORD: [i32; 3] = [0x80, 0x90, 0xe0];
const TEXT_EMAIL: [i32; 2] = [0x20, 0xd0];
const NUMBER_PASSWORD: i32 = 0x10;

const NOT_PASSWORD: [&str; 5] = [
    "new-password",
    "newpassword",
    "one-time-code",
    "otp",
    "smsotpcode",
];
const USERNAME_HINTS: [&str; 3] = ["username", "emailaddress", "email"];
const USERNAME_WORDS: [&str; 11] = [
    "user", "login", "email", "e-mail", "mail", "account", "konto", "benutzer", "nutzer",
    "anmelde", "kennung",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Password,
    /// A username; `true`: an explicit signal (hint, autocomplete, e-mail type).
    Username(bool),
    /// A plain text field (a username candidate of last resort).
    Text,
    None,
}

/// The node's hints and `autocomplete` tokens, in lower case; `on`/`off` are
/// no hint.
fn tokens(node: &Node) -> Vec<String> {
    node.hints
        .iter()
        .flat_map(|h| h.split_whitespace())
        .chain(
            node.html_autocomplete
                .iter()
                .flat_map(|a| a.split_whitespace()),
        )
        .map(str::to_ascii_lowercase)
        .filter(|t| t != "on" && t != "off")
        .collect()
}

fn lower(value: &Option<String>) -> String {
    value.as_deref().unwrap_or_default().to_ascii_lowercase()
}

fn role(node: &Node) -> Role {
    if node.id.is_none() {
        return Role::None;
    }
    let tokens = tokens(node);
    let has = |list: &[&str]| tokens.iter().any(|t| list.contains(&t.as_str()));
    let html_input = lower(&node.html_tag) == "input";
    let html_type = lower(&node.html_type);
    let class = node.input_type & CLASS_MASK;
    let variation = node.input_type & VARIATION_MASK;
    if has(&NOT_PASSWORD) {
        return Role::None;
    }
    let password_hint = has(&["password", "current-password"]);
    let is_password = (html_input && html_type == "password")
        || (class == CLASS_TEXT && TEXT_PASSWORD.contains(&variation))
        || (class == CLASS_NUMBER && variation == NUMBER_PASSWORD && password_hint);
    if is_password {
        return Role::Password;
    }
    if html_input && matches!(html_type.as_str(), "search" | "hidden") {
        return Role::None;
    }
    if tokens.iter().any(|t| t == "search" || t.starts_with("cc-")) {
        return Role::None;
    }
    let explicit = has(&USERNAME_HINTS)
        || tokens
            .iter()
            .any(|t| t.contains("username") || t.contains("email"))
        || (html_input && html_type == "email")
        || (class == CLASS_TEXT && TEXT_EMAIL.contains(&variation));
    if explicit {
        return Role::Username(true);
    }
    let named = [&node.id_entry, &node.html_name, &node.html_id]
        .into_iter()
        .map(lower)
        .any(|name| USERNAME_WORDS.iter().any(|w| name.contains(w)));
    let plain = if html_input {
        matches!(html_type.as_str(), "" | "text")
    } else {
        node.html_tag.is_none() && class == CLASS_TEXT && variation == 0
    };
    if named && (plain || (html_input && html_type == "email")) {
        return Role::Username(false);
    }
    if plain {
        return Role::Text;
    }
    Role::None
}

/// Each node's effective origin: its own web domain and scheme, or its
/// nearest ancestor's.
fn origins(req: &FillRequest) -> Vec<Option<(String, Option<String>)>> {
    let mut out: Vec<Option<(String, Option<String>)>> = Vec::with_capacity(req.nodes.len());
    for node in &req.nodes {
        let own = node.web_domain.as_ref().filter(|d| !d.is_empty()).map(|d| {
            (
                d.to_ascii_lowercase(),
                node.web_scheme.as_ref().map(|s| s.to_ascii_lowercase()),
            )
        });
        let inherited = node.parent.and_then(|p| out[p as usize].clone());
        out.push(own.or(inherited));
    }
    out
}

/// The username and password fields of the form, or `None` if Keystead
/// offers nothing for it (see the module docs).
pub fn classify(req: &FillRequest) -> Option<Fields> {
    let roles: Vec<Role> = req.nodes.iter().map(role).collect();
    let origins = origins(req);
    let focused = req.nodes.iter().position(|n| n.focused && n.id.is_some());
    let passwords: Vec<usize> = (0..roles.len())
        .filter(|&i| roles[i] == Role::Password)
        .collect();
    let password = focused
        .filter(|f| passwords.contains(f))
        .or_else(|| passwords.first().copied());

    let username = match password {
        Some(p) => focused
            .filter(|&f| matches!(roles[f], Role::Username(_)))
            .or_else(|| {
                (0..p)
                    .rev()
                    .find(|&i| matches!(roles[i], Role::Username(_)))
            })
            .or_else(|| (0..p).rev().find(|&i| roles[i] == Role::Text)),
        // Step 1 of a two-step login: only an explicit username in focus.
        None => focused.filter(|&f| roles[f] == Role::Username(true)),
    };
    let anchor = password.or(username)?;
    let origin = origins[anchor].clone();
    let username = username.filter(|&u| origins[u] == origin);

    let chosen = |i: Option<usize>| i.and_then(|i| req.nodes[i].id);
    let fields = Fields {
        username: chosen(username),
        password: chosen(password),
        origin,
    };
    if !req.manual {
        let focused = focused?;
        if Some(focused) != password && Some(focused) != username {
            return None;
        }
    }
    Some(fields)
}
