//! The serde representation matches the JSON in docs/ARCHITECTURE.md
//! ("Browser bridge protocol"), checked against literal JSON strings.

use keystead_bridge::protocol::{
    extract_id, CopyData, CopyField, IdData, PairData, StatusData, UnlockData, PAIRING_TIMEOUT,
    SEARCH_LIMIT,
};
use keystead_bridge::{BridgeError, LoginSecret, Payload, Request, Response};
use keystead_core::generator::{GeneratorKind, GeneratorOptions};
use keystead_core::totp::TotpCode;
use serde_json::{json, Value};
use zeroize::Zeroizing;

fn parse(s: &str) -> Request {
    Request::parse(s.as_bytes()).unwrap_or_else(|r| panic!("{s} rejected: {r:?}"))
}

fn same_json(actual: &impl serde::Serialize, expected: &str) {
    let actual = serde_json::to_value(actual).unwrap();
    let expected: Value = serde_json::from_str(expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn constants() {
    assert_eq!(PAIRING_TIMEOUT.as_secs(), 120);
    assert_eq!(SEARCH_LIMIT, 50);
}

#[test]
fn every_request_type_parses() {
    let cases: Vec<(&str, Payload)> = vec![
        (r#"{"id":"1","type":"status"}"#, Payload::Status),
        (
            r#"{"id":"1","type":"pair","clientName":"Chrome – DESKTOP-1","code":"042137"}"#,
            Payload::Pair {
                client_name: "Chrome – DESKTOP-1".into(),
                code: "042137".into(),
            },
        ),
        (
            r#"{"id":"1","type":"unlock","password":"pw"}"#,
            Payload::Unlock {
                password: Zeroizing::new("pw".into()),
            },
        ),
        (r#"{"id":"1","type":"lock"}"#, Payload::Lock),
        (r#"{"id":"1","type":"focus_app"}"#, Payload::FocusApp),
        (
            r#"{"id":"1","type":"logins_for_url","url":"https://github.com/login"}"#,
            Payload::LoginsForUrl {
                url: "https://github.com/login".into(),
            },
        ),
        (
            r#"{"id":"1","type":"search","query":"git hub"}"#,
            Payload::Search {
                query: "git hub".into(),
            },
        ),
        (
            r#"{"id":"1","type":"get_login","itemId":"abc"}"#,
            Payload::GetLogin {
                item_id: "abc".into(),
            },
        ),
        (
            r#"{"id":"1","type":"get_totp","itemId":"abc"}"#,
            Payload::GetTotp {
                item_id: "abc".into(),
            },
        ),
        (
            r#"{"id":"1","type":"generate_password"}"#,
            Payload::GeneratePassword { options: None },
        ),
        (
            r#"{"id":"1","type":"save_login","name":"GitHub","url":"https://github.com","username":"me","password":"pw"}"#,
            Payload::SaveLogin {
                name: "GitHub".into(),
                url: "https://github.com".into(),
                username: "me".into(),
                password: Zeroizing::new("pw".into()),
            },
        ),
        (
            r#"{"id":"1","type":"update_password","itemId":"abc","password":"new"}"#,
            Payload::UpdatePassword {
                item_id: "abc".into(),
                password: Zeroizing::new("new".into()),
            },
        ),
        (
            r#"{"id":"1","type":"check_login_password","itemId":"abc","password":"pw"}"#,
            Payload::CheckLoginPassword {
                item_id: "abc".into(),
                password: Zeroizing::new("pw".into()),
            },
        ),
        (
            r#"{"id":"1","type":"copy_field","itemId":"abc","field":"password"}"#,
            Payload::CopyField {
                item_id: "abc".into(),
                field: CopyField::Password,
            },
        ),
        (
            r#"{"id":"1","type":"copy_field","itemId":"abc","field":"totp"}"#,
            Payload::CopyField {
                item_id: "abc".into(),
                field: CopyField::Totp,
            },
        ),
        (
            r#"{"id":"1","type":"copy_secret","text":"pw"}"#,
            Payload::CopySecret {
                text: Zeroizing::new("pw".into()),
            },
        ),
    ];
    for (json, payload) in cases {
        let req = parse(json);
        assert_eq!(req, Request::new("1", payload.clone()), "{json}");
        assert_eq!(
            req.payload.type_name(),
            serde_json::from_str::<Value>(json).unwrap()["type"]
        );
        // Serialising gives back the same JSON object.
        same_json(&req, json);
    }
}

#[test]
fn credentials_are_optional_top_level_fields() {
    let req =
        parse(r#"{"id":"9f1c","type":"get_login","clientId":"c-1","token":"t0k","itemId":"x"}"#);
    assert_eq!(
        req,
        Request::new(
            "9f1c",
            Payload::GetLogin {
                item_id: "x".into()
            }
        )
        .with_credentials("c-1", "t0k")
    );
    same_json(
        &req,
        r#"{"id":"9f1c","type":"get_login","clientId":"c-1","token":"t0k","itemId":"x"}"#,
    );
    // null credentials = none.
    let req = parse(r#"{"id":"1","type":"status","clientId":null,"token":null}"#);
    assert!(req.client_id.is_none() && req.token.is_none());
}

#[test]
fn unknown_fields_are_ignored() {
    let req = parse(r#"{"id":"1","type":"status","extra":{"nested":[1,2]},"v":2}"#);
    assert_eq!(req.payload, Payload::Status);
    let req = parse(r#"{"id":"1","type":"search","query":"q","limit":10}"#);
    assert_eq!(req.payload, Payload::Search { query: "q".into() });
}

#[test]
fn partial_generator_options() {
    let req =
        parse(r#"{"id":"1","type":"generate_password","options":{"length":32,"symbols":false}}"#);
    let Payload::GeneratePassword { options: Some(o) } = req.payload else {
        panic!("options expected");
    };
    assert_eq!(o.length, 32);
    assert!(!o.symbols);
    let d = GeneratorOptions::default();
    assert_eq!(
        (o.uppercase, o.words, o.kind),
        (d.uppercase, d.words, GeneratorKind::Password)
    );
    let req = parse(r#"{"id":"1","type":"generate_password","options":null}"#);
    assert_eq!(req.payload, Payload::GeneratePassword { options: None });
}

#[test]
fn invalid_requests_yield_invalid_request_with_id_when_possible() {
    let cases = [
        ("", ""),
        ("null", ""),
        ("[1,2]", ""),
        ("\"status\"", ""),
        (r#"{"type":"status"}"#, ""),
        (r#"{"id":5,"type":"status"}"#, ""),
        (r#"{"id":"a1"}"#, "a1"),
        (r#"{"id":"a2","type":"nope"}"#, "a2"),
        (r#"{"id":"a3","type":"Status"}"#, "a3"),
        (r#"{"id":"a4","type":"unlock"}"#, "a4"),
        (r#"{"id":"a5","type":"unlock","password":42}"#, "a5"),
        (r#"{"id":"a6","type":"get_login","item_id":"snake"}"#, "a6"),
        (
            r#"{"id":"a7","type":"save_login","name":"n","url":"u","username":"x"}"#,
            "a7",
        ),
        (r#"{"id":"a8","type":"status","token":7}"#, "a8"),
        (r#"{"id":"a9","type":"status""#, ""),
    ];
    for (input, id) in cases {
        let r = Request::parse(input.as_bytes()).expect_err(input);
        assert_eq!(
            r,
            Response::error(id, BridgeError::InvalidRequest),
            "{input}"
        );
    }
    // Invalid UTF-8.
    let r = Request::parse(&[0x7b, 0xff, 0xfe, 0x7d]).unwrap_err();
    assert_eq!(r.error_code(), Some(BridgeError::InvalidRequest));
    assert_eq!(extract_id(br#"{"id":"x","garbage":"#), None);
    assert_eq!(extract_id(br#"{"id":"x","type":17}"#).as_deref(), Some("x"));
}

#[test]
fn response_shapes() {
    same_json(&Response::null("1"), r#"{"id":"1","ok":true,"data":null}"#);
    same_json(
        &Response::error("1", BridgeError::NotPaired),
        r#"{"id":"1","ok":false,"error":"not_paired"}"#,
    );
    // Exact bytes: no "data" on errors, no "error" on success.
    assert_eq!(
        String::from_utf8(Response::error("x", BridgeError::AppUnavailable).to_bytes()).unwrap(),
        r#"{"id":"x","ok":false,"error":"app_unavailable"}"#
    );
    assert_eq!(
        String::from_utf8(Response::success("x", &"pw").to_bytes()).unwrap(),
        r#"{"id":"x","ok":true,"data":"pw"}"#
    );
    let codes: Vec<&str> = BridgeError::ALL.iter().map(|e| e.code()).collect();
    assert_eq!(
        codes,
        [
            "not_paired",
            "pairing_denied",
            "locked",
            "wrong_password",
            "not_found",
            "invalid_request",
            "app_unavailable",
            "internal"
        ]
    );
    // Round trip.
    for r in [
        Response::null("a"),
        Response::error("b", BridgeError::Locked),
        Response::success("c", &json!({"k": [1, 2]})),
    ] {
        let back: Response = serde_json::from_slice(&r.to_bytes()).unwrap();
        assert_eq!(back, r);
    }
}

#[test]
fn data_shapes() {
    same_json(
        &StatusData {
            app_version: "2.0.0".into(),
            paired: true,
            unlocked: false,
            vault_name: None,
        },
        r#"{"appVersion":"2.0.0","paired":true,"unlocked":false,"vaultName":null}"#,
    );
    same_json(
        &PairData {
            client_id: "c".into(),
            token: "t".into(),
        },
        r#"{"clientId":"c","token":"t"}"#,
    );
    same_json(
        &UnlockData {
            vault_name: "Privat".into(),
        },
        r#"{"vaultName":"Privat"}"#,
    );
    same_json(&IdData { id: "i".into() }, r#"{"id":"i"}"#);
    same_json(
        &CopyData {
            remaining: Some(12),
        },
        r#"{"remaining":12}"#,
    );
    same_json(&CopyData { remaining: None }, r#"{"remaining":null}"#);
    same_json(
        &LoginSecret {
            id: "i".into(),
            name: "GitHub".into(),
            username: "me".into(),
            password: "pw".into(),
            totp: Some(TotpCode {
                code: "123456".into(),
                period: 30,
                remaining: 12,
            }),
            uris: vec!["https://github.com".into()],
        },
        r#"{"id":"i","name":"GitHub","username":"me","password":"pw",
            "totp":{"code":"123456","period":30,"remaining":12},"uris":["https://github.com"]}"#,
    );
    same_json(
        &LoginSecret {
            id: "i".into(),
            name: "n".into(),
            username: String::new(),
            password: String::new(),
            totp: None,
            uris: vec![],
        },
        r#"{"id":"i","name":"n","username":"","password":"","totp":null,"uris":[]}"#,
    );
}
