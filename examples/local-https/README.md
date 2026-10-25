# Private LAN HTTPS for AltKeeper (optional)

This is a **separate Caddy container**, not a replacement for AltKeeper's
existing HTTP server. On WalnutPi it serves
**https://192.168.0.62:8788/** and reverse-proxies to the existing
**http://host.docker.internal:8787**. No router port-forwarding or public CA
is needed. It does **not** configure remote Internet access. Apple sign-in,
sideloading and provisioning may still contact Apple's servers as before.

The example assumes the Pi has a stable LAN address `192.168.0.62`.
Set `ALTKEEPER_LAN_IP` in a dedicated `.env` file in this directory if
your LAN address differs. Check port 8788 availability before deploying.

## Install on the Pi

After the PR is merged, download only the two config files to a new directory,
not into /srv/altkeeper's existing Docker compose:

```bash
sudo mkdir -p /srv/altkeeper/https
sudo chown "$USER":"$USER" /srv/altkeeper/https
cd /srv/altkeeper/https

curl -fsSLo Caddyfile https://raw.githubusercontent.com/pingpingjinping/altkeeper/main/examples/local-https/Caddyfile
curl -fsSLo compose.yml https://raw.githubusercontent.com/pingpingjinping/altkeeper/main/examples/local-https/compose.yml
# If the Pi address changes, add ALTKEEPER_LAN_IP=... to .env here.
ss -ltn | grep ':8788 ' || true
docker compose -f compose.yml config
docker compose -f compose.yml up -d
docker logs --tail 25 altkeeper-https
curl -sk -o /dev/null -w 'HTTPS code: %{http_code}\n' https://192.168.0.62:8788/
```

HTTP status 401 is normal before entering AltKeeper's existing web PIN.
The Caddy `caddy-data` volume **must stay persistent** so the CA
and certificates are not replaced on each restart. Do not share
`caddy-data/caddy/pki/authorities/local/root.key`.

## Trust the local root certificate on iPhone

Caddy's `tls internal` certificate must be explicitly trusted by each
client. The public **root.crt is safe to distribute to your own devices;
never distribute root.key**.

Safest simple transfer from the Pi (serve **only** a temporary directory
containing the public CA certificate; never run an HTTP file server against
`/srv/altkeeper`, which contains `account.json` and pairing secrets):

```bash
tmpdir="$(mktemp -d)"
sudo install -m 0644 \
  /srv/altkeeper/https/caddy-data/caddy/pki/authorities/local/root.crt \
  "$tmpdir/altkeeper-root.crt"
python3 -m http.server 8877 --bind 192.168.0.62 --directory "$tmpdir"
# Open http://192.168.0.62:8877/altkeeper-root.crt in iPhone Safari.
# STOP the temporary server with Ctrl+C when the certificate downloads.
# Later: rm -rf "$tmpdir"
```

On iPhone, open the downloaded certificate/profile:
**Settings > General > VPN & Device Management > Downloaded Profile >
Install**, then **Settings > General > About > Certificate Trust Settings
> enable full trust for the AltKeeper/Caddy local root**.
The exact UI may differ slightly by iOS release.

Open **https://192.168.0.62:8788/** in Safari and enter the same web PIN
on the new origin if asked. Check the certificate is trusted: no warning.
The existing HTTP Home Screen shortcut is a **different origin** and will not
magically switch to HTTPS; if using Home Screen web-app mode, add a **new**
shortcut from the trusted HTTPS page after verifying Safari works.
This is what makes `window.isSecureContext` true and exposes the
`navigator.wakeLock` API.

The new HTTPS port only listens on `192.168.0.62` on the Pi and the
original **http://192.168.0.62:8787/** still works. Make sure there is
no router port forward to 8788; your router/firewall remain authoritative
for LAN isolation.

For desktop access, install the same CA into the computer's trusted roots
only if you understand the additional trust this grants; do not blindly
trust arbitrary certificates. The HTTPS proxy is optional.

## Verify Wake Lock

The new **5-minute Wake Lock test** is a small button next to the IPA
installation form (no video or floating card). Select iPhone
Auto-Lock 30 seconds, disable Low Power Mode, open HTTPS in Safari, tap
test, and leave the phone untouched. The status message shows whether the
browser accepted the request, but only an actual screen-on test confirms
the behavior. Repeat in Home Screen web-app mode if desired.

The API is used automatically **only** during new IPA installation and is
released when the job ends. Renewal and profile management do not trigger it.
An iOS/browser decision can revoke or deny a request; this is best effort.

## Recovery

```bash
cd /srv/altkeeper/https
docker compose -f compose.yml down
# Existing AltKeeper HTTP on 8787, and its Apple/phone data, are untouched.
```

Do not delete `caddy-data` unless you intend to revoke this local CA and
reinstall a replacement root certificate on every device.
