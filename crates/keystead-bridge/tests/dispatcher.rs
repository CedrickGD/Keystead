//! Dispatcher behaviour with a fake backend: pairing, lock state, token
//! checks, rate limiting and every request type.

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use common::*;
use serde_json::json;
use keystead_bridge::protocol::{LoginSecret, PairData, StatusData};
use keystead_bridge::{BridgeError, ClientStore, Dispatcher, DispatcherConfig, Response};
use keystead_core::model::ItemSummary;
use keystead_core::totp::TotpCode;

struct Fixture {
    dispatcher: Arc<Dispatcher>,
    backend: Arc<FakeBackend>,
    dir: tempfile::TempDir,
}

fn fixture(config: DispatcherConfig) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new());
    let store = ClientStore::open(dir.path().join("bridge-clients.json")).unwrap();
    let dispatcher = Arc::new(Dispatcher::with_config(backend.clone(), store, config));
    Fixture {
        dispatcher,
        backend,
        dir,
    }
}

fn default_fixture() -> Fixture {
    fixture(DispatcherConfig::default())
}

fn err(r: &Response) -> BridgeError {
    r.error_code()
        .unwrap_or_else(|| panic!("expected an error, got {:?}", r.data))
}

/// Runs `pair` while another thread plays the user and answers `approve`.
fn pair_with(f: &Fixture, name: &str, approve: bool) -> Response {
    thread::scope(|s| {
        s.spawn(|| {
            let req = f
                .backend
                .next_pairing(Duration::from_secs(10))
                .expect("pairing request reaches the app");
            assert_eq!(req.client_name, name);
            assert_eq!(req.code, "123456");
            assert!(f.dispatcher.respond_pairing(&req.request_id, approve));
        });
        call(
            f.dispatcher.as_ref(),
            json!({"id": "p1", "type": "pair", "clientName": name, "code": "123456"}),
        )
    })
}

fn paired(f: &Fixture) -> (String, String) {
    let r = pair_with(f, "Chrome – Test", true);
    assert!(r.ok, "{r:?}");
    let data: PairData = r.data_as().unwrap();
    (data.client_id, data.token)
}

#[test]
fn status_without_pairing() {
    let f = default_fixture();
    f.backend.set_unlocked(true);
    let r = call(f.dispatcher.as_ref(), json!({"id": "s", "type": "status"}));
    assert_eq!(r.id, "s");
    let status: StatusData = r.data_as().unwrap();
    assert_eq!(
        status,
        StatusData {
            app_version: "2.0.0-test".into(),
            paired: false,
            unlocked: true,
            // Not revealed to unpaired callers.
            vault_name: None,
        }
    );
    // Wrong credentials are just "not paired" for status.
    let r = call(
        f.dispatcher.as_ref(),
        with_creds(json!({"id": "s", "type": "status"}), "nope", "nope"),
    );
    assert!(!r.data_as::<StatusData>().unwrap().paired);
}

#[test]
fn everything_but_status_pair_focus_needs_pairing() {
    let f = default_fixture();
    f.backend.set_unlocked(true);
    let requests = [
        json!({"id": "1", "type": "unlock", "password": PASSWORD}),
        json!({"id": "2", "type": "lock"}),
        json!({"id": "3", "type": "logins_for_url", "url": "https://github.com"}),
        json!({"id": "4", "type": "search", "query": "git"}),
        json!({"id": "5", "type": "get_login", "itemId": GITHUB_ID}),
        json!({"id": "6", "type": "get_totp", "itemId": GITHUB_ID}),
        json!({"id": "7", "type": "generate_password"}),
        json!({"id": "8", "type": "save_login", "name": "n", "url": "u", "username": "x", "password": "y"}),
        json!({"id": "9", "type": "update_password", "itemId": GITHUB_ID, "password": "y"}),
    ];
    for req in requests {
        let r = call(f.dispatcher.as_ref(), req.clone());
        assert_eq!(err(&r), BridgeError::NotPaired, "{req}");
        assert_eq!(r.id, req["id"]);
    }
    assert_eq!(f.backend.unlock_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.backend.lock_calls.load(Ordering::SeqCst), 0);
    // focus_app works without pairing.
    let r = call(
        f.dispatcher.as_ref(),
        json!({"id": "f", "type": "focus_app"}),
    );
    assert_eq!(r, Response::null("f"));
    assert_eq!(f.backend.focus_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn pairing_approved_issues_working_credentials() {
    let f = default_fixture();
    let (client_id, token) = paired(&f);
    assert_eq!(token.len(), 43);

    let clients = f.dispatcher.clients();
    assert_eq!(clients.len(), 1);
    assert_eq!(clients[0].id, client_id);
    assert_eq!(clients[0].name, "Chrome – Test");
    // Persisted (hash only).
    let file = std::fs::read_to_string(f.dir.path().join("bridge-clients.json")).unwrap();
    assert!(file.contains(&client_id) && !file.contains(&token));

    let r = call(
        f.dispatcher.as_ref(),
        with_creds(json!({"id": "s", "type": "status"}), &client_id, &token),
    );
    let status: StatusData = r.data_as().unwrap();
    assert!(status.paired && !status.unlocked && status.vault_name.is_none());

    // lastSeenAt is touched by authenticated requests.
    let before = f.dispatcher.clients()[0].last_seen_at;
    thread::sleep(Duration::from_millis(5));
    call(
        f.dispatcher.as_ref(),
        with_creds(json!({"id": "s", "type": "status"}), &client_id, &token),
    );
    assert!(f.dispatcher.clients()[0].last_seen_at > before);
}

#[test]
fn pairing_denied() {
    let f = default_fixture();
    let r = pair_with(&f, "Edge", false);
    assert_eq!(err(&r), BridgeError::PairingDenied);
    assert_eq!(r.id, "p1");
    assert!(f.dispatcher.clients().is_empty());
}

#[test]
fn pairing_times_out() {
    let f = fixture(DispatcherConfig {
        pairing_timeout: Duration::from_millis(150),
        ..DispatcherConfig::default()
    });
    let r = call(
        f.dispatcher.as_ref(),
        json!({"id": "p", "type": "pair", "clientName": "Brave", "code": "654321"}),
    );
    assert_eq!(err(&r), BridgeError::PairingDenied);
    let req = f
        .backend
        .next_pairing(Duration::ZERO)
        .expect("the app was asked");
    // A late answer is rejected and creates no client.
    assert!(!f.dispatcher.respond_pairing(&req.request_id, true));
    assert!(f.dispatcher.clients().is_empty());
    assert!(f.dispatcher.pending_pairings().is_empty());
    assert!(!f.dispatcher.respond_pairing("unknown-request", true));
}

#[test]
fn pending_pairing_is_listed_and_superseded_by_same_client() {
    let f = default_fixture();
    thread::scope(|s| {
        let first = s.spawn(|| {
            call(
                f.dispatcher.as_ref(),
                json!({"id": "a", "type": "pair", "clientName": "Chrome", "code": "111111"}),
            )
        });
        let req1 = f.backend.next_pairing(Duration::from_secs(10)).unwrap();
        assert_eq!(f.dispatcher.pending_pairings(), vec![req1.clone()]);

        let second = s.spawn(|| {
            call(
                f.dispatcher.as_ref(),
                json!({"id": "b", "type": "pair", "clientName": "Chrome", "code": "222222"}),
            )
        });
        let req2 = f.backend.next_pairing(Duration::from_secs(10)).unwrap();
        assert_eq!(req2.code, "222222");
        // The older request was answered "denied" right away.
        assert_eq!(err(&first.join().unwrap()), BridgeError::PairingDenied);
        assert!(!f.dispatcher.respond_pairing(&req1.request_id, true));
        assert!(f.dispatcher.respond_pairing(&req2.request_id, true));
        let r = second.join().unwrap();
        assert!(r.ok && r.id == "b", "{r:?}");
    });
    assert_eq!(f.dispatcher.clients().len(), 1);
}

#[test]
fn invalid_pair_requests() {
    let f = default_fixture();
    for req in [
        json!({"id": "1", "type": "pair", "clientName": "x", "code": "12345"}),
        json!({"id": "2", "type": "pair", "clientName": "x", "code": "abcdef"}),
        json!({"id": "3", "type": "pair", "clientName": "   ", "code": "123456"}),
        json!({"id": "4", "type": "pair", "clientName": "x".repeat(101), "code": "123456"}),
        json!({"id": "5", "type": "pair", "code": "123456"}),
        json!({"id": "6", "type": "pair", "clientName": "x", "code": 123456}),
    ] {
        assert_eq!(
            err(&call(f.dispatcher.as_ref(), req.clone())),
            BridgeError::InvalidRequest,
            "{req}"
        );
    }
    assert!(
        f.backend.next_pairing(Duration::ZERO).is_none(),
        "app must not be asked"
    );
}

#[test]
fn token_mismatch_and_revocation() {
    let f = default_fixture();
    let (client_id, token) = paired(&f);
    let lock = json!({"id": "l", "type": "lock"});

    let wrong_token = format!("{}A", &token[..42]);
    for (id, tok) in [
        (client_id.as_str(), wrong_token.as_str()),
        (client_id.as_str(), ""),
        ("other-client", token.as_str()),
        ("", ""),
    ] {
        let r = call(f.dispatcher.as_ref(), with_creds(lock.clone(), id, tok));
        assert_eq!(err(&r), BridgeError::NotPaired);
    }
    // Token without client id (and vice versa).
    let mut only_token = lock.clone();
    only_token["token"] = json!(token);
    assert_eq!(
        err(&call(f.dispatcher.as_ref(), only_token)),
        BridgeError::NotPaired
    );

    let r = call(
        f.dispatcher.as_ref(),
        with_creds(lock.clone(), &client_id, &token),
    );
    assert_eq!(r, Response::null("l"));

    assert!(f.dispatcher.revoke(&client_id).unwrap());
    assert!(!f.dispatcher.revoke(&client_id).unwrap());
    let r = call(f.dispatcher.as_ref(), with_creds(lock, &client_id, &token));
    assert_eq!(err(&r), BridgeError::NotPaired);
}

#[test]
fn locked_vault_and_unlock_flow() {
    let f = default_fixture();
    let (cid, tok) = paired(&f);
    let c = |v| with_creds(v, &cid, &tok);
    let d = f.dispatcher.as_ref();

    for req in [
        json!({"id": "1", "type": "logins_for_url", "url": "https://github.com"}),
        json!({"id": "2", "type": "search", "query": "git"}),
        json!({"id": "3", "type": "get_login", "itemId": GITHUB_ID}),
        json!({"id": "4", "type": "get_totp", "itemId": GITHUB_ID}),
        json!({"id": "5", "type": "save_login", "name": "n", "url": "u", "username": "x", "password": "y"}),
        json!({"id": "6", "type": "update_password", "itemId": GITHUB_ID, "password": "y"}),
    ] {
        assert_eq!(err(&call(d, c(req.clone()))), BridgeError::Locked, "{req}");
    }
    // generate_password works while locked.
    let r = call(d, c(json!({"id": "g", "type": "generate_password"})));
    assert_eq!(r.data_as::<String>().unwrap().chars().count(), 20);

    let r = call(
        d,
        c(json!({"id": "u", "type": "unlock", "password": "nope"})),
    );
    assert_eq!(err(&r), BridgeError::WrongPassword);
    let r = call(
        d,
        c(json!({"id": "u", "type": "unlock", "password": PASSWORD})),
    );
    assert_eq!(r.data, json!({"vaultName": VAULT_NAME}));

    let status: StatusData = call(d, c(json!({"id": "s", "type": "status"})))
        .data_as()
        .unwrap();
    assert!(status.unlocked && status.paired);
    assert_eq!(status.vault_name.as_deref(), Some(VAULT_NAME));

    let r = call(d, c(json!({"id": "l", "type": "lock"})));
    assert_eq!(r, Response::null("l"));
    let r = call(
        d,
        c(json!({"id": "x", "type": "get_login", "itemId": GITHUB_ID})),
    );
    assert_eq!(err(&r), BridgeError::Locked);
}

#[test]
fn unlock_is_rate_limited_after_five_failures() {
    let f = fixture(DispatcherConfig {
        unlock_lockout: Duration::from_millis(400),
        ..DispatcherConfig::default()
    });
    let (cid, tok) = paired(&f);
    let unlock = |pw: &str| {
        call(
            f.dispatcher.as_ref(),
            with_creds(
                json!({"id": "u", "type": "unlock", "password": pw}),
                &cid,
                &tok,
            ),
        )
    };

    // Four failures, then a success resets the counter.
    for _ in 0..4 {
        assert_eq!(err(&unlock("wrong")), BridgeError::WrongPassword);
    }
    assert!(unlock(PASSWORD).ok);
    f.backend.set_unlocked(false);

    for _ in 0..5 {
        assert_eq!(err(&unlock("wrong")), BridgeError::WrongPassword);
    }
    let calls = f.backend.unlock_calls.load(Ordering::SeqCst);
    // Even the right password is refused during the lockout, without
    // reaching the backend.
    assert_eq!(err(&unlock(PASSWORD)), BridgeError::WrongPassword);
    assert_eq!(f.backend.unlock_calls.load(Ordering::SeqCst), calls);

    thread::sleep(Duration::from_millis(450));
    // Another failure after the lockout suspends again immediately.
    assert_eq!(err(&unlock("wrong")), BridgeError::WrongPassword);
    assert_eq!(err(&unlock(PASSWORD)), BridgeError::WrongPassword);
    thread::sleep(Duration::from_millis(450));
    let r = unlock(PASSWORD);
    assert!(r.ok, "{r:?}");
    // Counter reset by the success.
    f.backend.set_unlocked(false);
    assert_eq!(err(&unlock("wrong")), BridgeError::WrongPassword);
    assert!(unlock(PASSWORD).ok);
}

#[test]
fn concurrent_unlock_attempts_cannot_bypass_the_limit() {
    let f = fixture(DispatcherConfig {
        unlock_lockout: Duration::from_secs(60),
        ..DispatcherConfig::default()
    });
    let (cid, tok) = paired(&f);
    thread::scope(|s| {
        for _ in 0..20 {
            s.spawn(|| {
                call(
                    f.dispatcher.as_ref(),
                    with_creds(
                        json!({"id": "u", "type": "unlock", "password": "wrong"}),
                        &cid,
                        &tok,
                    ),
                )
            });
        }
    });
    assert_eq!(f.backend.unlock_calls.load(Ordering::SeqCst), 5);
}

#[test]
fn unlocked_operations() {
    let f = default_fixture();
    let (cid, tok) = paired(&f);
    f.backend.set_unlocked(true);
    let d = f.dispatcher.as_ref();
    let c = |v| with_creds(v, &cid, &tok);

    let r = call(
        d,
        c(json!({"id": "1", "type": "logins_for_url", "url": "https://gist.github.com/x"})),
    );
    let logins: Vec<ItemSummary> = r.data_as().unwrap();
    assert_eq!(logins.len(), 1);
    assert_eq!(logins[0].id, GITHUB_ID);
    assert!(logins[0].has_totp);
    // JSON shape of ItemSummary as in the contract.
    assert_eq!(r.data[0]["type"], "login");
    assert_eq!(r.data[0]["hasTotp"], true);
    assert_eq!(r.data[0]["subtitle"], "octocat");

    let r = call(d, c(json!({"id": "2", "type": "search", "query": "many"})));
    assert_eq!(
        r.data.as_array().unwrap().len(),
        50,
        "search is capped at 50"
    );

    let r = call(
        d,
        c(json!({"id": "3", "type": "get_login", "itemId": GITHUB_ID})),
    );
    let secret: LoginSecret = r.data_as().unwrap();
    assert_eq!(secret.username, "octocat");
    assert_eq!(secret.password, "gh-secret");
    assert_eq!(secret.uris, vec!["https://github.com"]);
    let totp = secret.totp.unwrap();
    assert_eq!(totp.code.len(), 6);
    let keys: Vec<_> = r.data.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys.len(), 6, "{keys:?}");
    let r = call(
        d,
        c(json!({"id": "3b", "type": "get_login", "itemId": EXAMPLE_ID})),
    );
    assert!(r.data["totp"].is_null());
    let r = call(
        d,
        c(json!({"id": "3c", "type": "get_login", "itemId": NOTE_ID})),
    );
    assert_eq!(err(&r), BridgeError::NotFound);

    let r = call(
        d,
        c(json!({"id": "4", "type": "get_totp", "itemId": GITHUB_ID})),
    );
    let code: TotpCode = r.data_as().unwrap();
    assert!(code.period == 30 && code.remaining >= 1 && code.remaining <= 30);
    let r = call(
        d,
        c(json!({"id": "4b", "type": "get_totp", "itemId": EXAMPLE_ID})),
    );
    assert_eq!(err(&r), BridgeError::NotFound);

    let r = call(
        d,
        c(
            json!({"id": "5", "type": "save_login", "name": "Shop", "url": "https://shop.example.org",
                 "username": "buyer", "password": "pw1"}),
        ),
    );
    let new_id = r.data["id"].as_str().unwrap().to_owned();
    let r = call(
        d,
        c(json!({"id": "6", "type": "update_password", "itemId": new_id, "password": "pw2"})),
    );
    assert_eq!(r.data, json!({"id": new_id}));
    let r = call(
        d,
        c(json!({"id": "7", "type": "get_login", "itemId": new_id})),
    );
    assert_eq!(r.data["password"], "pw2");
    assert_eq!(r.data["username"], "buyer");
    let r = call(
        d,
        c(json!({"id": "8", "type": "update_password", "itemId": "missing", "password": "x"})),
    );
    assert_eq!(err(&r), BridgeError::NotFound);
}

#[test]
fn generate_password_options() {
    let f = default_fixture();
    let (cid, tok) = paired(&f);
    let d = f.dispatcher.as_ref();
    let c = |v| with_creds(v, &cid, &tok);

    let r = call(
        d,
        c(
            json!({"id": "1", "type": "generate_password", "options": {"length": 32, "symbols": false}}),
        ),
    );
    let pw = r.data_as::<String>().unwrap();
    assert_eq!(pw.len(), 32);
    assert!(pw.chars().all(|ch| ch.is_ascii_alphanumeric()));

    let r = call(
        d,
        c(json!({"id": "2", "type": "generate_password", "options": null})),
    );
    assert_eq!(r.data_as::<String>().unwrap().len(), 20);

    let r = call(
        d,
        c(
            json!({"id": "3", "type": "generate_password", "options": {"kind": "passphrase", "words": 4, "separator": "_"}}),
        ),
    );
    assert_eq!(r.data_as::<String>().unwrap().split('_').count(), 4);

    let r = call(
        d,
        c(json!({"id": "4", "type": "generate_password", "options": {"length": 2}})),
    );
    assert_eq!(err(&r), BridgeError::InvalidRequest);
    let r = call(
        d,
        c(json!({"id": "5", "type": "generate_password", "options": {"length": "long"}})),
    );
    assert_eq!(err(&r), BridgeError::InvalidRequest);
}

#[test]
fn activity_is_reported_for_user_actions_only() {
    let f = default_fixture();
    let (cid, tok) = paired(&f);
    f.backend.set_unlocked(true);
    let d = f.dispatcher.as_ref();
    let c = |v| with_creds(v, &cid, &tok);

    call(d, c(json!({"id": "1", "type": "status"})));
    call(
        d,
        c(json!({"id": "2", "type": "logins_for_url", "url": "https://github.com"})),
    );
    call(d, c(json!({"id": "3", "type": "focus_app"})));
    assert_eq!(f.backend.activity.load(Ordering::SeqCst), 0);
    call(
        d,
        c(json!({"id": "4", "type": "get_login", "itemId": GITHUB_ID})),
    );
    call(d, c(json!({"id": "5", "type": "search", "query": "x"})));
    assert_eq!(f.backend.activity.load(Ordering::SeqCst), 2);
    // Failed requests do not count.
    call(
        d,
        c(json!({"id": "6", "type": "get_login", "itemId": "missing"})),
    );
    assert_eq!(f.backend.activity.load(Ordering::SeqCst), 2);
}

#[test]
fn client_store_survives_dispatcher_restart() {
    let f = default_fixture();
    let (cid, tok) = paired(&f);
    let path = f.dir.path().join("bridge-clients.json");
    drop(f.dispatcher);
    let store = ClientStore::open(&path).unwrap();
    let dispatcher = Dispatcher::new(f.backend.clone(), store);
    let r = call(
        &dispatcher,
        with_creds(json!({"id": "s", "type": "status"}), &cid, &tok),
    );
    assert!(r.data_as::<StatusData>().unwrap().paired);
}
