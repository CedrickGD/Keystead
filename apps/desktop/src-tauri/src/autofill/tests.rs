//! Forms as Chrome (`FormData`, per-field origin, HTML attributes), Firefox
//! (GeckoView, no hints) and native apps (input types only) report them.

use serde_json::{json, Value};

use super::browsers::{pins_for, BROWSERS};
use super::classify::{classify, Fields};
use super::request::{parse_request, FillRequest, NODES_MAX, REQUEST_MAX, TEXT_MAX};
use super::target::{target_of, Target};

const OWN: &str = "io.github.cedrickgd.keystead";
const TEXT: i32 = 0x01;
const TEXT_PASSWORD: i32 = 0x81;
const TEXT_EMAIL: i32 = 0x21;
const NUMBER_PASSWORD: i32 = 0x12;

fn request(package: &str, verified: bool, nodes: Vec<Value>) -> Value {
    json!({
        "v": 1,
        "ownPackage": OWN,
        "package": package,
        "browserVerified": verified,
        "nodes": nodes,
    })
}

fn parse(v: &Value) -> FillRequest {
    parse_request(&v.to_string()).expect("a valid request")
}

/// A page's root (the web view) with its origin.
fn web_root(domain: &str, scheme: &str) -> Value {
    json!({"webDomain": domain, "webScheme": scheme})
}

fn html(id: u32, input: &str, extra: Value) -> Value {
    let mut node = json!({"parent": 0, "id": id, "htmlTag": "input", "htmlType": input});
    for (k, v) in extra.as_object().unwrap() {
        node[k] = v.clone();
    }
    node
}

fn native(id: u32, input_type: i32, extra: Value) -> Value {
    let mut node = json!({"parent": 0, "id": id, "inputType": input_type});
    for (k, v) in extra.as_object().unwrap() {
        node[k] = v.clone();
    }
    node
}

fn fields(v: &Value) -> Option<Fields> {
    classify(&parse(v))
}

fn target(v: &Value) -> Option<Target> {
    let req = parse(v);
    let f = classify(&req)?;
    target_of(&req, &f)
}

#[test]
fn a_chrome_login_form() {
    let form = request(
        "com.android.chrome",
        true,
        vec![
            web_root("github.com", "https"),
            html(0, "text", json!({"htmlName": "login", "htmlAutocomplete": "username", "focused": true})),
            html(1, "password", json!({"htmlName": "password", "htmlAutocomplete": "current-password"})),
        ],
    );
    let f = fields(&form).unwrap();
    assert_eq!((f.username, f.password), (Some(0), Some(1)));
    assert_eq!(f.origin, Some(("github.com".into(), Some("https".into()))));
    assert_eq!(
        target(&form),
        Some(Target::Web {
            url: "https://github.com/".into(),
            host: "github.com".into()
        })
    );
}

#[test]
fn two_step_logins_and_sign_up_forms() {
    // Step 1: only an explicit username.
    let step1 = request(
        "com.android.chrome",
        true,
        vec![
            web_root("accounts.example.com", "https"),
            html(0, "email", json!({"focused": true})),
        ],
    );
    let f = fields(&step1).unwrap();
    assert_eq!((f.username, f.password), (Some(0), None));
    // A plain field alone is no login.
    let plain = request(
        "com.android.chrome",
        true,
        vec![web_root("example.com", "https"), html(0, "text", json!({"focused": true}))],
    );
    assert_eq!(fields(&plain), None);
    // Step 2: the password alone.
    let step2 = request(
        "com.android.chrome",
        true,
        vec![web_root("example.com", "https"), html(0, "password", json!({"focused": true}))],
    );
    assert_eq!(fields(&step2).unwrap().password, Some(0));
    // Sign-up: a new password is never filled.
    let signup = request(
        "com.android.chrome",
        true,
        vec![
            web_root("example.com", "https"),
            html(0, "email", json!({"htmlAutocomplete": "username"})),
            html(1, "password", json!({"htmlAutocomplete": "new-password", "focused": true})),
        ],
    );
    assert_eq!(fields(&signup), None);
    // A change form: the current password, not the new one.
    let change = request(
        "com.android.chrome",
        true,
        vec![
            web_root("example.com", "https"),
            html(0, "password", json!({"htmlAutocomplete": "current-password", "focused": true})),
            html(1, "password", json!({"htmlAutocomplete": "new-password"})),
            html(2, "password", json!({"htmlAutocomplete": "new-password"})),
        ],
    );
    let f = fields(&change).unwrap();
    assert_eq!((f.username, f.password), (None, Some(0)));
}

#[test]
fn firefox_html_without_hints_and_native_apps() {
    let firefox = request(
        "org.mozilla.firefox",
        true,
        vec![
            web_root("login.example.org", "https"),
            html(0, "text", json!({"htmlName": "user"})),
            html(1, "password", json!({"focused": true})),
        ],
    );
    let f = fields(&firefox).unwrap();
    assert_eq!((f.username, f.password), (Some(0), Some(1)));
    // A native app: input types and view ids only.
    let app = request(
        "de.bank.app",
        false,
        vec![
            json!({}),
            native(0, TEXT, json!({"idEntry": "login_name", "focused": true})),
            native(1, TEXT_PASSWORD, json!({"idEntry": "pin"})),
        ],
    );
    let f = fields(&app).unwrap();
    assert_eq!((f.username, f.password, f.origin.clone()), (Some(0), Some(1), None));
    assert_eq!(
        target(&app),
        Some(Target::App {
            package: "de.bank.app".into()
        })
    );
    // The nearest plain text field before the password, without a name.
    let bare = request(
        "de.shop.app",
        false,
        vec![json!({}), native(0, TEXT, json!({})), native(1, TEXT_PASSWORD, json!({"focused": true}))],
    );
    assert_eq!(fields(&bare).unwrap().username, Some(0));
    // An e-mail input type is a username.
    let mail = request(
        "de.mail.app",
        false,
        vec![json!({}), native(0, TEXT_EMAIL, json!({"focused": true}))],
    );
    assert_eq!(fields(&mail).unwrap().username, Some(0));
}

#[test]
fn what_is_never_a_password_or_a_username() {
    // A password hint on a plain text field: no password field.
    let visible = request(
        "de.app.x",
        false,
        vec![json!({}), native(0, TEXT, json!({"hints": ["password"], "focused": true}))],
    );
    assert_eq!(fields(&visible), None);
    // A number password needs a password hint; a one-time code never counts.
    let pin = request(
        "de.app.x",
        false,
        vec![json!({}), native(0, NUMBER_PASSWORD, json!({"focused": true}))],
    );
    assert_eq!(fields(&pin), None);
    let pin = request(
        "de.app.x",
        false,
        vec![json!({}), native(0, NUMBER_PASSWORD, json!({"hints": ["password"], "focused": true}))],
    );
    assert_eq!(fields(&pin).unwrap().password, Some(0));
    let otp = request(
        "de.app.x",
        false,
        vec![
            json!({}),
            native(0, TEXT_PASSWORD, json!({"hints": ["smsOTPCode"], "focused": true})),
        ],
    );
    assert_eq!(fields(&otp), None);
    // Search and card fields are no usernames.
    let search = request(
        "com.android.chrome",
        true,
        vec![
            web_root("example.com", "https"),
            html(0, "search", json!({"htmlName": "user", "focused": true})),
        ],
    );
    assert_eq!(fields(&search), None);
    let card = request(
        "com.android.chrome",
        true,
        vec![
            web_root("example.com", "https"),
            html(0, "text", json!({"htmlAutocomplete": "cc-number", "htmlName": "account", "focused": true})),
        ],
    );
    assert_eq!(fields(&card), None);
}

#[test]
fn focus_origin_and_manual_requests() {
    // The focused field is no login field: nothing (unless asked for).
    let mut other = request(
        "com.android.chrome",
        true,
        vec![
            web_root("example.com", "https"),
            html(0, "text", json!({"htmlName": "q", "focused": true})),
            html(1, "email", json!({})),
            html(2, "password", json!({})),
        ],
    );
    assert_eq!(fields(&other), None);
    other["manual"] = json!(true);
    let f = fields(&other).unwrap();
    assert_eq!((f.username, f.password), (Some(1), Some(2)));
    // A username from another frame's origin is dropped.
    let frames = request(
        "com.android.chrome",
        true,
        vec![
            web_root("example.com", "https"),
            json!({"parent": 0, "webDomain": "ads.evil.test", "webScheme": "https"}),
            json!({"parent": 1, "id": 0, "htmlTag": "input", "htmlType": "email"}),
            html(1, "password", json!({"focused": true})),
        ],
    );
    let f = fields(&frames).unwrap();
    assert_eq!((f.username, f.password), (None, Some(1)));
    assert_eq!(f.origin.unwrap().0, "example.com");
    // Not fillable (no id): ignored.
    let unfillable = request(
        "com.android.chrome",
        true,
        vec![
            web_root("example.com", "https"),
            json!({"parent": 0, "htmlTag": "input", "htmlType": "password", "focused": true}),
        ],
    );
    assert_eq!(fields(&unfillable), None);
}

#[test]
fn whose_login_it_is() {
    let login = |package: &str, verified: bool, domain: Option<&str>, scheme: &str| {
        let root = match domain {
            Some(d) => web_root(d, scheme),
            None => json!({}),
        };
        request(
            package,
            verified,
            vec![
                root,
                html(0, "email", json!({})),
                html(1, "password", json!({"focused": true})),
            ],
        )
    };
    let web = |host: &str| {
        Some(Target::Web {
            url: format!("https://{host}/"),
            host: host.into(),
        })
    };
    assert_eq!(target(&login("com.android.chrome", true, Some("GitHub.com."), "https")), web("github.com"));
    assert_eq!(target(&login("org.mozilla.firefox", true, Some("münchen.de"), "HTTPS")), web("xn--mnchen-3ya.de"));
    // A browser whose signer is not pinned, without a domain, or over http.
    assert_eq!(target(&login("com.android.chrome", false, Some("github.com"), "https")), None);
    assert_eq!(target(&login("com.android.chrome", true, None, "https")), None);
    assert_eq!(target(&login("com.android.chrome", true, Some("github.com"), "http")), None);
    let mut no_scheme = login("com.android.chrome", true, Some("github.com"), "https");
    no_scheme["nodes"][0].as_object_mut().unwrap().remove("webScheme");
    assert_eq!(target(&no_scheme), None);
    assert_eq!(target(&login("com.android.chrome", true, Some("bank.com@evil.com"), "https")), None);
    // Any other app: its package, whatever domain its web view reports.
    assert_eq!(
        target(&login("com.evil.app", false, Some("bank.com"), "https")),
        Some(Target::App {
            package: "com.evil.app".into()
        })
    );
    // Keystead itself and the system: nothing; a compatibility request too.
    for refused in [OWN, "android", "com.android.settings", "com.android.systemui", "notapackage"] {
        assert_eq!(target(&login(refused, false, None, "")), None, "{refused}");
    }
    let mut compat = login("com.android.chrome", true, Some("github.com"), "https");
    compat["compat"] = json!(true);
    assert_eq!(target(&compat), None);
    // Labels.
    assert_eq!(web("github.com").unwrap().label(), "github.com");
    assert!(web("github.com").unwrap().is_web());
}

#[test]
fn requests_out_of_bounds_are_refused() {
    let ok = request("de.app.x", false, vec![json!({}), native(0, TEXT_PASSWORD, json!({"focused": true}))]);
    assert!(parse_request(&ok.to_string()).is_some());
    let mut wrong_version = ok.clone();
    wrong_version["v"] = json!(2);
    assert!(parse_request(&wrong_version.to_string()).is_none());
    let mut long = ok.clone();
    long["nodes"][1]["idEntry"] = json!("x".repeat(TEXT_MAX + 1));
    assert!(parse_request(&long.to_string()).is_none());
    let mut parent_after = ok.clone();
    parent_after["nodes"][1]["parent"] = json!(1);
    assert!(parse_request(&parent_after.to_string()).is_none());
    let many = request("de.app.x", false, (0..=NODES_MAX).map(|_| json!({})).collect());
    assert!(parse_request(&many.to_string()).is_none());
    assert!(parse_request(&" ".repeat(REQUEST_MAX + 1)).is_none());
    assert!(parse_request("not json").is_none());
}

#[test]
fn the_pinned_browsers() {
    let mut seen = std::collections::BTreeSet::new();
    for b in BROWSERS {
        assert!(seen.insert(b.package), "{} twice", b.package);
        assert!(!b.sha256.is_empty());
        for pin in b.sha256 {
            assert_eq!(pin.len(), 64, "{pin}");
            assert!(pin.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)), "{pin}");
        }
    }
    assert!(pins_for("com.android.chrome").is_some());
    assert!(pins_for("com.android.chrome.evil").is_none());
}
