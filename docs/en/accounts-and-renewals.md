# Accounts and renewals

AltKeeper has two separate workflows:

- **AltServer refresh:** AltStore handles Apple sign-in on the iPhone. AltKeeper does not need the
  Apple ID password for this workflow.
- **Independent renewal:** AltKeeper signs in to Apple to renew profiles without waiting for
  AltStore. This requires an Apple ID whose team signed the apps.

## Sign in

From the persistent data directory, run:

```sh
./AltKeeper login your-apple-id@example.com
```

Complete the password and two-factor prompts. Saving the password enables unattended renewal.
AltKeeper stores a saved password in `account.json` **in clear text**; restrict access to the data
directory and back it up securely. Do not send this file to anyone.

Check the account and installed profiles:

```sh
./AltKeeper apps
./AltKeeper phone
```

## Renew manually

Start with a dry run:

```sh
./AltKeeper renew --dry-run --min-days 7
```

Then renew profiles that expire within the selected threshold:

```sh
./AltKeeper renew --min-days 7
```

The default threshold is 3 days. A normal renewal checks installed apps first and skips profiles
that do not match installed apps. If the phone's installed-app inventory cannot be read, AltKeeper
logs a warning and falls back to all known profiles for the configured team. A successful lookup
that returns no apps skips renewals.

Use `--bundle <bundle-id>` to target an app. `--force` bypasses the normal expiry threshold.
`--installed-only` requests installed-app filtering, including with `--force`. If the lookup
fails, AltKeeper logs a warning and falls back to all known profiles for the configured team,
regardless of expiry, even with `--force --installed-only`. It does not guarantee renewal of only
installed profiles. Read `altkeeper renew --help` for the complete option list.

### Automatically register missing App IDs

The dashboard option **Auto-register missing App IDs** is off by default. When enabled, renewal
registers a missing App ID before downloading its profile, consuming the free account's quota of
10 App IDs per 7 days. If no quota remains, renewal fails explicitly instead of skipping the app;
dry runs never register.

## Schedule renewals

Choose one scheduler:

- The dashboard's **Automatic renewal** setting runs once per UTC day at the selected time. It is
  off by default and needs the server to stay running, a saved Apple ID password, and a reachable
  iPhone. Each device profile has its own schedule. Only one phone operation runs at a time.
- The example scripts [`renew.sh`](../../examples/renew.sh) and
  [`renew.ps1`](../../examples/renew.ps1) can be run from cron or Windows Task Scheduler.

Do not enable both the built-in schedule and an external cron/task unless you intentionally want
two attempts. An external scheduler must run from the correct data directory and keep its logs and
credentials private.

## Multiple iPhones and accounts

In the dashboard, use the device selector and **Add device** to create a named profile for another
iPhone. Select a profile before pairing its phone or signing in to its Apple ID. Each profile keeps
its own account, pairing, saved phone addresses, settings, schedule, logs, and profile backups.
The default profile keeps existing data; added profiles are stored under `devices/`.

AltServer remains a single network service and uses the currently selected device profile. Do not
run multiple copies of AltKeeper to serve several profiles concurrently.

## Remove and restore profiles

The dashboard can remove individual provisioning profiles after confirmation. Before removal,
AltKeeper saves and verifies a backup under `profili-salvati/`. Removing a profile that an
installed app still needs may stop that app from opening. Restore a saved profile with:

```sh
./AltKeeper profile-restore profili-salvati/<profile-file>
```

Renewal does not remove older profiles or revoke Apple Developer App IDs.
