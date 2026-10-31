# Installazione e avvio

AltKeeper richiede una cartella dati persistente e accesso di rete all'iPhone. Scegli un solo metodo
di avvio; non eseguire più istanze AltKeeper nella stessa rete, a meno che tu non sappia isolare i
loro annunci AltServer.

## Linux con Docker Compose

Il file Compose di esempio usa la rete host per Bonjour e per annunciare il servizio ad AltStore.
È pensato per host Docker Linux:

1. Copia [`examples/docker-compose.yml`](../../examples/docker-compose.yml) in una cartella, per
   esempio `/srv/altkeeper`.
2. Crea `/srv/altkeeper/.env` con `ALTKEEPER_WEB_PIN=<pin-privato>` e limita i permessi del file.
3. Metti `rp-pairing.plist` in `/srv/altkeeper`. Se vuoi i rinnovi autonomi, accedi una volta per
   creare `account.json` e `state/` nella cartella dati montata.
4. Da quella cartella esegui `docker compose up -d`.

L'esempio monta `/srv/altkeeper` in `/data`; pairing, account, stato, log e backup dei profili
rimangono persistenti. L'immagine non contiene questi file. Per l'accesso USB dal container, vedi
il mount `usbmuxd` commentato nel Compose. Per Anisette locale, consulta
[Server Anisette locale](anisette-server.md).

## Windows

Usa l'eseguibile nativo Windows, non Docker Desktop: AltKeeper usa Bonjour e Docker Desktop non
fornisce il comportamento di rete host necessario. Se compili dai sorgenti, installa gli strumenti
di build Microsoft Visual C++; altrimenti scarica l'eseguibile Windows dalle
[release](https://github.com/BartolomeoRusso9/altkeeper/releases). Il pacchetto Windows da scaricare
è `AltKeeper-v<versione>-Windows-x64.zip`: contiene `AltKeeper.exe`, le guide e gli esempi. È
disponibile anche il binario singolo `AltKeeper-v<versione>-Windows-x64.exe`; entrambi hanno il
relativo file `.sha256`. L'aggiornamento integrato scarica quel binario singolo e il suo file
`.sha256`, quindi questo schema di nomi va mantenuto.

Da PowerShell, pairing e avvio dei servizi:

```powershell
.\AltKeeper.exe pair-usb
.\AltKeeper.exe serve --altserver
```

Installa iTunes o Dispositivi Apple per il servizio USB mux locale. Consenti AltKeeper nel firewall
Windows solo sulle reti **Private**. Per i rinnovi pianificati, il computer deve essere acceso e non
in sospensione. Usa l'Utilità di pianificazione per l'avvio o i rinnovi; vedi
[`examples/renew.ps1`](../../examples/renew.ps1).

La modalità tray avvia pagina web e AltServer senza terminale:

```powershell
.\AltKeeper.exe tray
```

Di default, la cartella dati è quella dell'eseguibile; `--dir <cartella>` permette di cambiarla.
La pagina web integrata ascolta solo su loopback. Per la finestra dell'app serve WebView2.

## macOS

Scarica l'archivio corretto per il tuo Mac dalle
[release](https://github.com/BartolomeoRusso9/altkeeper/releases):
`AltKeeper-v<versione>-macOS-Apple-Silicon-App.zip` per Apple Silicon o
`AltKeeper-v<versione>-macOS-Intel-App.zip` per Intel. Estrai `AltKeeper.app` e spostala in
`/Applications`. L'app resta nella barra dei menu e avvia AltServer e la pagina web locale.
Consenti l'accesso alla rete locale quando richiesto. L'app non è autenticata da Apple: se macOS
blocca il primo avvio, segui le istruzioni Gatekeeper nelle note della release.

Gli archivi CLI corrispondenti si chiamano `...-macOS-Apple-Silicon-CLI.tar.gz` e
`...-macOS-Intel-CLI.tar.gz`. Gli archivi Linux si chiamano `...-Linux-x64.tar.gz` o
`...-Linux-ARM64.tar.gz`. Ogni archivio ha un file checksum `.sha256` con lo stesso nome.

La cartella dati dell'app è `~/Library/Application Support/AltKeeper`. L'opzione **Start at login**
nel menu gestisce il LaunchAgent. Puoi anche compilare la versione CLI con
`cargo build --release`; gli script del progetto permettono di creare un'app `.app` personalizzata.

## Dati e backup

Mantieni privata e persistente la cartella dati. Esegui backup sicuri di `rp-pairing.plist`,
`account.json`, `state/` e dei backup dei profili. Non inserirli in immagini pubbliche né nel
repository. Vedi [Problemi e sicurezza](troubleshooting-and-security.md).
