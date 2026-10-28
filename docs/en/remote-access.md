# Remote access

The simplest safe setup is to keep AltKeeper and the iPhone on the same trusted Wi-Fi. Remote
access adds VPN, routing, firewall, and discovery requirements; it is not just a matter of
forwarding a port.

## Recommended approach: private VPN

Connect the phone and server through a private VPN such as WireGuard or a managed mesh VPN. Allow
only the required traffic between those VPN peers:

- The phone's remote-pairing service (usually TCP `49152`) must be reachable from the server.
- AltStore must be able to reach AltServer on TCP `49500`.
- The dashboard uses TCP `8787`, if enabled.
- The optional Anisette listener uses the port configured with `--anisette-bind`.

The iPhone must be connected to Wi-Fi for its remote-pairing service to be available. Cellular
connectivity alone is not sufficient. Background iOS VPN suspension can also make unattended
renewal unreliable; test manual access first.

## AltStore discovery over a VPN

AltStore normally discovers AltServer using Bonjour multicast. iOS does not send this discovery
traffic over a VPN interface, so a VPN connection by itself may not be enough. AltKeeper's
optional unicast DNS-SD responder can publish AltServer records to a DNS resolver:

```sh
./AltKeeper altserver \
  --dns-domain home.internal \
  --dns-address 10.7.0.1 \
  --dns-clients 10.7.0.0/24
```

`--dns-address` is the server's VPN address. `--dns-clients` is the VPN subnet used by the phone.
The DNS-SD responder listens on UDP `5533` by default; `--dns-port` changes it. Configure the
resolver used by the phone to forward both the chosen service domain and the VPN subnet's reverse
DNS zone to the AltKeeper responder. For example, with `home.internal` and `10.7.0.0/24`, forward
`home.internal` and `7.10.in-addr.arpa`. Exact configuration depends on the DNS resolver.

Do not expose the DNS-SD responder, AltServer, dashboard, or Anisette listener to the public
Internet. Restrict access to the VPN peers in both the VPN and host firewall.

## Verify and diagnose

1. Connect the VPN on both devices and confirm the phone can reach the server's VPN address.
2. Confirm the remote-pairing port is reachable while the phone is on Wi-Fi.
3. If phone pairing works but AltStore cannot find AltServer, inspect DNS-SD forwarding for both
   the service domain and reverse zone.
4. Check AltKeeper logs while pressing **Refresh All** in AltStore.

If automatic renewal cannot reach the phone while iOS has suspended the VPN app, use manual
renewal when the VPN is active or keep the phone on the local network.
