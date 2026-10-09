//! Keystead terminal UI.
//!
//! Without a subcommand an interactive full-screen UI (ratatui) starts:
//! vault picker, master-password prompt and a two-pane item browser with
//! copy/open/edit/generator functions. Subcommands (`list`, `get`,
//! `generate`, `vaults`) offer non-interactive access for scripts.

mod app;
mod cli;
mod i18n;
mod input;
mod platform;
mod term;
#[cfg(test)]
mod test_support;
mod ui;

/// Entry point used by `Keystead --cli` and the `keystead-cli` binary.
/// Parses the process arguments (a leading `--cli` / `cli` is ignored) and
/// returns the process exit code: 0 = success, 1 = error, 2 = invalid usage.
pub fn run() -> i32 {
    // Lossy conversion: a non-UTF-8 argument must not panic.
    run_with_args(
        std::env::args_os()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
    )
}

/// Same as [`run`] with explicit arguments (`args[0]` = program name).
pub fn run_with_args(args: Vec<String>) -> i32 {
    let (args, via_flag) = strip_cli_flag(args);
    cli::main(args, via_flag)
}

/// Removes a leading `--cli` / `cli` argument. Returns whether it was there
/// (then the program is the desktop app's `Keystead --cli`).
fn strip_cli_flag(mut args: Vec<String>) -> (Vec<String>, bool) {
    if args.is_empty() {
        args.push("keystead-cli".to_owned());
    }
    let via_flag = args.len() > 1 && matches!(args[1].as_str(), "--cli" | "cli");
    if via_flag {
        args.remove(1);
    }
    (args, via_flag)
}

#[cfg(test)]
mod tests {
    use super::strip_cli_flag;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cli_flag_is_ignored() {
        assert_eq!(
            strip_cli_flag(v(&["Keystead.exe", "--cli", "list"])),
            (v(&["Keystead.exe", "list"]), true)
        );
        assert_eq!(
            strip_cli_flag(v(&["Keystead", "cli"])),
            (v(&["Keystead"]), true)
        );
        assert_eq!(
            strip_cli_flag(v(&["keystead-cli", "get", "cli"])),
            (v(&["keystead-cli", "get", "cli"]), false)
        );
        assert_eq!(strip_cli_flag(Vec::new()), (v(&["keystead-cli"]), false));
    }
}
