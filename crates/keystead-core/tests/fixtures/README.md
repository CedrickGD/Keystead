# Legacy VaultX 1.x fixtures

Generated independently of the Rust code with a small Python script
(`hashlib.pbkdf2_hmac("sha1")`, `hmac`, and AES-256-CBC/PKCS#7 from the
`cryptography` package) that reproduces `Save-Vault`,
`Get-KeyPairFromPassword` and `Invoke-RecoveryOptions` from
`legacy/VaultX.ps1`, including Windows PowerShell 5.1 quirks (UTF-8 BOM and
CRLF in the file, `ConvertTo-Json` spacing and `'`-style escapes).

| File | Password(s) | What it covers |
|---|---|---|
| `legacy_v2.json` | `Korrekt-Pferd-42!` | Version 2 with `Mac`, 100 000 iterations, `TotpSecret`, three entries (e-mail as username, phone/other fields, `/Date(…)/` timestamp) |
| `legacy_v1.json` | `altes-passwort` | Version 1 without `Mac` (32-byte key only) |
| `legacy_recovery.json` | `master-pw` or recovery `Wiederherstellung-2024` | Recovery fields (`RecoveryKeyData` = encKey‖macKey) |
| `legacy_single_entry.json` | `einzel` | `Entries` is a single object, UTF-8 BOM inside the plaintext |
| `legacy_v1_empty.json` | any | Empty `Data` (vault without entries) |
| `legacy_accounts.json` | – | `accounts.json` array incl. a broken file and a path-traversal attempt |
| `legacy_accounts_single.json` | – | `accounts.json` holding a single object |
