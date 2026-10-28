# Local Anisette V3 server

AltKeeper can optionally host an Anisette V3 service. It is not started by default. This is
separate from AltServer and the web dashboard.

## Provide the native libraries

The operator must supply both native libraries; AltKeeper does not download or bundle them:

```text
<data-directory>/anisette-libraries/lib/<architecture>/libstoreservicescore.so
<data-directory>/anisette-libraries/lib/<architecture>/libCoreADI.so
```

Use the architecture directory expected by the native engine:

| Host architecture | Directory name |
| --- | --- |
| x86-64 | `x86_64` |
| x86 | `x86` |
| ARM64 | `arm64-v8a` |
| ARM 32-bit | `armeabi-v7a` |

Use libraries you are authorized to use and follow their license terms. The default library root
is `<data-directory>/anisette-libraries`; `--anisette-libs <directory>` overrides it. The path
must contain the `lib/<architecture>/` subdirectories shown above.

## Start the listener

Keep it on a trusted LAN or VPN interface. Example with the web dashboard:

```sh
ALTKEEPER_WEB_PIN='your-private-pin' ./AltKeeper serve \
  --bind 0.0.0.0:8787 \
  --anisette-bind 192.168.1.20:8788
```

On Windows, use the native executable and PowerShell syntax. `--anisette-libs` is optional when
using the default directory. For Docker on Linux, mount the libraries read-only at
`/data/anisette-libraries` and add `--anisette-bind 192.168.1.20:8788` to the `serve` command.

Select **AltKeeper local server** in the dashboard's Anisette settings when AltKeeper should use
this server. Another client on the LAN can use `http://192.168.1.20:8788`.

## Choose which Anisette server AltKeeper uses

The dashboard can use AltKeeper's local server, the configured public isideload server
(`https://ani.jkcoxson.com`), or a custom HTTP(S) URL. The default is
`https://ani3.stikstore.app`. The selected URL is saved in `state/anisette-url`.

If `ALTKEEPER_ANISETTE_URL` is set, it overrides and locks the dashboard selection. Remove that
environment variable to manage the choice in the UI. Clearing the saved selection restores the
default. When the effective server changes, AltKeeper retires the saved Anisette identity; Apple
may ask you to verify the account again.

## Network and data safety

The Anisette V3 protocol has no authentication or TLS. The web PIN does **not** protect this
listener. Bind only to an interface reachable by trusted devices, restrict access with the host
firewall, and never expose the port directly to the Internet.

Provisioning state is temporary and stored under `state/anisette-provisioning/`. If the service
fails at startup, check that both library files exist in the exact architecture directory and
that the process can read them.
