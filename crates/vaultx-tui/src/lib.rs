//! VaultX terminal UI.

/// Entry point used by `VaultX --cli` and the `vaultx-cli` binary.
/// Parses `std::env::args()` (a leading `--cli` / `cli` is ignored) and
/// returns the process exit code.
pub fn run() -> i32 {
    run_with_args(std::env::args().collect())
}

/// Same as [`run`] with explicit arguments (`args[0]` = program name).
pub fn run_with_args(_args: Vec<String>) -> i32 {
    0
}
