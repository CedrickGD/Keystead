//! Runs the `keystead-cli` binary against a temporary data directory
//! (`KEYSTEAD_DATA_DIR`) with vaults created through keystead-core using cheap
//! KDF parameters.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use keystead_core::model::{ItemType, LoginUri, VaultItem};
use keystead_core::settings::{Language, Settings};
use keystead_core::{KdfParams, VaultStore};

const PASSWORD: &str = "test master password";
const TOTP_SEED: &str = "JBSWY3DPEHPK3PXP";

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(lang: Language) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = Settings {
            language: lang,
            ..Settings::default()
        };
        settings
            .save_to(&dir.path().join("settings.json"))
            .expect("settings");
        Env { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn add_vault(&self, name: &str, items: Vec<VaultItem>) -> String {
        let store = VaultStore::new(self.path());
        let mut vault = store
            .create_vault_with_params(name, PASSWORD, KdfParams::insecure_for_tests())
            .expect("create vault");
        for item in items {
            vault.save_item(item).expect("save item");
        }
        vault.id().to_owned()
    }

    /// Runs the binary with the master password in the environment.
    fn run(&self, args: &[&str]) -> Output {
        self.run_with_password(args, Some(PASSWORD))
    }

    fn run_with_password(&self, args: &[&str], password: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_keystead-cli"));
        cmd.args(args)
            .env("KEYSTEAD_DATA_DIR", self.path())
            // No display: clipboard access fails deterministically.
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::null());
        match password {
            Some(pw) => cmd.env("KEYSTEAD_MASTER_PASSWORD", pw),
            None => cmd.env_remove("KEYSTEAD_MASTER_PASSWORD"),
        };
        cmd.output().expect("run keystead-cli")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn login(name: &str, user: &str, password: &str, uri: &str) -> VaultItem {
    let mut item = VaultItem::new(ItemType::Login, name);
    if let Some(l) = item.login.as_mut() {
        l.username = user.into();
        l.password = password.into();
        if !uri.is_empty() {
            l.uris.push(LoginUri {
                uri: uri.into(),
                ..Default::default()
            });
        }
    }
    item
}

fn sample_items() -> Vec<VaultItem> {
    let mut github = login(
        "GitHub",
        "octocat",
        "gh-secret-pw-123",
        "https://github.com",
    );
    if let Some(l) = github.login.as_mut() {
        l.totp = TOTP_SEED.into();
    }
    github.notes = "line one\nline two".into();
    let mut note = VaultItem::new(ItemType::Note, "WLAN");
    note.notes = "Key: wifi".into();
    vec![
        github,
        login("GitLab", "tanuki", "gl-pw", "gitlab.com"),
        login("Amazon", "me@example.com", "amz-pass", "amazon.de"),
        note,
    ]
}

#[test]
fn vaults_lists_names_and_ids() {
    let env = Env::new(Language::En);
    let empty = env.run(&["vaults"]);
    assert_eq!(empty.status.code(), Some(0));
    assert!(
        stderr(&empty).contains("No vault found"),
        "{}",
        stderr(&empty)
    );

    let a = env.add_vault("Privat", Vec::new());
    let b = env.add_vault("Arbeit", Vec::new());
    let out = env.run(&["vaults"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("NAME"), "{text}");
    assert!(lines[1].starts_with("Arbeit") && lines[1].ends_with(&b));
    assert!(lines[2].starts_with("Privat") && lines[2].ends_with(&a));
}

#[test]
fn list_items_in_german() {
    let env = Env::new(Language::De);
    env.add_vault("Privat", sample_items());
    let out = env.run(&["list"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].starts_with("NAME") && lines[0].contains("TYP"),
        "{text}"
    );
    assert!(lines[1].starts_with("Amazon") && lines[1].contains("me@example.com"));
    assert!(lines[2].starts_with("GitHub") && lines[2].contains("https://github.com"));
    assert!(lines[3].starts_with("GitLab"));
    assert!(lines[4].starts_with("WLAN") && lines[4].contains("Sichere Notiz"));
    assert_eq!(lines.len(), 5);
    // Secrets never appear in the listing.
    assert!(!text.contains("gh-secret-pw-123"));
}

#[test]
fn vault_selection() {
    let env = Env::new(Language::En);
    env.add_vault("Privat", sample_items());
    let work = env.add_vault("Arbeit", vec![login("Jira", "dev", "jira-pw", "")]);

    let ambiguous = env.run(&["list"]);
    assert_eq!(ambiguous.status.code(), Some(2));
    assert!(
        stderr(&ambiguous).contains("--vault"),
        "{}",
        stderr(&ambiguous)
    );

    let out = env.run(&["list", "--vault", "arbeit"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("Jira"));
    // Global option before the subcommand, vault given by id.
    let out = env.run(&["--vault", &work, "get", "jira"]);
    assert_eq!(stdout(&out), "jira-pw\n");

    let missing = env.run(&["list", "--vault", "Nope"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        stderr(&missing).contains("“Arbeit”"),
        "{}",
        stderr(&missing)
    );

    // The last used vault (settings.lastVaultId) is the default.
    let mut settings = Settings::load_from(&env.path().join("settings.json"));
    settings.last_vault_id = Some(work);
    settings.save_to(&env.path().join("settings.json")).unwrap();
    let out = env.run(&["get", "Jira"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn wrong_password_and_missing_vault() {
    let env = Env::new(Language::De);
    let none = env.run(&["list"]);
    assert_eq!(none.status.code(), Some(1));
    assert!(stderr(&none).contains("Kein Tresor gefunden"));

    env.add_vault("Privat", sample_items());
    let out = env.run_with_password(&["list"], Some("falsch"));
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Falsches Master-Passwort"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty());
}

#[test]
fn get_fields() {
    let env = Env::new(Language::En);
    env.add_vault("Privat", sample_items());
    let get = |args: &[&str]| {
        let out = env.run(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr(&out));
        stdout(&out)
    };
    assert_eq!(get(&["get", "github"]), "gh-secret-pw-123\n");
    assert_eq!(get(&["get", "GitHub", "--field", "username"]), "octocat\n");
    assert_eq!(
        get(&["get", "GitHub", "--field", "uri"]),
        "https://github.com\n"
    );
    assert_eq!(
        get(&["get", "GitHub", "--field", "notes"]),
        "line one\nline two\n"
    );
    // Search terms (username) find a unique item.
    assert_eq!(get(&["get", "tanuki"]), "gl-pw\n");
    let code = get(&["get", "github", "--field", "totp"]);
    assert_eq!(code.trim().len(), 6, "{code}");
    assert!(code.trim().chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn get_errors() {
    let env = Env::new(Language::En);
    env.add_vault("Privat", sample_items());

    let ambiguous = env.run(&["get", "git"]);
    assert_eq!(ambiguous.status.code(), Some(1));
    let err = stderr(&ambiguous);
    assert!(err.contains("Several items match"), "{err}");
    assert!(err.contains("GitHub") && err.contains("GitLab"));
    assert!(stdout(&ambiguous).is_empty());

    let missing = env.run(&["get", "nothing-like-this"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("No item found"));

    let empty = env.run(&["get", "WLAN"]);
    assert_eq!(empty.status.code(), Some(1));
    assert!(
        stderr(&empty).contains("has no value"),
        "{}",
        stderr(&empty)
    );

    let bad_field = env.run(&["get", "GitHub", "--field", "pin"]);
    assert_eq!(bad_field.status.code(), Some(2));
}

#[test]
fn copy_failure_never_prints_the_secret() {
    let env = Env::new(Language::En);
    env.add_vault("Privat", sample_items());
    // No display server in the child: the clipboard is unavailable.
    let out = env.run(&["get", "GitHub", "--copy"]);
    if out.status.code() == Some(0) {
        // A platform clipboard that works without a display (Windows/macOS).
        assert!(stderr(&out).contains("copied"));
    } else {
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("Clipboard not available"),
            "{}",
            stderr(&out)
        );
    }
    assert!(!stdout(&out).contains("gh-secret-pw-123"));
    assert!(!stderr(&out).contains("gh-secret-pw-123"));
}

#[test]
fn generate_passwords() {
    let env = Env::new(Language::En);
    let run = |args: &[&str]| env.run_with_password(args, None);

    let out = run(&["generate"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim_end().chars().count(), 20);

    let out = run(&["generate", "--length", "32", "--no-symbols"]);
    let pw = stdout(&out);
    assert_eq!(pw.trim_end().len(), 32);
    assert!(
        pw.trim_end().chars().all(|c| c.is_ascii_alphanumeric()),
        "{pw}"
    );

    let out = run(&["generate", "--passphrase", "--words", "4"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).trim_end().split('-').count() >= 4);
    let out = run(&["generate", "--words", "6"]);
    assert_eq!(out.status.code(), Some(0));

    for bad in [
        &["generate", "--length", "4"][..],
        &["generate", "--length", "129"],
        &["generate", "--words", "2"],
        &["generate", "--length", "12", "--passphrase"],
        &["generate", "--no-symbols", "--words", "5"],
    ] {
        let out = run(bad);
        assert_eq!(out.status.code(), Some(2), "{bad:?}");
        assert!(stdout(&out).is_empty());
    }
}

#[test]
fn help_version_and_cli_flag() {
    let env = Env::new(Language::De);
    let out = env.run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = stdout(&out);
    assert!(help.contains("Aufruf:"), "{help}");
    assert!(help.contains("KEYSTEAD_MASTER_PASSWORD"));
    assert!(help.contains("UNSICHER"));

    let out = env.run(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));

    // `Keystead --cli …` passes the flag through: it is ignored.
    env.add_vault("Privat", sample_items());
    let out = env.run(&["--cli", "get", "amazon"]);
    assert_eq!(stdout(&out), "amz-pass\n", "{}", stderr(&out));
    let out = env.run(&["cli", "--help"]);
    assert!(
        stdout(&out).contains("--cli"),
        "usage shows the real invocation"
    );

    let out = env.run(&["--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!stderr(&out).is_empty());
}

#[test]
fn interactive_mode_requires_a_terminal() {
    let env = Env::new(Language::En);
    let out = env.run(&[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("needs a terminal"),
        "{}",
        stderr(&out)
    );
}
