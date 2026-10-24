# Windows console / Apple ID 2FA smoke test

This is a **manual integration test** for [issue #1](https://github.com/lgg/altkeeper/issues/1).
Automated unit tests cover parsing and EOF; they cannot emulate Apple's trusted-device
challenge or prove correct PowerShell console behavior.

## Environment

- Windows 10 (the verified platform) or Windows 11 (not independently confirmed),
  an iPhone and a working anisette endpoint
- Run from the AltKeeper project directory
- **Never** put your real Apple ID password, six-digit code, or `account.json` in logs,
  screenshots, CI artifacts or issue comments
- Avoid repeatedly requesting verification codes when Apple rate limits requests

## Verification matrix (2026-10-24)

| Platform / scenario | Result |
|---|---|
| Windows 10 PowerShell: real Apple ID login | **User-confirmed:** 2FA digits and menu commands work |
| Windows 10 `cargo test --locked` | **Passed:** 63 / 63 tests |
| Windows 10 `cargo build --release --locked` | **Passed:** executable built |
| Windows 10 `ALTKEEPER_LANG=en` / `--help` | **Passed:** English usage printed |
| Windows PowerShell 5.1 (ConsoleHost) | Not separately confirmed |
| cmd.exe regression | Not separately confirmed after this change |
| Windows 11 PowerShell | Not verified |
| Windows AltServer and unattended renewal | Not verified end to end |

## Steps

1. Build the current main branch (or the feature branch if testing a future PR):

   ```powershell
   git switch main
   git pull --ff-only
   cargo test --locked
   cargo build --release --locked
   $env:ALTKEEPER_ANISETTE_URL = "https://mac-ani.jkcoxson.com"
   $env:ALTKEEPER_LANG = "en"
   .\target\release\altkeeper.exe login "your-apple-id@example.com"
   ```

2. Type the Apple ID password. When asked for a 2FA code, enter the **current** six-digit code
   and press Enter. Check that it accepts all six digits (including a leading zero).
3. If you have multiple teams, check selecting a number. On the save-password prompt, press
   `N` unless you specifically want an unattended renewal with credentials stored locally.
4. Repeat the login in a fresh PowerShell 5.1 session and in cmd.exe if Apple requests 2FA
   again; do not force excessive 2FA challenges just to complete the matrix.
5. If safe and practical, check `d` (send to device), `r` (resend if offered) and
   `p<id>` (SMS to listed number); never post real codes in test output.
6. Confirm errors do **not** repeatedly submit an empty code when input reaches EOF.
   Automated tests cover this case; do not trigger real Apple attempts to simulate EOF.

## Automated checks

```powershell
cargo test --locked
$env:ALTKEEPER_LANG = "en"
cargo run --locked -- --help
$env:ALTKEEPER_LANG = "it"
cargo run --locked -- --help
```

English output should start with `usage: altkeeper`, Italian with `uso: altkeeper`.
The GitHub workflow defines Windows and Linux tests, but **no Actions runs were available**
when the Windows 10 results were recorded. Local success does not imply CI success.

Report only operating system, terminal version, pass/fail, and sanitized error messages.
