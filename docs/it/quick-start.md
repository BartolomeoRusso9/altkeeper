# Guida introduttiva

Questa guida associa un iPhone e ne verifica la connessione. Dopo la prova, scegli come mantenere
AltKeeper in esecuzione nella guida [Installazione e avvio](deployment.md).

## Requisiti

- iPhone con iOS 17 o successivo e Modalità sviluppatore attiva.
- AltKeeper in esecuzione su una macchina raggiungibile dall'iPhone via Wi-Fi o VPN.
- Un collegamento USB iniziale per creare il pairing remoto.
- AltStore Classic serve solo per usare AltKeeper come AltServer.
- Un Apple ID salvato serve solo per rinnovare le app autonomamente o firmare IPA dalla pagina web.

## Ottieni AltKeeper

Scarica un binario dalle [release GitHub](https://github.com/BartolomeoRusso9/altkeeper/releases)
oppure compilalo con Rust:

```sh
git clone https://github.com/BartolomeoRusso9/altkeeper
cd altkeeper
cargo build --release --locked
```

I pacchetti release usano l'eseguibile `AltKeeper` (`AltKeeper.exe` su Windows). Compilando dai
sorgenti si ottiene `target/release/altkeeper` (`target\release\altkeeper.exe` su Windows). Per
installazione e avvio persistente, vedi
[Installazione e avvio](deployment.md). I comandi qui sotto presuppongono il nome del pacchetto
release; se compili dai sorgenti, copia il binario come `AltKeeper` oppure usa il percorso
`target/release/`.

## Associa l'iPhone via USB

Collega e sblocca l'iPhone, autorizza il computer, poi esegui:

```sh
./AltKeeper pair-usb
./AltKeeper phone
```

Su Windows usa `.\AltKeeper.exe`. Il pairing crea `rp-pairing.plist` nella cartella
dati. Se AltKeeper verrà eseguito su un'altra macchina, trasferisci il file in modo sicuro nella
cartella dati di quella macchina. È un file segreto.

## Avvia AltServer e la pagina web

Su una rete domestica fidata, avvia entrambi i servizi:

```sh
ALTKEEPER_WEB_PIN='scegli-un-pin-privato' ./AltKeeper serve \
  --bind 0.0.0.0:8787 --altserver
```

Su Windows, da PowerShell esegui `.\AltKeeper.exe serve --altserver`. Poi:

1. Apri `http://<indirizzo-server>:8787` per usare la pagina web.
2. In AltStore Classic consenti l'accesso alla rete locale e prova prima con una sola app.
3. Mantieni attivo un solo AltServer nella rete.

Senza `--altserver`, `serve` avvia solo la pagina web. Per l'accesso solo locale, ometti `--bind`
oppure usa `127.0.0.1`. Un bind web non locale richiede `ALTKEEPER_WEB_PIN`; il servizio non usa
HTTPS, quindi non pubblicarlo direttamente su Internet.

## Rilevamento dell'iPhone

Di norma AltKeeper trova l'iPhone automaticamente. Per impostare un indirizzo fisso, passa
`--phone ip:porta` oppure imposta `ALTKEEPER_PHONE=ip:porta`; il servizio di pairing usa di solito
la porta `49152`. L'iPhone deve essere collegato al Wi-Fi per rendere raggiungibile il pairing remoto.

I messaggi della CLI sono in inglese per impostazione predefinita. Imposta `ALTKEEPER_LANG=it` per
usare l'italiano; l'impostazione non cambia la lingua della pagina web.

## Prossimi passi

- [Installazione e avvio](deployment.md) per Docker, Windows e macOS.
- [Account e rinnovi](accounts-and-renewals.md) per accedere e pianificare i rinnovi.
- [Web e JIT](web-and-jit.md) per le funzioni della pagina web e i requisiti JIT.
- [Problemi e sicurezza](troubleshooting-and-security.md) prima di esporre un servizio in rete.
