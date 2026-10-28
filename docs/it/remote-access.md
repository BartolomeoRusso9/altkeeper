# Accesso remoto

La configurazione più semplice e sicura è mantenere AltKeeper e l'iPhone sulla stessa rete Wi-Fi
fidata. L'accesso remoto aggiunge requisiti di VPN, routing, firewall e rilevamento: non basta
inoltrare una porta.

## Approccio consigliato: VPN privata

Collega telefono e server con una VPN privata come WireGuard o una VPN mesh gestita. Consenti solo
il traffico necessario tra i peer:

- Il servizio di pairing remoto dell'iPhone (di solito TCP `49152`) deve essere raggiungibile dal
  server.
- AltStore deve raggiungere AltServer su TCP `49500`.
- La pagina web usa TCP `8787`, se attiva.
- Il listener Anisette facoltativo usa la porta scelta con `--anisette-bind`.

L'iPhone deve essere collegato al Wi-Fi perché il servizio di pairing remoto sia disponibile. La
sola rete cellulare non basta. Inoltre iOS può sospendere in background l'app VPN e rendere
inaffidabili i rinnovi automatici: verifica prima l'accesso manuale.

## Rilevamento AltStore tramite VPN

AltStore normalmente trova AltServer con il multicast Bonjour. iOS non invia questo traffico di
rilevamento attraverso l'interfaccia VPN, quindi la sola VPN potrebbe non bastare. Il responder
DNS-SD unicast facoltativo di AltKeeper può pubblicare i record AltServer tramite un resolver DNS:

```sh
./AltKeeper altserver \
  --dns-domain casa.internal \
  --dns-address 10.7.0.1 \
  --dns-clients 10.7.0.0/24
```

`--dns-address` è l'indirizzo VPN del server. `--dns-clients` è la sottorete VPN usata dall'iPhone.
Il responder DNS-SD ascolta di default sulla porta UDP `5533`; `--dns-port` la cambia. Configura il
resolver DNS usato dall'iPhone per inoltrare al responder AltKeeper sia il dominio del servizio sia
la zona DNS inversa della sottorete VPN. Per `casa.internal` e `10.7.0.0/24`, inoltra ad esempio
`casa.internal` e `7.10.in-addr.arpa`. La configurazione precisa dipende dal resolver DNS.

Non esporre DNS-SD, AltServer, pagina web o listener Anisette direttamente su Internet. Limita
l'accesso ai peer VPN sia nella VPN sia nel firewall dell'host.

## Verifica e diagnosi

1. Collega entrambi i dispositivi alla VPN e verifica che l'iPhone raggiunga l'indirizzo VPN del
   server.
2. Controlla che la porta di pairing remoto sia raggiungibile mentre l'iPhone è collegato al Wi-Fi.
3. Se il pairing funziona ma AltStore non trova AltServer, controlla l'inoltro DNS-SD sia per il
   dominio del servizio sia per la zona inversa.
4. Controlla i log AltKeeper mentre premi **Refresh All** in AltStore.

Se il rinnovo automatico non riesce a raggiungere l'iPhone perché iOS ha sospeso l'app VPN, rinnova
manualmente con la VPN attiva oppure mantieni il telefono sulla rete locale.
