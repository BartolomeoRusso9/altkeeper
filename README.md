# AltKeeper

AltKeeper keeps apps installed on an iPhone available by renewing their provisioning profiles.
It can also act as an AltServer for AltStore Classic, so apps can be refreshed or installed over
Wi-Fi. It runs on a home server, a Mac, or Windows.

> **Experimental software.** AltKeeper uses private Apple APIs and protocols that may change
> without notice. JIT activation and some installation flows have not been verified end to end on
> physical devices. Use it with your own devices and Apple ID; it is not affiliated with Apple or
> AltStore.

## Choose a guide

| I want to… | Guide |
| --- | --- |
| Set up AltKeeper and pair my iPhone | [Quick start](docs/en/quick-start.md) |
| Choose Docker, Windows, or macOS deployment | [Deployment](docs/en/deployment.md) |
| Host my own Anisette V3 server | [Local Anisette server](docs/en/anisette-server.md) |
| Sign in, renew apps, or manage multiple devices | [Accounts and renewals](docs/en/accounts-and-renewals.md) |
| Use the web dashboard, install an IPA, or enable JIT | [Web and JIT](docs/en/web-and-jit.md) |
| Connect from outside my home network | [Remote access](docs/en/remote-access.md) |
| Diagnose a problem or protect my data | [Troubleshooting and security](docs/en/troubleshooting-and-security.md) |

Prefer Italian? Start at [README in italiano](README.it.md) or browse the [Italian guides](docs/it/README.md).

## At a glance

- **AltServer mode:** AltStore Classic can refresh and install apps through AltKeeper.
- **Independent renewal:** renew profiles from the CLI, dashboard, or an optional schedule.
- **Device profiles:** keep pairing, account, settings, and renewal state separate for each iPhone.
- **Local Anisette V3:** an optional server for Linux, macOS, and Windows; native libraries must be
  supplied by the operator.
- **JIT:** dashboard actions and an HTTP API are available to compatible clients. Developer Mode
  and a compatible mounted Developer Disk Image are required.

The CLI and server support Linux, macOS, and Windows; the tray app is available on macOS and
Windows. Docker is intended for Linux hosts that support host networking and Bonjour.

For private vulnerability reports, see [SECURITY.md](SECURITY.md). Third-party licenses are listed
in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
