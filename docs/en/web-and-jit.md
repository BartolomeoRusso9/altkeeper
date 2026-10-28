# Web dashboard and JIT

## Start the dashboard

For access from other trusted devices on the LAN, set a PIN and bind the web server to a LAN
address:

```sh
ALTKEEPER_WEB_PIN='your-private-pin' ./AltKeeper serve \
  --bind 0.0.0.0:8787 --altserver
```

Open `http://<server-address>:8787`. Use any username and the PIN as the password. A non-loopback
web bind requires `ALTKEEPER_WEB_PIN` (at least four characters). A loopback-only server does not
require a PIN. The web service has no HTTPS; keep it on a trusted network and never forward its
port directly to the Internet.

The dashboard displays phone reachability, apps and expiry dates, logs, and renewal status. It
also provides Apple sign-in, pairing, device profiles, Anisette selection, renewal scheduling, and
profile management. See [Accounts and renewals](accounts-and-renewals.md) for credential and
scheduling behavior.

## Install an IPA from a computer

The dashboard's IPA upload signs and installs the selected IPA using the Apple ID for the active
device profile. This may register the iPhone and create development App IDs and certificates on
the Apple team. Uploads are limited to 1 GiB. Keep the dashboard private and review its job log.

The optional JIT setting attempts to launch the installed app and enable JIT. If JIT activation
fails, the app remains installed and the job log reports the failure.

## JIT requirements and status

JIT can be requested for a running app from the dashboard, during supported IPA installation, or
by an AltStore request. The iPhone must have Developer Mode enabled and a compatible Developer
Disk Image already mounted. AltKeeper does not mount the image automatically. JIT activation has
not yet been verified end to end on a physical device.

The HTTP API is specific to AltKeeper. It is not the Husk/StikJIT protocol; clients must implement
AltKeeper's API explicitly.

## API for compatible clients

Base URL: `http://<server-address>:8787`. When a PIN is set, authenticate with HTTP Basic (any
username, PIN as password). POST requests must include `X-Requested-With: altkeeper`. API jobs run
in the background: start a job, then poll `/api/state` until `job.running` is `false`.

First fetch the current process list:

```sh
export ALTKEEPER_URL='http://192.168.1.20:8787'
export ALTKEEPER_WEB_PIN='your-pin'
curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
  -H 'X-Requested-With: altkeeper' -H 'Content-Type: application/json' \
  -d '{}' "$ALTKEEPER_URL/api/jit/apps"
curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
  "$ALTKEEPER_URL/api/state" | jq '{job, jit_apps}'
```

After the job completes, use a PID from the returned `jit_apps` list:

```sh
curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
  -H 'X-Requested-With: altkeeper' -H 'Content-Type: application/json' \
  -d '{"pid":1234}' "$ALTKEEPER_URL/api/jit/enable"
curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
  "$ALTKEEPER_URL/api/state" | jq '.job'
```

Refresh the process list before enabling JIT. AltKeeper accepts only a PID from the latest list and
checks that the process is still a running app. For a non-default device profile, add
`X-AltKeeper-Device: <profile-id>` to each request. Never expose the API to the public Internet.
