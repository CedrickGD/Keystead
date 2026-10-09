//! Command-line parsing and the non-interactive subcommands.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use clap::{Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use keystead_core::generator::{self, GeneratorKind, GeneratorOptions};
use keystead_core::model::{ItemType, VaultInfo, VaultItem};
use keystead_core::settings::Settings;
use keystead_core::{clipboard, paths, totp, UnlockedVault, VaultStore};
use zeroize::Zeroizing;

use crate::app::{clipboard_detail, first_url, password_of, username_of, App};
use crate::i18n::{Lang, M};
use crate::input::str_width;
use crate::platform::SystemPlatform;
use crate::term;

/// Environment variable with the master password for scripts (insecure).
pub const MASTER_PASSWORD_ENV: &str = "KEYSTEAD_MASTER_PASSWORD";

#[derive(Parser, Debug)]
#[command(name = "keystead-cli", version, disable_help_subcommand = true)]
struct Cli {
    #[arg(long, global = true, value_name = "NAME")]
    vault: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    List,
    Get {
        query: String,
        #[arg(long, value_enum, default_value_t = Field::Password)]
        field: Field,
        #[arg(long)]
        copy: bool,
    },
    Generate {
        #[arg(long, value_name = "N",
              value_parser = clap::value_parser!(u32).range(5..=128),
              conflicts_with_all = ["passphrase", "words"])]
        length: Option<u32>,
        #[arg(long)]
        passphrase: bool,
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(3..=20))]
        words: Option<u32>,
        #[arg(long, conflicts_with_all = ["passphrase", "words"])]
        no_symbols: bool,
        #[arg(long)]
        copy: bool,
    },
    Vaults,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Password,
    Username,
    Totp,
    Notes,
    Uri,
}

/// An error with its exit code (1 = failure, 2 = invalid usage).
#[derive(Debug)]
struct CliError {
    code: i32,
    message: String,
}

impl CliError {
    fn failure(message: impl Into<String>) -> Self {
        CliError {
            code: 1,
            message: message.into(),
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        CliError {
            code: 2,
            message: message.into(),
        }
    }
}

/// Writing to stdout; a closed pipe (`keystead-cli list | head`) is not an
/// error worth reporting.
fn out_err(e: io::Error) -> CliError {
    if e.kind() == io::ErrorKind::BrokenPipe {
        CliError {
            code: 0,
            message: String::new(),
        }
    } else {
        CliError::failure(e.to_string())
    }
}

fn help_arg(lang: Lang) -> Arg {
    Arg::new("help")
        .short('h')
        .long("help")
        .action(ArgAction::Help)
        .help(lang.t(M::CliHelp))
        .help_heading(lang.t(M::CliOptions))
}

fn template(lang: Lang) -> String {
    format!(
        "{{about-with-newline}}\n{}: {{usage}}\n\n{{all-args}}{{after-help}}",
        lang.t(M::CliUsage)
    )
}

fn localize_sub(cmd: clap::Command, lang: Lang, about: M) -> clap::Command {
    cmd.about(lang.t(about))
        .help_template(template(lang))
        .disable_help_flag(true)
        .arg(help_arg(lang))
}

/// The clap command with German or English help texts.
fn command(lang: Lang) -> clap::Command {
    let t = |m| lang.t(m);
    let opt = |a: Arg, m: M| a.help(lang.t(m)).help_heading(lang.t(M::CliOptions));
    Cli::command()
        .about(t(M::CliAbout))
        .after_help(t(M::CliAfterHelp))
        .help_template(template(lang))
        .subcommand_help_heading(t(M::CliCommands))
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(help_arg(lang))
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help(t(M::CliVersion))
                .help_heading(t(M::CliOptions)),
        )
        .mut_arg("vault", |a| opt(a, M::CliVaultArg))
        .mut_subcommand("list", |c| localize_sub(c, lang, M::CliList))
        .mut_subcommand("vaults", |c| localize_sub(c, lang, M::CliVaults))
        .mut_subcommand("get", |c| {
            localize_sub(c, lang, M::CliGet)
                .mut_arg("query", |a| {
                    a.help(lang.t(M::CliGetQuery))
                        .help_heading(lang.t(M::CliArguments))
                })
                .mut_arg("field", |a| {
                    // The values are listed in the (translated) help text.
                    opt(a, M::CliGetField)
                        .hide_default_value(true)
                        .hide_possible_values(true)
                })
                .mut_arg("copy", |a| opt(a, M::CliCopy))
        })
        .mut_subcommand("generate", |c| {
            localize_sub(c, lang, M::CliGenerate)
                .mut_arg("length", |a| opt(a, M::CliGenLength))
                .mut_arg("passphrase", |a| opt(a, M::CliGenPassphrase))
                .mut_arg("words", |a| opt(a, M::CliGenWords))
                .mut_arg("no_symbols", |a| opt(a, M::CliGenNoSymbols))
                .mut_arg("copy", |a| opt(a, M::CliCopy))
        })
}

/// Parses `args` and runs the interactive UI or a subcommand.
pub fn main(args: Vec<String>, via_cli_flag: bool) -> i32 {
    let settings = Settings::load();
    let lang = Lang::from(settings.language);
    let mut cmd = command(lang);
    if via_cli_flag {
        // `Keystead --cli`: show the real invocation in the usage line.
        let exe = args
            .first()
            .map(Path::new)
            .and_then(Path::file_name)
            .map_or_else(|| "Keystead".to_owned(), |f| f.to_string_lossy().into_owned());
        cmd = cmd.bin_name(format!("{exe} --cli"));
    }
    let matches = match cmd.try_get_matches_from(&args) {
        Ok(m) => m,
        Err(e) => {
            // Help/version go to stdout with code 0, errors to stderr with 2.
            let _ = e.print();
            return e.exit_code();
        }
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return 2;
        }
    };
    match cli.command {
        None => interactive(lang, cli.vault.as_deref()),
        Some(command) => {
            let stdout = io::stdout();
            let mut out = stdout.lock();
            match execute(command, cli.vault.as_deref(), &settings, lang, &mut out) {
                Ok(()) => 0,
                Err(e) => {
                    if !e.message.is_empty() {
                        eprintln!("{}: {}", lang.t(M::CliError), e.message);
                    }
                    e.code
                }
            }
        }
    }
}

fn interactive(lang: Lang, vault_hint: Option<&str>) -> i32 {
    if !io::stdout().is_terminal() {
        eprintln!("{}", lang.t(M::CliNeedsTerminal));
        return 1;
    }
    let store = match VaultStore::open_default() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {}", lang.t(M::CliError), lang.error(&e));
            return 1;
        }
    };
    let settings = Settings::load_from(&store.root().join("settings.json"));
    let mut app = App::new(store, settings, Box::new(SystemPlatform), vault_hint);
    let result = term::run(&mut app);
    app.shutdown();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!(
                "{}",
                lang.tf(M::CliTerminalError, &[("err", &e.to_string())])
            );
            1
        }
    }
}

fn execute(
    command: Command,
    vault_arg: Option<&str>,
    settings: &Settings,
    lang: Lang,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let store = VaultStore::new(paths::data_dir());
    match command {
        Command::Vaults => cmd_vaults(&store, lang, out),
        Command::List => {
            let vault = open_vault(&store, vault_arg, settings, lang)?;
            cmd_list(&vault, lang, out)
        }
        Command::Get { query, field, copy } => {
            let vault = open_vault(&store, vault_arg, settings, lang)?;
            cmd_get(&vault, &query, field, copy, settings, lang, out)
        }
        Command::Generate {
            length,
            passphrase,
            words,
            no_symbols,
            copy,
        } => {
            let mut opts = GeneratorOptions::default();
            if passphrase || words.is_some() {
                opts.kind = GeneratorKind::Passphrase;
            }
            if let Some(w) = words {
                opts.words = w;
            }
            if let Some(l) = length {
                opts.length = l;
            }
            if no_symbols {
                opts.symbols = false;
            }
            let value = Zeroizing::new(
                generator::generate(&opts).map_err(|e| CliError::usage(lang.error(&e)))?,
            );
            if copy {
                copy_and_wait(
                    &value,
                    true,
                    settings.clipboard_clear_seconds,
                    lang.t(M::WhatPassword),
                    lang,
                )
            } else {
                writeln!(out, "{}", value.as_str()).map_err(out_err)
            }
        }
    }
}

/// Replaces control characters so that vault content cannot inject
/// terminal escape sequences into listings.
fn printable(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Writes left-aligned columns (each at most `max` wide for alignment;
/// longer values are not cut).
fn write_table(out: &mut dyn Write, rows: &[Vec<String>], max: usize) -> io::Result<()> {
    let cols = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .map(|r| str_width(&r[c]))
                .max()
                .unwrap_or(0)
                .min(max)
        })
        .collect();
    for row in rows {
        let mut line = String::new();
        for (c, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if c + 1 < row.len() {
                let pad = widths[c].saturating_sub(str_width(cell)) + 2;
                line.push_str(&" ".repeat(pad));
            }
        }
        writeln!(out, "{}", line.trim_end())?;
    }
    Ok(())
}

fn vault_list_text(vaults: &[VaultInfo], lang: Lang) -> String {
    vaults
        .iter()
        .map(|v| lang.quote(&printable(&v.name)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Picks the vault: `--vault` (id or name), the only vault, or the last
/// used one.
fn choose_vault(
    vaults: &[VaultInfo],
    arg: Option<&str>,
    last_used: Option<&str>,
    data_dir: &Path,
    lang: Lang,
) -> Result<VaultInfo, CliError> {
    if vaults.is_empty() {
        return Err(CliError::failure(
            lang.tf(M::CliNoVaults, &[("dir", &data_dir.display().to_string())]),
        ));
    }
    if let Some(query) = arg {
        let query = query.trim();
        if let Some(v) = vaults.iter().find(|v| v.id == query) {
            return Ok(v.clone());
        }
        let wanted = query.to_lowercase();
        let by_name: Vec<&VaultInfo> = vaults
            .iter()
            .filter(|v| v.name.trim().to_lowercase() == wanted)
            .collect();
        return match by_name.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(CliError::failure(lang.tf(
                M::CliVaultNotFound,
                &[
                    ("name", &printable(query)),
                    ("list", &vault_list_text(vaults, lang)),
                ],
            ))),
            _ => Err(CliError::usage(
                lang.tf(M::CliVaultAmbiguous, &[("name", &printable(query))]),
            )),
        };
    }
    if let [only] = vaults {
        return Ok(only.clone());
    }
    if let Some(v) = last_used.and_then(|id| vaults.iter().find(|v| v.id == id)) {
        return Ok(v.clone());
    }
    Err(CliError::usage(lang.tf(
        M::CliChooseVault,
        &[("list", &vault_list_text(vaults, lang))],
    )))
}

fn master_password(vault: &VaultInfo, lang: Lang) -> Result<Zeroizing<String>, CliError> {
    if let Some(pw) = std::env::var_os(MASTER_PASSWORD_ENV).filter(|v| !v.is_empty()) {
        return Ok(Zeroizing::new(pw.to_string_lossy().into_owned()));
    }
    let prompt = lang.tf(M::CliPasswordPrompt, &[("name", &printable(&vault.name))]);
    rpassword::prompt_password(prompt)
        .map(Zeroizing::new)
        .map_err(|e| CliError::failure(lang.tf(M::CliNoTty, &[("err", &e.to_string())])))
}

fn open_vault(
    store: &VaultStore,
    arg: Option<&str>,
    settings: &Settings,
    lang: Lang,
) -> Result<UnlockedVault, CliError> {
    let vaults = store
        .list_vaults()
        .map_err(|e| CliError::failure(lang.error(&e)))?;
    let info = choose_vault(
        &vaults,
        arg,
        settings.last_vault_id.as_deref(),
        store.root(),
        lang,
    )?;
    let password = master_password(&info, lang)?;
    store
        .unlock(&info.id, &password)
        .map_err(|e| CliError::failure(lang.error(&e)))
}

fn type_name(lang: Lang, t: ItemType) -> &'static str {
    lang.t(match t {
        ItemType::Login => M::TypeLogin,
        ItemType::Card => M::TypeCard,
        ItemType::Identity => M::TypeIdentity,
        ItemType::Note => M::TypeNote,
    })
}

fn cmd_vaults(store: &VaultStore, lang: Lang, out: &mut dyn Write) -> Result<(), CliError> {
    let vaults = store
        .list_vaults()
        .map_err(|e| CliError::failure(lang.error(&e)))?;
    if vaults.is_empty() {
        eprintln!(
            "{}",
            lang.tf(
                M::CliNoVaults,
                &[("dir", &store.root().display().to_string())]
            )
        );
        return Ok(());
    }
    let mut rows = vec![vec![
        lang.t(M::ColName).to_owned(),
        lang.t(M::ColId).to_owned(),
    ]];
    rows.extend(
        vaults
            .iter()
            .map(|v| vec![printable(&v.name), v.id.clone()]),
    );
    write_table(out, &rows, 40).map_err(out_err)
}

fn cmd_list(vault: &UnlockedVault, lang: Lang, out: &mut dyn Write) -> Result<(), CliError> {
    let items = vault.summaries();
    if items.is_empty() {
        eprintln!(
            "{}",
            lang.tf(M::CliEmptyVault, &[("name", &printable(vault.name()))])
        );
        return Ok(());
    }
    let mut rows = vec![vec![
        lang.t(M::ColName).to_owned(),
        lang.t(M::ColType).to_owned(),
        lang.t(M::ColUser).to_owned(),
        lang.t(M::ColWebsite).to_owned(),
    ]];
    rows.extend(items.iter().map(|s| {
        vec![
            printable(&s.name),
            type_name(lang, s.item_type).to_owned(),
            printable(&s.subtitle),
            printable(&s.uri),
        ]
    }));
    write_table(out, &rows, 36).map_err(out_err)
}

/// Finds one item by id, exact name or search terms.
fn find_item<'v>(
    vault: &'v UnlockedVault,
    query: &str,
    lang: Lang,
) -> Result<&'v VaultItem, CliError> {
    let q = query.trim();
    if let Some(item) = vault.item(q).filter(|i| !i.is_trashed()) {
        return Ok(item);
    }
    let wanted = q.to_lowercase();
    let all = vault.summaries();
    let exact: Vec<_> = all
        .iter()
        .filter(|s| s.name.trim().to_lowercase() == wanted)
        .cloned()
        .collect();
    let candidates = if exact.is_empty() {
        vault.search(q)
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => vault
            .item(&one.id)
            .ok_or_else(|| CliError::failure(lang.tf(M::CliItemNotFound, &[("query", q)]))),
        [] => Err(CliError::failure(
            lang.tf(M::CliItemNotFound, &[("query", &printable(q))]),
        )),
        many => {
            let mut msg = lang.tf(M::CliItemAmbiguous, &[("query", &printable(q))]);
            for s in many.iter().take(20) {
                msg.push_str(&format!("\n  {}", printable(&s.name)));
                if !s.subtitle.is_empty() {
                    msg.push_str(&format!(" ({})", printable(&s.subtitle)));
                }
                msg.push_str(&format!("  [{}]", s.id));
            }
            if many.len() > 20 {
                msg.push_str("\n  …");
            }
            Err(CliError::failure(msg))
        }
    }
}

fn cmd_get(
    vault: &UnlockedVault,
    query: &str,
    field: Field,
    copy: bool,
    settings: &Settings,
    lang: Lang,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let item = find_item(vault, query, lang)?;
    let login = item.login.as_ref();
    let (value, label, secret): (Zeroizing<String>, M, bool) = match field {
        Field::Password => {
            let label = if item.item_type == ItemType::Card {
                M::WhatCardNumber
            } else {
                M::WhatPassword
            };
            (Zeroizing::new(password_of(item)), label, true)
        }
        Field::Username => (Zeroizing::new(username_of(item)), M::WhatUsername, false),
        Field::Totp => {
            let seed = login.map_or("", |l| l.totp.trim());
            let code = if seed.is_empty() {
                String::new()
            } else {
                totp::totp_now(seed)
                    .map_err(|e| CliError::failure(lang.error(&e)))?
                    .code
            };
            (Zeroizing::new(code), M::WhatCode, true)
        }
        Field::Notes => (Zeroizing::new(item.notes.clone()), M::FieldNotes, true),
        Field::Uri => (
            Zeroizing::new(first_url(item).unwrap_or_default()),
            M::FieldNameUri,
            false,
        ),
    };
    if value.trim().is_empty() {
        return Err(CliError::failure(lang.tf(
            M::CliFieldEmpty,
            &[("name", &printable(&item.name)), ("field", lang.t(label))],
        )));
    }
    if copy {
        copy_and_wait(
            &value,
            secret,
            settings.clipboard_clear_seconds,
            lang.t(label),
            lang,
        )
    } else {
        writeln!(out, "{}", value.as_str()).map_err(out_err)
    }
}

/// On X11 (Linux/BSD) the clipboard content is served by the copying
/// process and disappears when it exits.
const CLIPBOARD_NEEDS_OWNER: bool = cfg!(all(unix, not(target_os = "macos")));

/// Copies `value`; secrets are cleared after `clear_secs` (the process
/// waits, any key clears immediately). Status messages go to stderr.
fn copy_and_wait(
    value: &str,
    secret: bool,
    clear_secs: u32,
    what: &str,
    lang: Lang,
) -> Result<(), CliError> {
    let copied = if secret {
        clipboard::copy_secret(value, None)
    } else {
        clipboard::copy_text(value)
    };
    copied.map_err(|e| {
        CliError::failure(lang.tf(M::ClipboardError, &[("err", &clipboard_detail(&e))]))
    })?;
    let interactive = io::stderr().is_terminal();
    if secret && clear_secs > 0 {
        eprintln!(
            "{}",
            lang.tf(
                M::CliCopiedWait,
                &[("what", what), ("s", &clear_secs.to_string())]
            )
        );
        wait_for_key(
            Some(Duration::from_secs(u64::from(clear_secs))),
            interactive,
            lang,
        );
        if matches!(clipboard::clear_if_equals(value), Ok(true)) {
            eprintln!("{}", lang.t(M::CliCleared));
        }
    } else if CLIPBOARD_NEEDS_OWNER && interactive {
        eprintln!("{}", lang.tf(M::CliHoldClipboard, &[("what", what)]));
        wait_for_key(None, true, lang);
    } else {
        eprintln!("{}", lang.tf(M::CliCopiedDone, &[("what", what)]));
    }
    Ok(())
}

/// Waits until a key is pressed or `timeout` elapsed (with a countdown on
/// stderr). Without a terminal it just sleeps for `timeout`.
fn wait_for_key(timeout: Option<Duration>, interactive: bool, lang: Lang) {
    let deadline = timeout.map(|t| Instant::now() + t);
    let raw = if interactive {
        term::RawModeGuard::enter().ok()
    } else {
        None
    };
    if raw.is_none() {
        if let Some(t) = timeout {
            std::thread::sleep(t);
        }
        return;
    }
    let mut stderr = io::stderr();
    loop {
        let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        if remaining.is_some_and(|r| r.is_zero()) {
            break;
        }
        if let Some(r) = remaining {
            let secs = r.as_secs() + u64::from(r.subsec_nanos() > 0);
            let _ = write!(
                stderr,
                "\r{}   ",
                lang.tf(M::CliCountdown, &[("s", &secs.to_string())])
            );
            let _ = stderr.flush();
        }
        let step = remaining.map_or(Duration::from_secs(1), |r| r.min(Duration::from_secs(1)));
        match event::poll(step) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => break,
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    }
    drop(raw);
    if deadline.is_some() {
        let _ = write!(stderr, "\r\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, name: &str) -> VaultInfo {
        VaultInfo {
            id: id.into(),
            name: name.into(),
            path: String::new(),
            created_at: 0,
            updated_at: 0,
            has_recovery_key: false,
        }
    }

    #[test]
    fn help_is_localised() {
        let mut de = command(Lang::De);
        let help = de.render_help().to_string();
        assert!(help.contains("Aufruf:"), "{help}");
        assert!(help.contains("Befehle"));
        assert!(help.contains("Optionen"));
        assert!(help.contains("KEYSTEAD_MASTER_PASSWORD"));
        let mut en = command(Lang::En);
        let get = en
            .find_subcommand_mut("get")
            .unwrap()
            .render_help()
            .to_string();
        assert!(get.contains("Usage:"));
        assert!(get.contains("Arguments"));
        assert!(get.contains("Copy to the clipboard"));
        command(Lang::De).debug_assert();
    }

    #[test]
    fn argument_parsing() {
        let parse = |args: &[&str]| {
            command(Lang::En)
                .try_get_matches_from(args)
                .and_then(|m| Cli::from_arg_matches(&m))
        };
        let cli = parse(&[
            "x", "get", "github", "--field", "totp", "--copy", "--vault", "Work",
        ])
        .unwrap();
        assert_eq!(cli.vault.as_deref(), Some("Work"));
        assert!(matches!(
            cli.command,
            Some(Command::Get {
                field: Field::Totp,
                copy: true,
                ..
            })
        ));
        assert!(parse(&["x"]).unwrap().command.is_none());
        assert!(parse(&["x", "generate", "--length", "4"]).is_err());
        assert!(parse(&["x", "generate", "--length", "12", "--passphrase"]).is_err());
        assert!(parse(&["x", "generate", "--words", "21"]).is_err());
        assert!(parse(&["x", "get"]).is_err());
        let e = parse(&["x", "--bogus"]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        let e = parse(&["x", "--help"]).unwrap_err();
        assert_eq!(e.exit_code(), 0);
    }

    #[test]
    fn vault_choice() {
        let dir = Path::new("/data");
        let vaults = vec![
            info("a1", "Privat"),
            info("b2", "Arbeit"),
            info("c3", "arbeit "),
        ];
        let pick = |arg, last| choose_vault(&vaults, arg, last, dir, Lang::En);
        assert_eq!(pick(Some("privat"), None).unwrap().id, "a1");
        assert_eq!(pick(Some("b2"), None).unwrap().id, "b2");
        assert_eq!(pick(Some("ARBEIT"), None).unwrap_err().code, 2);
        assert_eq!(pick(Some("nope"), None).unwrap_err().code, 1);
        assert_eq!(pick(None, Some("c3")).unwrap().id, "c3");
        assert_eq!(pick(None, None).unwrap_err().code, 2);
        assert_eq!(pick(None, Some("gone")).unwrap_err().code, 2);
        let one = vec![info("a1", "Privat")];
        assert_eq!(
            choose_vault(&one, None, None, dir, Lang::De).unwrap().id,
            "a1"
        );
        assert_eq!(
            choose_vault(&[], None, None, dir, Lang::De)
                .unwrap_err()
                .code,
            1
        );
    }

    #[test]
    fn table_and_sanitising() {
        let mut out = Vec::new();
        let rows = vec![
            vec!["NAME".to_owned(), "ID".to_owned()],
            vec![printable("evil\x1b[2Jname"), "1".to_owned()],
        ];
        write_table(&mut out, &rows, 40).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains('\x1b'));
        assert_eq!(text.lines().next(), Some("NAME          ID"));
    }
}
