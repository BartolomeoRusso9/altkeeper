# Windows console / Apple ID 2FA smoke test

This is a **manual integration test** for [issue #1](https://github.com/lgg/altkeeper/issues/1).
Automated unit tests cover parsing and EOF; they cannot emulate Apple's trusted-device
challenge or prove correct PowerShell console behavior.

## Environment

- Windows 10 or Windows 11, paired iPhone and working anisette endpoint
- Run from the AltKeeper project directory
- **Never** put your real Apple ID password, six-digit code, or `account.json` in logs,
  screenshots, CI artifacts or issue comments
- Avoid repeatedly requesting verification codes when Apple rate limits requests

## Test matrix

| Host | Expected result |
|---|---|
| PowerShell 7.x in Windows Terminal | Code and Enter are accepted |
| Windows PowerShell 5.1 (ConsoleHost) | Code and Enter are accepted |
| cmd.exe | Existing behavior still works |
| PowerShell with `ALTKEEPER_LANG=en` | English prompts, code works |
| PowerShell with `ALTKEEPER_LANG=it` | Italian prompts, code works |

## Steps

1. Build the current PR branch:

   ```powershell
   git switch fix/issue-1-windows-console-2fa
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
These tests should run on GitHub Actions for Windows and Linux as well.

Report only operating system, terminal version, pass/fail, and sanitized error messages.
