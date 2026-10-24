# AltKeeper

**AltServer for iOS 17+ that runs on your home server (Docker).** Refresh and install your
AltStore / SideStore-style apps over Wi-Fi, with no computer left on and no VPN.

Keeps sideloaded iPhone apps alive. Apps installed with a **free Apple ID** stop opening after
7 days unless their *provisioning profile* is renewed. AltKeeper runs on a small always-on
machine at home and does it over Wi-Fi, with **no computer plugged in, no AltServer on a
computer and no VPN on the phone**. It can work in two ways, together or separately:

- **As AltServer for AltStore Classic**: AltStore's *Refresh All* button and installing or
  updating apps from AltStore work, with this machine answering instead of AltServer.
- **On its own**: it renews the profiles by itself every evening (cron), even if AltStore is
  never opened.

There is also a small **web page** to see the state of your apps and renew them:

<img src="docs/screenshots/web.png" alt="AltKeeper web page: iPhone reachable, three apps with their expiry, renew buttons" width="320">

> **Status: experimental.** Tested on an iPhone 15 Pro (iOS 27.0) with AltStore Classic (the latest
> version at the time of writing), running on a Mac and on a Debian 13
> mini PC (x86-64, Docker). It relies on private Apple APIs and on AltStore's own protocol, which can
> change without notice. For personal use with your own Apple ID; not affiliated with Apple or
> AltStore. Some command-line messages and code comments are localized to the original project language.
>
> Confirmed with AltServer running from the Docker image on the Debian mini PC: *Refresh* from
> AltStore installs the new profile on the iPhone, and installing an app from AltStore works.
> Confirmed earlier with the server on a Mac: the same, with an app of about 126 MB, plus sign-in,
> `apps`, `renew`, and that an app still opens with only the *newer* profiles installed (also after
> rebooting the iPhone). Not confirmed yet: what happens on the day the *original* profile
> expires, running AltServer on a server for a longer time, or installing an IPA uploaded from the
> web page against Apple and a real iPhone.

## CLI language (English / Italian)

AltKeeper's command-line prompts, errors, and service logs default to Italian for backwards
compatibility. Set `ALTKEEPER_LANG=en` to use English, or `ALTKEEPER_LANG=it` for Italian.
Language is read for each invocation (no rebuild necessary). Values such as `en-US`,
`en_GB`, and `it-IT` are accepted case-insensitively; unknown languages fall back to Italian.
The legacy `ALTREFRESH_LANG` variable is also accepted when `ALTKEEPER_LANG` is unset.
This changes human-readable CLI output, **not** protocol messages, machine-readable data,
or the web application's language.

PowerShell:

```powershell
$env:ALTKEEPER_LANG = "en"
.\target\release\altkeeper.exe phone
.\target\release\altkeeper.exe --help
```

Command Prompt (cmd.exe):

```cmd
set ALTKEEPER_LANG=en
target\release\altkeeper.exe phone
```

Linux, macOS, or a container environment:

```sh
ALTKEEPER_LANG=en ./altkeeper phone
```

When running under Docker Compose, add `ALTKEEPER_LANG: en` to the service's
`environment:` section.

## What it does, and what it does not

- Renews the provisioning profiles of your apps (and, in AltServer mode, installs the apps AltStore
  sends it). The renewal flow does not re-sign apps or create/revoke certificates. Installing an
  IPA uploaded from the web page does sign it and may create a development certificate.
- **AltServer mode does not need your Apple ID on the server.** AltStore signs in on the iPhone;
  this program only gives it anisette data and installs what AltStore sends. The unattended renewal
  (`renew`, cron, and the web page's renew action) and signing an IPA from the web page do need the
  Apple ID and its password on the server.
- The two do not clash: `renew` looks at the newest profile on the phone. If AltStore renewed
  recently, `renew` does nothing and does not even sign in to Apple.
- Old profiles stay on the phone after a renewal (iOS does not remove them), so you will see
  several profiles per app. That is expected. In AltServer mode it never removes profiles, even
  when AltStore asks (`activeProfiles`).
- Not supported: enabling JIT (`EnableUnsignedCodeExecution`); it answers with an error.
- After an unattended `renew`, AltStore may keep showing the old expiry date: it only updates its
  own record when it does the refresh itself. The apps are fine.

## What you need

- An iPhone on **iOS 17 or later**, with Developer Mode on, on the same Wi-Fi as the server.
- A machine that is always on in your home network. Tested: Debian 13 (x86-64). A Mac works too. Windows 10/11 also works natively
  (see [Windows](#10-windows-native-no-docker)); that port has not been tried on a real PC yet.
- For AltServer mode: **AltStore Classic** on the iPhone, with the *Local Network* permission.
- For the unattended renewal: a **free Apple ID** whose team already signed the apps.
- For the **first pairing only**: the iPhone plugged in by USB to a computer (a Mac is easiest).
- Optional: `curl`, `jq` and the [ntfy](https://ntfy.sh) app for notifications; Docker.

## Set up

### 1. Get the program

```sh
git clone https://github.com/BartolomeoRusso9/altkeeper
cd altkeeper
docker run --rm -v "$PWD":/w -w /w rust:1-bookworm cargo build --release
```

The binary is `target/release/altkeeper`. Notes:

- On an Apple Silicon Mac, add `--platform linux/amd64` to build for an x86-64 server (slower:
  about 5 to 7 minutes).
- If you also build for the Mac in the same folder, add `-e CARGO_TARGET_DIR=/w/target-linux` so
  the two builds do not overwrite each other.
- On a Mac you can simply run `cargo build --release`.
- Or use the Docker image (see [Docker](#9-docker)).

On the server, create a folder (for example `/srv/altkeeper`) and copy there the `altkeeper`
binary and the files in `examples/` you need.

### 2. Find your iPhone (optional)

AltKeeper finds the iPhone by itself, and finds it again if its address changes (it changed
several times in a few hours in our tests). It tries, in order: the address you give, the last one
that worked (`state/phone-addr`), Bonjour (`_remotepairing._tcp`), and finally a scan of your local
network for the phone's service port (49152), which does not depend on Bonjour (on a server where
another program shares the Bonjour port, the answers can go missing). So you can skip this step. If you prefer to give the address, use `--phone ip:port` or `ALTKEEPER_PHONE=ip:port`
(the port is usually 49152). On a Mac you can look it up:

```sh
dns-sd -B _remotepairing._tcp                       # lists the instances, stop with Ctrl-C
dns-sd -L "<instance name>" _remotepairing._tcp local   # shows host and port (usually 49152)
dns-sd -G v4 <host>.local                           # shows the IP address
```

### 3. Pair (once, over USB)

`pair-usb` needs the computer's `usbmuxd` reachable over TCP. On a Mac, use the small bridge in
`examples/`:

```sh
# Terminal 1 (leave it open): exposes the Mac's usbmuxd on 127.0.0.1:27015
python3 examples/usbmux-bridge.py

# Terminal 2, with the iPhone plugged in, unlocked, and "Trust" accepted:
export USBMUXD_SOCKET_ADDRESS=127.0.0.1:27015
./target/release/altkeeper pair-usb
```

This writes `rp-pairing.plist`. Copy it next to the binary on the server and keep it private
(`chmod 600`). Copying a pairing made on a Mac to a Debian server worked in our tests. On Linux, the
same bridge idea with `socat` (`socat TCP-LISTEN:27015,bind=127.0.0.1,fork UNIX-CONNECT:/var/run/usbmuxd`)
should work, but we have not tried it.

Then check the connection, from the server folder:

```sh
./altkeeper phone        # lists the profiles installed on the phone
```

### 4. AltServer for AltStore (recommended)

```sh
./altkeeper altserver            # or: ./altkeeper serve --altserver (with the web page)
```

Leave it running (as a service or in Docker). On the iPhone, in AltStore, open **My Apps** and tap
**Refresh All**, or install an app: AltStore finds the server by itself. It listens on port
**49500** (`--port N` to change it) and announces itself with Bonjour as `_altserver._tcp`.

- Only one AltServer should be on the network: stop any AltServer on a computer, and do not run
  two copies of this program.
- Try it first with one unimportant app, not with *Refresh All* on everything.
- AltStore shows *"AltServer could not be found"* for several kinds of lost connection when the
  server is not the one it prefers, so that message can hide the real cause. Look at the server's
  output: it logs every connection, request and answer.
- The first request creates the anisette identity in `state/` if it does not exist yet. We have
  used it with an identity created by `login`; a clean folder has not been tried.

### 5. Sign in (for unattended renewal and PC IPA installation)

```sh
./altkeeper login your.apple.id@example.com
```

It asks for your password (**it is shown on screen while you type**) and the 6-digit 2FA code from
your other devices. Answer `s` when it asks to save the password: that is what lets the renewal run
without you. It is stored **in clear text** in `account.json` (mode 600). Sign in once; if Apple
answers with errors, wait before trying again instead of repeating it.

```sh
./altkeeper apps                                # your App IDs and expiry dates
./altkeeper renew --dry-run --min-days 7        # shows what it would renew, changes nothing
./altkeeper renew --min-days 7                  # renews now what expires within 7 days
```

`renew` looks at each app's newest profile, renews the ones with `--min-days` days left or fewer
(default 3), and signs in to Apple **only if something is due**.

### 6. Automatic renewal (optional)

A safety net that does not depend on the iPhone running AltStore.

1. Copy `renew.sh` and `notifica.sh` from `examples/` next to the binary, and `chmod +x renew.sh`.
   Leave `ALTKEEPER_PHONE` unset in `renew.sh` (the phone is found by itself) or put its `IP:port`.
2. Install the cron job (edit the path first): `cp altkeeper.cron /etc/cron.d/altkeeper`

It runs every evening at 21:17, renews what is due and appends date and result to `renew.log`.
The iPhone must be on the home Wi-Fi at that time. With profiles lasting 7 days and a threshold of
3, you get several evenings of margin before an app expires.

### 7. Notifications (optional)

Create `notify.conf` next to the scripts, with a long random topic name:

```sh
echo 'NTFY_TOPIC=pick-a-long-random-name-here' > notify.conf && chmod 600 notify.conf
```

Install the ntfy app on your phone and subscribe to that topic. You get a message when apps are
renewed and when a run fails (at most one error message every 24 hours, because the phone may just
be away from home). Nothing is sent when there is nothing to renew. Anyone who knows the topic can
read it, so keep it secret.

### 8. Web page (optional)

```sh
ALTKEEPER_WEB_PIN=123456 ./altkeeper serve --bind 0.0.0.0:8787 --altserver
```

Open `http://<server>:8787` (any user name, the PIN as password). It shows whether the iPhone is
reachable and the apps with the days left, lets you renew them ("Renew expiring apps" or "Renew all
now", with a live log), sign in with your Apple ID (the 2FA code is typed in the browser) and see
the latest renewals from `renew.log`. Without `--bind`, or with a loopback address, it listens only
on the machine itself and needs no PIN; on any other address it **refuses to start without a PIN**
(at least 4 characters). Use the environment variable rather than `--pin`, which is visible in `ps`.
The web page has no HTTPS: keep it inside your home network. The web page's own sign-in and renew
were not run against Apple yet.

From a PC, you can also upload an `.ipa` in **Install an app from this PC**. AltKeeper signs it with
the configured Apple ID and installs it on the iPhone over Wi-Fi. This requires the Apple ID
password to be saved in `account.json`; enter the 2FA code in the page if Apple asks for it. Signing
may register the iPhone and create development app IDs and certificates on the Apple team. Uploads
are limited to 1 GiB. Protect the page with a PIN when exposing it on your network, and never
forward its port to the internet.

### 9. Docker

```sh
docker build --platform linux/amd64 -t altkeeper:latest .
```

The image holds nothing secret: pairing, account and state live in the `/data` volume. Copy
`examples/docker-compose.yml` to `/srv/altkeeper/docker-compose.yml`, put your
`rp-pairing.plist` (and `account.json`/`state/` if you use `renew`) in the same folder, write
`ALTKEEPER_WEB_PIN=...` in `/srv/altkeeper/.env` (mode 600) and run `docker compose up -d`. It
uses the host network, because it needs Bonjour to find the iPhone and to be found by AltStore;
ports 8787 (web) and 49500 (AltServer) must be free.

A GitHub Actions workflow (`.github/workflows/docker.yml`) runs the tests and publishes the image
to `ghcr.io/<user>/altkeeper` on pushes to `main` (amd64) and on `vX.Y.Z` tags (amd64 and
arm64). It has not been run on GitHub yet. If the repository is private the package is private too.

### 10. Windows (native, no Docker)

Docker Desktop on Windows cannot use the host network that AltKeeper needs for Bonjour, so on
Windows run the program directly. **This port has not been built or run on a real Windows
machine yet**: if something breaks, please open an issue.

1. Install [Rust](https://rustup.rs) (the default MSVC toolchain) and the *Build Tools for Visual
   Studio* with the **Desktop development with C++** workload. Then, in PowerShell:

   ```powershell
   git clone https://github.com/BartolomeoRusso9/altkeeper
   cd altkeeper
   cargo build --release
   ```

   The binary is `target\release\altkeeper.exe`. Make a folder (for example `C:\altkeeper`) and
   copy there the `.exe` and, from `examples\`, `renew.ps1`.

2. **Pair (once, over USB).** If iTunes or the *Apple Devices* app is installed, Windows already
   has a usbmuxd listening on `127.0.0.1:27015`: no bridge needed (`usbmux-bridge.py` is for the
   Mac). With the iPhone plugged in, unlocked and trusted:

   ```powershell
   $env:USBMUXD_SOCKET_ADDRESS = "127.0.0.1:27015"
   .\altkeeper.exe pair-usb
   ```

   Then `.\altkeeper.exe phone` should list the profiles on the phone.

3. **Run it.** From the folder you made:

   ```powershell
   .\altkeeper.exe serve --altserver      # web page on http://127.0.0.1:8787 + AltServer
   # or only AltServer:
   .\altkeeper.exe altserver
   ```

   The first time, Windows asks to allow the program through the firewall: allow it on **Private**
   networks (your Wi-Fi must be set as *Private*, not *Public*). By hand, for AltServer (TCP
   49500) and Bonjour (UDP 5353), from an administrator PowerShell:

   ```powershell
   New-NetFirewallRule -DisplayName "AltKeeper" -Direction Inbound -Action Allow  -Program "C:\altkeeper\altkeeper.exe" -Profile Private
   ```

   If another Bonjour service already uses UDP 5353 on that PC (Apple's *Bonjour Service*, which
   comes with iTunes, is fine), see "If something goes wrong". Stop with Ctrl-C.

4. **Sign in and automatic renewal.** `.\altkeeper.exe login you@example.com` works as on the
   other systems. In PowerShell and cmd.exe, type the six-digit 2FA code and press Enter;
   the `d`, `r`, and `p<id>` options also work. Interactive prompts are read directly
   from the Windows console, so switching from PowerShell to cmd.exe is unnecessary.
   Run `login` in an interactive console rather than a background task.
   `account.json`, `state\` and `profili-salvati\` are restricted to your Windows
   user (the equivalent of `chmod 600/700`). For the nightly renewal use the Task Scheduler with
   `renew.ps1` instead of cron:

   ```powershell
   schtasks /Create /TN "AltKeeper renew" /SC DAILY /ST 21:17  /TR "powershell -NoProfile -ExecutionPolicy Bypass -File C:\altkeeper\renew.ps1"
   ```

   To start AltServer when you log in:

   ```powershell
   schtasks /Create /TN "AltKeeper" /SC ONLOGON  /TR "C:\altkeeper\altkeeper.exe altserver --dir C:\altkeeper"
   ```

   `renew.ps1` keeps the same `renew.log`, the same optional `notify.conf` (ntfy) and the same
   once-a-day error notification as `renew.sh`. It needs the PC to be on and awake at that time.

Everything else in this guide (commands, environment variables, outside-the-home setup) is the
same; only the paths use `\`, and `cron` and `renew.sh` are replaced by the Task Scheduler and
`renew.ps1`.

## Using it from outside your home network

AltStore finds servers with Bonjour, which normally means multicast — and **iOS never sends
multicast discovery over a VPN interface**. That is why installing Tailscale or ZeroTier is not
enough by itself: the phone never even asks.

The way around it is Bonjour's second, older mode: **unicast DNS-SD**, where services are
published as ordinary DNS records. AltStore browses with an empty domain, meaning "all default
domains", and iOS works out which domains those are by asking the DNS server about the network
it is on. Answer that question and the phone finds the server on its own — nothing to configure
on the phone at all.

AltKeeper serves those records itself. Here is the whole setup.

### 1. A VPN between the phone and the server

Any VPN works as long as the phone gets an address on it. WireGuard is a good fit: it is in the
Linux kernel, the iOS app is free, and it holds up better than the alternatives when the phone
changes network.

On the server:

```
apt install wireguard-tools
cd /etc/wireguard
umask 077
wg genkey > server.key && wg pubkey < server.key > server.pub
wg genkey > phone.key  && wg pubkey < phone.key  > phone.pub
wg genpsk > psk.key
```

`/etc/wireguard/wg0.conf`:

```ini
[Interface]
Address = 10.7.0.1/24
ListenPort = 51820
PrivateKey = <contents of server.key>

[Peer]
PublicKey = <contents of phone.pub>
PresharedKey = <contents of psk.key>
AllowedIPs = 10.7.0.2/32
```

```
systemctl enable --now wg-quick@wg0
```

Forward **UDP 51820** to this machine on your router. WireGuard never answers packets that are
not cryptographically valid, so the port stays invisible to scanners — but it is still an open
door, so it is worth restricting what the tunnel can reach. Add to `[Interface]`:

```ini
PostUp = iptables -I INPUT 1 -i wg0 -p udp --dport 53 -j ACCEPT
PostUp = iptables -I INPUT 2 -i wg0 -p tcp --dport 49500 -j ACCEPT
PostUp = iptables -I INPUT 3 -i wg0 -p tcp --dport 8787 -j ACCEPT
PostUp = iptables -I INPUT 4 -i wg0 -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
PostUp = iptables -A INPUT -i wg0 -j DROP
PostUp = iptables -I FORWARD 1 -i wg0 -j DROP
```

That leaves DNS, AltServer and the web page reachable and nothing else — not SSH, and no route
into the rest of your LAN. Mirror each rule with `PostDown = iptables -D …`.

The phone's config, which you import by scanning a QR code
(`qrencode -t ansiutf8 < phone.conf`):

```ini
[Interface]
PrivateKey = <contents of phone.key>
Address = 10.7.0.2/32
DNS = 10.7.0.1

[Peer]
PublicKey = <contents of server.pub>
PresharedKey = <contents of psk.key>
Endpoint = your-home-address:51820
AllowedIPs = 10.7.0.0/24
PersistentKeepalive = 25
```

`AllowedIPs = 10.7.0.0/24` sends only the tunnel's own subnet through the VPN, so the phone's
normal browsing is untouched. `Endpoint` needs an address that follows your home connection —
a dynamic DNS name if your ISP changes your IP.

### 2. Publishing the records

```
altkeeper altserver --dns-domain home.internal --dns-address 10.7.0.1 --dns-clients 10.7.0.0/24
```

- `--dns-address` is where the phone should connect: this machine's address **on the VPN**.
- `--dns-clients` is the subnet the phones are on. It matters because iOS discovers the domain by
  asking about the reverse name of its own address; without it AltKeeper assumes the `/24` around
  `--dns-address`.
- The responder listens on UDP 5533 (`--dns-port`), answers only for its own names and forwards
  nothing.

### 3. Sending those answers to the phone

The phone's DNS is the server (`DNS = 10.7.0.1` above), which is whatever resolver you already
run. Point just this one domain — and the reverse zone of the VPN subnet — at AltKeeper.

With **AdGuard Home**, in *Upstream DNS servers*:

```
[/home.internal/]127.0.0.1:5533
```

and, because AdGuard sends private reverse lookups to a separate list, also add to
*Private reverse DNS servers*:

```
[/7.10.in-addr.arpa/]127.0.0.1:5533
```

With **dnsmasq**:

```
server=/home.internal/127.0.0.1#5533
server=/7.10.in-addr.arpa/127.0.0.1#5533
```

That second line is the one people miss, and without it the phone never learns the domain.

### 4. Try it

Turn on the VPN, connect the phone to any Wi-Fi that is not your home one, and press *Refresh
All*. On the server you should see the discovery and then the work:

```
[altserver] 10.7.0.2: AnisetteDataRequest -> AnisetteDataResponse
[altserver] 10.7.0.2: InstallProvisioningProfilesRequest
[altserver] profile installed: com.example.app
```

If it says it cannot find the server, watch the DNS with
`tcpdump -i wg0 -n port 53` and see which name the phone asks for.

### What to expect

- **The phone must be on Wi-Fi.** iOS only runs the pairing service AltKeeper needs while
  connected to a Wi-Fi network; on cellular alone the port is closed, and nothing can change that.
- **Unattended renewals over the tunnel are not dependable yet.** iOS suspends VPN apps in the
  background, so a nightly cron may find the phone unreachable. Renewing by hand while the tunnel
  is up works fine.
- If the server was finding the phone by scanning the LAN, put its VPN address in
  `state/phone-addr` (`10.7.0.2:49152`) so it tries that first.

## Commands

| Command | What it does |
| --- | --- |
| `phone [--phone ip:port]` | lists the profiles installed on the iPhone |
| `pair-usb [file]` | creates the remote pairing over USB (once) |
| `login <apple-id>` | signs in with 2FA and saves the account |
| `apps` | lists your App IDs and their expiry dates |
| `renew [--dry-run] [--force] [--min-days N] [--phone ip:port]` | renews the profiles that are due |
| `altserver [--port N] [--dir folder] [--phone ip:port] [--dns-domain D --dns-address IP]` | acts as AltServer for AltStore |
| `serve [--bind ip:port] [--pin PIN] [--dir folder] [--phone ip:port] [--altserver] [--altserver-port N]` | web page (and, with `--altserver`, AltServer) |
| `profile-remove <uuid>` | takes one profile off the phone, saving a copy in `profili-salvati/` |
| `profile-restore <file>` | puts a saved profile back |

`ALTKEEPER_PHONE=ip:port` can replace `--phone`; without either the phone is found by itself (see
step 2). `ALTKEEPER_NO_MDNS=1` turns Bonjour off (networks that block multicast, or to test the scan).

**USB.** If the iPhone is plugged into the same computer that runs AltKeeper, it is used instead of
Wi-Fi (faster, especially for installing IPAs). AltKeeper looks for it through the local `usbmuxd`
(installed with iTunes / Apple Devices on Windows, built in on macOS, the `usbmuxd` package on
Linux) or through `USBMUXD_SOCKET_ADDRESS`; the phone must already trust the computer. If there is
no phone on USB, or it fails, Wi-Fi is used as before. `--phone` / `ALTKEEPER_PHONE` force Wi-Fi,
and `ALTKEEPER_NO_USB=1` turns USB off. In Docker, mount `/var/run/usbmuxd` into the container.

**USB.** If the iPhone is plugged into the same computer that runs AltKeeper, it is used instead of Wi-Fi (faster, especially for installing IPAs): AltKeeper looks for it through the local `usbmuxd` (on Windows/macOS it is installed with iTunes/Apple Devices or is built in; on Linux install `usbmuxd`), or through `USBMUXD_SOCKET_ADDRESS`. The phone must already trust the computer. Passing `--phone ip:port` forces Wi-Fi, and `ALTKEEPER_NO_USB=1` turns USB off. In Docker, mount `/var/run/usbmuxd` into the container.
`ALTKEEPER_DEBUG=1` prints what Apple answers to each sign-in request (status, headers, error
bodies; never request bodies or successful responses).
`ALTKEEPER_ANISETTE_URL=https://...` uses your own anisette server (for example `anisette-v3-server`)
instead of the default one; it must start with `http://` or `https://`. If the default one stops working, see "If something goes wrong".

The project used to be called altrefresh: the old `ALTREFRESH_*` variable names still work (the
`ALTKEEPER_*` one wins if both are set). `altkeeper --version` prints the version.

## If something goes wrong

- **"remote pairing is missing"**: there is no `rp-pairing.plist` in the folder. Pair again (step 3).
- **"phone pairing failed" / `early eof`**: the iPhone no longer recognises that pairing. Pair
  again, or copy a working `rp-pairing.plist` from another computer.
- **"cannot find the iPhone on the network", "No route to host", timeouts**: the iPhone is not on
  the network (asleep, away). If you gave an address that is no longer right, the program looks for
  the phone by itself; if you want a fixed address, on the iPhone open Settings, Wi‑Fi, the (i) of
  your network, and set the private Wi‑Fi address to **Fixed** or off (the wording depends on the iOS
  version), then give the iPhone a fixed IP in your router.
- **AltStore says "AltServer could not be found"**: see step 4. Check that the server is running, on
  the same network, that AltStore has the Local Network permission, and read the server's output.
- **AltStore shows an error and the log says `not provisioned for this server (-45061)` or
  `reprovision required`**: the default public anisette server does not accept the identity saved
  in `state/anisette_state` (the server changed or is having trouble). Move the file aside
  (`mv state/anisette_state state/anisette_state.old`) and start with another server, for example
  `ALTKEEPER_ANISETTE_URL=https://ani3.stikstore.app altkeeper altserver`. People also report
  `https://mac-ani.jkcoxson.com` working. Keep the variable set for good (shell profile, the `.env`
  of the compose file, or `Environment=` in a systemd unit), otherwise the next start goes back to
  the default server. Apple may ask for a verification code again after the new identity is
  created. Public servers come and go: if it fails again, try another, or run your own
  (`anisette-v3-server`).
- **A refresh from AltStore seems stuck right after `PrepareAppRequest`**: AltStore first sends the
  app to the server, and it can be well over 100 MB. On a weak Wi‑Fi that takes minutes; the server
  prints `app in arrivo: X di Y byte` every few seconds while it arrives, and gives up after 10
  minutes in total. Keep AltStore open on screen and the iPhone close to the router.
- **Sign-in errors 429 or 503**: Apple is limiting requests. Do not retry in a loop. Run with
  `ALTKEEPER_DEBUG=1` and, if it persists, open an issue with the lines starting with `[gs #`
  (they contain no secrets).
- **An app does not open after a renewal**: reinstall it with AltStore or the tool you normally
  use. If you removed a profile with `profile-remove`, put it back with `profile-restore`.

## Security

These files hold secrets. They are ignored by git and should be mode `600`:

- `rp-pairing.plist`: the pairing keys for the phone.
- `account.json`: the Apple ID and, if you chose to save it, the password **in clear text**.
- `state/`: the anisette "device" identity and the AltServer `serverID`.
- `notify.conf`: the ntfy topic.
- `profili-salvati/`: copies of profiles taken off the phone with `profile-remove`.

See also [SECURITY.md](SECURITY.md) for reporting a vulnerability privately.

Never paste your password, `account.json` or the pairing file in an issue or a chat. Do not use
`RUST_LOG=debug` on shared logs: at that level `idevice` prints the pairing keys. Keep the machine
that stores them inside your home network.

**The AltServer port (49500) has no login, exactly like the real AltServer**: anyone on your network
who speaks the protocol can ask for anisette data or send profiles and apps to the paired iPhone.
Do not expose that port outside your home network.

## How it works

1. The iPhone (iOS 17+) advertises the Bonjour service `_remotepairing._tcp`.
2. Using an existing remote pairing, the program does the *pair-verify*, asks the phone to open a
   tunnel and connects to it with TLS-PSK ([`idevice`](https://github.com/jkcoxson/idevice)).
3. Inside the tunnel it reads and installs profiles with `misagent`, and installs or removes apps
   with `installation_proxy`.
4. In AltServer mode, AltStore finds the machine through Bonjour (`_altserver._tcp`, with a `serverID`
   record) and sends JSON requests over TCP (a 32-bit little-endian length, then the JSON), one
   connection per operation: anisette data, profiles to install or remove, and the signed app.
5. For the unattended renewal, with your Apple ID ([`isideload`](https://github.com/nab138/isideload))
   it downloads new profiles for the existing App IDs.

The old Wi-Fi sync used by `netmuxd` and AltServer-Linux is rejected by recent iOS versions; remote
pairing is the channel that works.

## Notes on the copy of `isideload` in `vendor/`

`isideload` 0.3.17 has two problems with Apple's sign-in servers, so `vendor/isideload` is a
patched copy of the original (MIT, © nab138):

- **503 at sign-in.** It presents itself as `com.apple.dt.Xcode`, which Apple rejects.
  `src/anisette/remote_v3/mod.rs` now declares `com.apple.akd/1.0` (client identity
  `Mac15,7`, macOS 27.0) and the AuthKit User-Agent AltSign uses.
- **429 at the password proof.** Apple's edge lets only a couple of requests through on one
  connection and refuses the rest. Sign-in sends three in a row (URL bag, `init`, `complete`),
  so the third got a 429. AltSign avoids it with one fresh connection per request
  (`ALTAppleAPI+Authentication.swift`, notarized branch); `src/auth/grandslam.rs` now does the
  same (HTTP/1.1, no connection reuse, `Connection: close`). On macOS it also uses the system
  TLS. Which of these changes is the decisive one has not been isolated.

It also exposes a few read-only accessors of `AnisetteData`, used to answer AltStore.

## Credits and license

Built on [`idevice`](https://github.com/jkcoxson/idevice) (Jackson Coxson) and
[`isideload`](https://github.com/nab138/isideload) (nab138), both MIT-licensed. The sign-in fix
follows what [AltSign](https://github.com/rileytestut/AltSign) (Riley Testut) does, and the AltServer
messages follow [AltStore](https://github.com/altstoreio/AltStore)'s server protocol.
This project is licensed under the [MIT license](LICENSE).
