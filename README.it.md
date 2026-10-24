# AltKeeper

**AltServer per iOS 17+ che gira sul tuo server di casa (Docker).** Rinnova e installa le app
in stile AltStore / SideStore via Wi-Fi, senza un computer acceso e senza VPN.

[English](README.md)

Tiene in vita le app installate sull'iPhone senza App Store. Le app installate con un **Apple ID
gratuito** smettono di aprirsi dopo 7 giorni, se non si rinnova il loro *profilo di provisioning*.
AltKeeper gira su una piccola macchina sempre accesa in casa e lo fa via Wi-Fi, **senza computer
collegato, senza AltServer su un computer e senza VPN sul telefono**. Può funzionare in due modi,
insieme o separati:

- **Come AltServer per AltStore Classic**: il tasto *Refresh All* di AltStore e l'installazione o
  l'aggiornamento delle app da AltStore funzionano, con questa macchina che risponde al posto di
  AltServer.
- **Da sola**: rinnova i profili ogni sera (cron), anche se AltStore non viene mai aperto.

C'è anche una piccola **pagina web** per vedere lo stato delle app e rinnovarle:

<img src="docs/screenshots/web.png" alt="Pagina web di AltKeeper: iPhone raggiungibile, tre app con la scadenza, pulsanti di rinnovo" width="320">

> **Stato: sperimentale.** Provato su un iPhone 15 Pro (iOS 27.0) con AltStore Classic (l'ultima
> versione al momento della scrittura), con il programma su un Mac e su un mini PC
> Debian 13 (x86-64, Docker). Usa API private di Apple e il protocollo di AltStore, che possono
> cambiare senza preavviso. Per uso personale con il proprio Apple ID; non affiliato ad Apple né ad
> AltStore. La CLI supporta l'italiano (predefinito) e l'inglese per messaggi, errori e log;
> la pagina web e i messaggi delle librerie esterne hanno impostazioni indipendenti.
>
> Confermato con AltServer dall'immagine Docker sul mini PC Debian: il *Refresh* da AltStore installa
> il profilo nuovo sull'iPhone e l'installazione di un'app da AltStore funziona. Confermato prima con
> il server sul Mac: lo stesso, con un'app di circa 126 MB, più login, `apps`,
> `renew`, e che un'app si apre anche con i soli profili *nuovi* installati (anche dopo il riavvio
> dell'iPhone). Non ancora confermato: cosa succede il giorno in cui scade il profilo *originale*,
> AltServer su un server per un periodo lungo, né l'installazione di un IPA caricato dalla pagina web
> con Apple e un iPhone reale.

## Lingua della CLI (inglese / italiano)

La CLI, inclusi i messaggi di errore, le richieste interattive e i log del server, usa
l'italiano per impostazione predefinita per compatibilità. Per usare l'inglese, imposta
`ALTKEEPER_LANG=en`; per l'italiano, `ALTKEEPER_LANG=it`. Funzionano anche `en-US`,
`en_GB`, `it-IT` (senza distinzione maiuscole/minuscole). Lingue sconosciute
ricadono sull'italiano. Se `ALTKEEPER_LANG` manca, funziona anche
`ALTREFRESH_LANG`. Questo cambia solo il testo visibile della CLI, non il
protocollo AltStore, i dati JSON o la lingua della pagina web.

PowerShell (vale per la sessione corrente e per i processi figli):

```powershell
$env:ALTKEEPER_LANG = "en"
.\target\release\altkeeper.exe --help
.\target\release\altkeeper.exe login "you@example.com"
# Per ripristinare il valore predefinito: Remove-Item Env:ALTKEEPER_LANG
```

Prompt dei comandi (cmd.exe):

```cmd
set ALTKEEPER_LANG=en
target\release\altkeeper.exe phone
```

Linux/macOS:

```sh
ALTKEEPER_LANG=en ./altkeeper phone
```

Il file `examples/docker-compose.yml` usa una lista `environment:`: per l'inglese aggiungi
`- ALTKEEPER_LANG=en`. Questo cambia i messaggi CLI e i log, **non** il protocollo,
i JSON o la pagina web. I comandi del menu 2FA restano `d`, `r` (se disponibile), `p<id>`.

## Cosa fa e cosa non fa

- Rinnova i profili di provisioning delle tue app (e, in modalità AltServer, installa le app che
  AltStore gli manda). Il rinnovo non rifirma le app e non crea né revoca certificati. L'installazione
  di un IPA caricato dalla pagina web invece lo rifirma e può creare un certificato di sviluppo.
- **La modalità AltServer non ha bisogno dell'Apple ID sul server.** AltStore fa il login
  sull'iPhone; questo programma gli dà solo i dati anisette e installa quello che AltStore manda. Il
  rinnovo senza controllo (`renew`, cron, il tasto "Rinnova" della pagina web) e la firma di un IPA
  caricato dalla pagina web hanno invece bisogno dell'Apple ID e della sua password sul server.
- I due non si pestano i piedi: `renew` guarda il profilo più recente sul telefono. Se AltStore ha
  rinnovato da poco, `renew` non fa niente e non accede nemmeno ad Apple.
- Dopo un rinnovo i profili vecchi restano sul telefono (iOS non li toglie), quindi per ogni app
  vedrai più profili. È normale. In modalità AltServer non toglie mai profili, nemmeno quando
  AltStore lo chiede (`activeProfiles`).
- Non supportato: l'abilitazione del JIT (`EnableUnsignedCodeExecution`); risponde con un errore.
- Dopo un `renew` senza controllo, AltStore può continuare a mostrare la vecchia scadenza: aggiorna
  il suo registro solo quando il rinnovo lo fa lui. Le app funzionano.

## Cosa serve

- Un iPhone con **iOS 17 o successivo**, con la modalità sviluppatore attiva, sullo stesso Wi-Fi
  del server.
- Una macchina sempre accesa nella rete di casa. Provato: Debian 13 (x86-64); va bene anche un Mac.
  Su **Windows 10** sono stati verificati la compilazione nativa, i test e il login interattivo
  con Apple ID e 2FA. Windows 11 e l'intero flusso AltServer/rinnovo su Windows non sono ancora
  verificati end-to-end (vedi [Windows](#10-windows-nativo-senza-docker)).
- Per la modalità AltServer: **AltStore Classic** sull'iPhone, con il permesso *Rete locale*.
- Per il rinnovo senza controllo: un **Apple ID gratuito** il cui team ha già firmato le app.
- Solo per il **primo pairing**: l'iPhone collegato via USB a un computer (il più facile è un Mac).
- Facoltativi: `curl`, `jq` e l'app [ntfy](https://ntfy.sh) per le notifiche; Docker.

## Configurazione

### 1. Ottieni il programma

```sh
git clone https://github.com/lgg/altkeeper
cd altkeeper
docker run --rm -v "$PWD":/w -w /w rust:1-bookworm cargo build --release
```

Il binario è `target/release/altkeeper`. Note:

- Su un Mac con Apple Silicon, aggiungi `--platform linux/amd64` per compilare per un server
  x86-64 (più lento: circa 5-7 minuti).
- Se compili anche per il Mac nella stessa cartella, aggiungi `-e CARGO_TARGET_DIR=/w/target-linux`
  così le due compilazioni non si sovrascrivono.
- Su un Mac puoi semplicemente lanciare `cargo build --release`.
- Oppure usa l'immagine Docker (vedi [Docker](#9-docker)).

Sul server crea una cartella (per esempio `/srv/altkeeper`) e copia lì il binario `altkeeper` e i
file di `examples/` che ti servono.

### 2. Trova l'iPhone (facoltativo)

AltKeeper trova l'iPhone da solo e lo ritrova se il suo indirizzo cambia (nelle nostre prove è
cambiato più volte in poche ore). Prova, in ordine: l'indirizzo che indichi tu, l'ultimo che ha
funzionato (`state/phone-addr`), Bonjour (`_remotepairing._tcp`) e infine una scansione della rete
locale cercando la porta del servizio del telefono (49152), che non dipende da Bonjour (su un server
dove un altro programma condivide la porta di Bonjour, le risposte possono andare perse). Puoi quindi
saltare questo passo. Se preferisci indicare l'indirizzo, usa `--phone ip:porta` o `ALTKEEPER_PHONE=ip:porta`
(la porta di solito è 49152). Da un Mac lo trovi così:

```sh
dns-sd -B _remotepairing._tcp                       # elenca le istanze, si ferma con Ctrl-C
dns-sd -L "<nome istanza>" _remotepairing._tcp local    # mostra host e porta (di solito 49152)
dns-sd -G v4 <host>.local                           # mostra l'indirizzo IP
```

### 3. Pairing (una volta sola, via USB)

`pair-usb` ha bisogno dell'`usbmuxd` del computer raggiungibile via TCP. Su un Mac usa il piccolo
ponte in `examples/`:

```sh
# Terminale 1 (lascialo aperto): espone l'usbmuxd del Mac su 127.0.0.1:27015
python3 examples/usbmux-bridge.py

# Terminale 2, con l'iPhone collegato, sbloccato e con "Autorizza" già accettato:
export USBMUXD_SOCKET_ADDRESS=127.0.0.1:27015
./target/release/altkeeper pair-usb
```

Crea `rp-pairing.plist`. Copialo accanto al binario sul server e tienilo riservato (`chmod 600`).
Copiare sul server Debian un pairing fatto con un Mac ha funzionato nelle nostre prove. Su Linux la
stessa idea con `socat` (`socat TCP-LISTEN:27015,bind=127.0.0.1,fork UNIX-CONNECT:/var/run/usbmuxd`)
dovrebbe funzionare, ma non l'abbiamo provata.

Poi controlla il collegamento, dalla cartella del server:

```sh
./altkeeper phone        # elenca i profili installati sul telefono
```

### 4. AltServer per AltStore (consigliato)

```sh
./altkeeper altserver            # oppure: ./altkeeper serve --altserver (con la pagina web)
```

Lascialo acceso (come servizio o in Docker). Sull'iPhone, in AltStore, apri **My Apps** e tocca
**Refresh All**, oppure installa un'app: AltStore trova il server da solo. Ascolta sulla porta
**49500** (`--port N` per cambiarla) e si annuncia via Bonjour come `_altserver._tcp`.

- Sulla rete deve esserci un solo AltServer: ferma quello sul computer e non far girare due copie di
  questo programma.
- Provalo prima con una sola app poco importante, non con *Refresh All* su tutto.
- AltStore mostra *"AltServer could not be found"* per diversi tipi di connessione persa quando il
  server non è quello che preferisce, quindi il messaggio può nascondere la causa vera. Guarda
  l'output del server: registra ogni connessione, richiesta e risposta.
- La prima richiesta crea l'identità anisette in `state/` se non c'è ancora. L'abbiamo usato con
  un'identità creata da `login`; una cartella pulita non è stata provata.

### 5. Accesso (per il rinnovo automatico e l'installazione di IPA dal PC)

```sh
./altkeeper login tuo.apple.id@example.com
```

Chiede la password (**si vede sullo schermo mentre scrivi**) e il codice 2FA a 6 cifre che arriva
agli altri tuoi dispositivi. Rispondi `s` alla domanda sul salvare la password: è quello che permette
al rinnovo di girare senza di te. Viene salvata **in chiaro** in `account.json` (permessi 600).
Accedi una volta sola; se Apple risponde con errori, aspetta prima di riprovare invece di ripetere.

```sh
./altkeeper apps                                # i tuoi App ID e le scadenze
./altkeeper renew --dry-run --min-days 7        # mostra cosa rinnoverebbe, non cambia niente
./altkeeper renew --min-days 7                  # rinnova ora quello che scade entro 7 giorni
```

`renew` guarda il profilo più recente di ogni app, rinnova quelle con `--min-days` giorni o meno
(default 3) e accede ad Apple **solo se c'è qualcosa da rinnovare**.

### 6. Rinnovo automatico (facoltativo)

Una rete di sicurezza che non dipende dall'iPhone che esegue AltStore.

1. Copia `renew.sh` e `notifica.sh` da `examples/` accanto al binario e fai `chmod +x renew.sh`.
   Lascia `ALTKEEPER_PHONE` vuota in `renew.sh` (il telefono si trova da solo) oppure metti il suo
   `IP:porta`.
2. Installa il cron (modifica prima il percorso): `cp altkeeper.cron /etc/cron.d/altkeeper`

Parte ogni sera alle 21:17, rinnova quello che serve e aggiunge data ed esito a `renew.log`.
L'iPhone deve essere sul Wi-Fi di casa a quell'ora. Con profili da 7 giorni e soglia a 3 hai diverse
sere di margine prima che un'app scada.

### 7. Notifiche (facoltativo)

Crea `notify.conf` accanto agli script, con un nome di topic lungo e casuale:

```sh
echo 'NTFY_TOPIC=scegli-un-nome-lungo-e-casuale' > notify.conf && chmod 600 notify.conf
```

Installa l'app ntfy sul telefono e iscriviti a quel topic. Ricevi un messaggio quando le app vengono
rinnovate e quando un giro fallisce (al massimo un messaggio di errore ogni 24 ore, perché il
telefono può essere semplicemente fuori casa). Se non c'è niente da rinnovare non arriva nulla.
Chiunque conosca il topic può leggerlo: tienilo segreto.

### 8. Pagina web (facoltativa)

```sh
ALTKEEPER_WEB_PIN=123456 ./altkeeper serve --bind 0.0.0.0:8787 --altserver
```

Apri `http://<server>:8787` (nome utente qualsiasi, il PIN come password). Mostra se l'iPhone è
raggiungibile e le app con i giorni che mancano, permette di rinnovarle ("Rinnova quelle in scadenza"
o "Rinnova tutte adesso", con il log in diretta), di accedere con l'Apple ID (il codice 2FA si scrive
nel browser) e di vedere gli ultimi rinnovi da `renew.log`. Senza `--bind`, o con un indirizzo di
loopback, ascolta solo sulla macchina stessa e non chiede il PIN; su qualsiasi altro indirizzo
**rifiuta di partire senza PIN** (almeno 4 caratteri). Usa la variabile d'ambiente e non `--pin`, che
si vede con `ps`. La pagina web è in inglese e non c'è HTTPS: tienila dentro la rete di casa. L'accesso
e il rinnovo dalla pagina web non sono ancora stati provati con Apple.

Da un PC puoi anche caricare un file `.ipa` nella sezione **Install an app from this PC**: AltKeeper
lo firma con l'Apple ID configurato e lo installa sull'iPhone via Wi-Fi. Per farlo, l'account deve
avere la password salvata in `account.json`; il codice 2FA, se richiesto, si inserisce nella pagina.
La firma può registrare l'iPhone e creare identificativi e certificati di sviluppo sul team Apple.
L'upload è limitato a 1 GiB. Proteggi la pagina con PIN quando la esponi in rete e non inoltrare la
porta su Internet.

### 9. Docker

```sh
docker build --platform linux/amd64 -t altkeeper:latest .
```

L'immagine non contiene niente di segreto: pairing, account e stato stanno nel volume `/data`. Copia
`examples/docker-compose.yml` in `/srv/altkeeper/docker-compose.yml`, metti nella stessa cartella il
tuo `rp-pairing.plist` (e `account.json` e `state/` se usi `renew`), scrivi
`ALTKEEPER_WEB_PIN=...` in `/srv/altkeeper/.env` (permessi 600) e lancia `docker compose up -d`.
Usa la rete dell'host, perché serve Bonjour per trovare l'iPhone e per farsi trovare da AltStore; le
porte 8787 (web) e 49500 (AltServer) devono essere libere.

Un workflow di GitHub Actions (`.github/workflows/docker.yml`) definisce test Linux/Windows e
pubblica l'immagine su `ghcr.io/<utente>/altkeeper` a ogni push su `main` (amd64) e sui tag
`vX.Y.Z` (amd64 e arm64). Nel fork non risultano ancora esecuzioni di GitHub Actions: i test
Windows sono stati eseguiti localmente, non in CI. Se il repository è privato, anche il pacchetto lo è.

### 10. Windows (nativo, senza Docker)

Docker Desktop su Windows non può usare la rete dell'host, che AltKeeper richiede per Bonjour:
su Windows si lancia quindi il programma direttamente. **Verificato su Windows 10:**
`cargo test --locked` (63/63), `cargo build --release --locked`, `ALTKEEPER_LANG=en` con
`--help`, e login reale da PowerShell con codice 2FA di sei cifre e comandi del menu 2FA.
Queste verifiche **non** coprono Windows 11, tutti i terminali, AltServer o rinnovo automatico
end-to-end.

1. Installa [Rust](https://rustup.rs) (la toolchain MSVC predefinita) e i *Build Tools per Visual
   Studio* con il carico di lavoro **Sviluppo di applicazioni desktop con C++**. Poi, in PowerShell:

   ```powershell
   git clone https://github.com/lgg/altkeeper
   cd altkeeper
   cargo test --locked
   cargo build --release --locked
   ```

   Il programma è `target\release\altkeeper.exe`. Crea una cartella (per esempio `C:\altkeeper`) e
   copiaci l'`.exe` e, da `examples\`, `renew.ps1`.

2. **Pairing (una volta sola, via USB).** Se hai iTunes o l'app *Dispositivi Apple*, Windows ha già
   un usbmuxd in ascolto su `127.0.0.1:27015`: non serve nessun ponte (`usbmux-bridge.py` è per il
   Mac). Con l'iPhone collegato, sbloccato e autorizzato:

   ```powershell
   $env:USBMUXD_SOCKET_ADDRESS = "127.0.0.1:27015"
   .\altkeeper.exe pair-usb
   ```

   Poi `.\altkeeper.exe phone` deve elencare i profili sul telefono.

3. **Avvio.** Dalla cartella che hai creato:

   ```powershell
   .\altkeeper.exe serve --altserver      # pagina web su http://127.0.0.1:8787 + AltServer
   # oppure solo AltServer:
   .\altkeeper.exe altserver
   ```

   La prima volta Windows chiede di consentire il programma nel firewall: consentilo sulle reti
   **Private** (il tuo Wi-Fi deve essere impostato come *Privato*, non *Pubblico*). A mano, per
   AltServer (TCP 49500) e Bonjour (UDP 5353), da PowerShell come amministratore:

   ```powershell
   New-NetFirewallRule -DisplayName "AltKeeper" -Direction Inbound -Action Allow  -Program "C:\altkeeper\altkeeper.exe" -Profile Private
   ```

   Se su quel PC un altro servizio Bonjour usa già la UDP 5353 (il *Bonjour Service* di Apple,
   che arriva con iTunes, va bene), guarda "Se qualcosa va storto". Si ferma con Ctrl-C.

4. **Accesso e rinnovo automatico.** `.\altkeeper.exe login tu@esempio.it` funziona come sugli altri
   sistemi. In PowerShell e cmd.exe inserisci il codice 2FA di sei cifre e premi Invio;
   funzionano anche i comandi `d`, `r` e `p<id>`. AltKeeper legge le risposte
   direttamente dalla console di Windows: non è necessario passare a cmd.exe.
   **Login e menu 2FA verificati manualmente su Windows 10 PowerShell.** Avvia `login` in
   una console interattiva, non come attività in background. Consulta la
   [guida alla verifica Windows 2FA](docs/windows-2fa-smoke-test.md).
   `account.json`, `state\` e `profili-salvati\` restano riservati al tuo utente Windows
   (l'equivalente di `chmod 600/700`). Per il rinnovo serale usa l'Utilità di pianificazione con
   `renew.ps1` al posto di cron:

   ```powershell
   schtasks /Create /TN "AltKeeper renew" /SC DAILY /ST 21:17  /TR "powershell -NoProfile -ExecutionPolicy Bypass -File C:\altkeeper\renew.ps1"
   ```

   Per avviare AltServer quando accedi a Windows:

   ```powershell
   schtasks /Create /TN "AltKeeper" /SC ONLOGON  /TR "C:\altkeeper\altkeeper.exe altserver --dir C:\altkeeper"
   ```

   `renew.ps1` usa lo stesso `renew.log`, lo stesso `notify.conf` facoltativo (ntfy) e la stessa
   notifica d'errore al massimo una volta al giorno di `renew.sh`. Il PC deve essere acceso e non in
   sospensione a quell'ora.

Tutto il resto di questa guida (comandi, variabili d'ambiente, uso da fuori casa) vale uguale; cambia
solo che i percorsi usano `\`, e che `cron` e `renew.sh` si sostituiscono con l'Utilità di
pianificazione e `renew.ps1`.

## Usarlo da fuori casa

AltStore cerca i server con Bonjour, che di solito vuol dire multicast — e **iOS il multicast non
lo manda mai dentro una VPN**. Per questo installare Tailscale o ZeroTier da solo non basta: il
telefono non fa proprio la domanda.

La via d'uscita è la seconda modalità di Bonjour, più vecchia: il **DNS-SD unicast**, in cui i
servizi si pubblicano come normali record DNS. AltStore cerca in un dominio vuoto, cioè "tutti i
domini predefiniti", e iOS capisce quali siano chiedendolo al server DNS della rete in cui si
trova. Rispondendo a quella domanda, il telefono trova il server da solo — senza impostare niente
sul telefono.

AltKeeper quei record li serve da sé. Ecco tutto il procedimento.

### 1. Una VPN tra telefono e server

Va bene qualunque VPN, purché il telefono ci prenda un indirizzo. WireGuard è la scelta migliore:
è dentro il kernel Linux, l'app iOS è gratuita e regge meglio delle altre i cambi di rete.

Sul server:

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
PrivateKey = <contenuto di server.key>

[Peer]
PublicKey = <contenuto di phone.pub>
PresharedKey = <contenuto di psk.key>
AllowedIPs = 10.7.0.2/32
```

```
systemctl enable --now wg-quick@wg0
```

Sul router inoltra la porta **UDP 51820** a questa macchina. WireGuard non risponde mai ai
pacchetti che non sono crittograficamente validi, quindi la porta resta invisibile a chi
scansiona — ma è pur sempre una porta aperta, e conviene limitare cosa può raggiungere chi entra.
Aggiungi in `[Interface]`:

```ini
PostUp = iptables -I INPUT 1 -i wg0 -p udp --dport 53 -j ACCEPT
PostUp = iptables -I INPUT 2 -i wg0 -p tcp --dport 49500 -j ACCEPT
PostUp = iptables -I INPUT 3 -i wg0 -p tcp --dport 8787 -j ACCEPT
PostUp = iptables -I INPUT 4 -i wg0 -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
PostUp = iptables -A INPUT -i wg0 -j DROP
PostUp = iptables -I FORWARD 1 -i wg0 -j DROP
```

Così dal tunnel si raggiungono solo DNS, AltServer e la pagina web, e nient'altro: niente SSH e
nessuna via verso il resto della rete di casa. Ogni regola va tolta con un `PostDown = iptables -D …`
corrispondente.

La configurazione del telefono, che importi inquadrando un codice QR
(`qrencode -t ansiutf8 < phone.conf`):

```ini
[Interface]
PrivateKey = <contenuto di phone.key>
Address = 10.7.0.2/32
DNS = 10.7.0.1

[Peer]
PublicKey = <contenuto di server.pub>
PresharedKey = <contenuto di psk.key>
Endpoint = indirizzo-di-casa:51820
AllowedIPs = 10.7.0.0/24
PersistentKeepalive = 25
```

`AllowedIPs = 10.7.0.0/24` manda nel tunnel solo la sua rete, quindi la normale navigazione del
telefono non cambia. `Endpoint` vuole un indirizzo che segua la tua connessione di casa: se
l'operatore te lo cambia, serve un nome DNS dinamico.

### 2. Pubblicare i record

```
altkeeper altserver --dns-domain casa.internal --dns-address 10.7.0.1 --dns-clients 10.7.0.0/24
```

- `--dns-address` è l'indirizzo a cui il telefono deve collegarsi: quello di questa macchina
  **sulla VPN**.
- `--dns-clients` è la rete da cui arrivano i telefoni. Conta perché iOS scopre il dominio
  chiedendo del "rovescio" del proprio indirizzo; se non lo indichi, AltKeeper assume la `/24`
  attorno a `--dns-address`.
- Il server DNS ascolta sulla porta UDP 5533 (`--dns-port`), risponde solo per i propri nomi e
  non inoltra niente.

### 3. Far arrivare quelle risposte al telefono

Il DNS del telefono è il server (`DNS = 10.7.0.1` qui sopra), cioè il resolver che già usi. Basta
inoltrargli questo solo dominio — e la zona inversa della rete della VPN.

Con **AdGuard Home**, in *Server DNS upstream*:

```
[/casa.internal/]127.0.0.1:5533
```

e, siccome AdGuard manda le richieste inverse private a un elenco a parte, aggiungi anche in
*Server DNS inversi privati*:

```
[/7.10.in-addr.arpa/]127.0.0.1:5533
```

Con **dnsmasq**:

```
server=/casa.internal/127.0.0.1#5533
server=/7.10.in-addr.arpa/127.0.0.1#5533
```

Quella seconda riga è quella che sfugge a tutti, e senza di lei il telefono non scopre mai il
dominio.

### 4. La prova

Accendi la VPN, collega il telefono a un Wi-Fi che non sia quello di casa e premi *Refresh All*.
Sul server dovresti vedere prima la scoperta e poi il lavoro:

```
[altserver] 10.7.0.2: AnisetteDataRequest -> AnisetteDataResponse
[altserver] 10.7.0.2: InstallProvisioningProfilesRequest
[altserver] profilo installato: com.esempio.app
```

Se dice che non trova il server, guarda le richieste DNS con `tcpdump -i wg0 -n port 53` e controlla
quale nome sta chiedendo il telefono.

### Cosa aspettarsi

- **Il telefono deve essere su Wi-Fi.** iOS tiene acceso il servizio di pairing che serve ad
  AltKeeper solo quando è collegato a una rete Wi-Fi; con la sola rete dati la porta è chiusa, e
  non c'è modo di aggirarlo.
- **Il rinnovo automatico attraverso il tunnel non è ancora affidabile.** iOS sospende le app VPN
  in background, quindi il cron notturno può trovare il telefono irraggiungibile. Il rinnovo fatto
  a mano con il tunnel attivo funziona bene.
- Se il server trovava il telefono scandagliando la rete, scrivi il suo indirizzo VPN in
  `state/phone-addr` (`10.7.0.2:49152`), così lo prova per primo.

## Comandi

| Comando | Cosa fa |
| --- | --- |
| `phone [--phone ip:porta]` | elenca i profili installati sull'iPhone |
| `pair-usb [file]` | crea il pairing remoto via USB (una volta) |
| `login <apple-id>` | accede con 2FA e salva l'account |
| `apps` | elenca i tuoi App ID e le scadenze |
| `renew [--dry-run] [--force] [--min-days N] [--phone ip:porta]` | rinnova i profili in scadenza |
| `altserver [--port N] [--dir cartella] [--phone ip:porta] [--dns-domain D --dns-address IP]` | fa da AltServer per AltStore |
| `serve [--bind ip:porta] [--pin PIN] [--dir cartella] [--phone ip:porta] [--altserver] [--altserver-port N]` | pagina web (e, con `--altserver`, AltServer) |
| `profile-remove <uuid>` | toglie un profilo dal telefono, salvandone una copia in `profili-salvati/` |
| `profile-restore <file>` | rimette un profilo salvato |

`ALTKEEPER_PHONE=ip:porta` può sostituire `--phone`; senza nessuno dei due il telefono si trova da
solo (vedi il passo 2). `ALTKEEPER_NO_MDNS=1` spegne Bonjour (reti che bloccano il multicast, o per
provare la scansione). `ALTKEEPER_DEBUG=1` stampa cosa risponde Apple a ogni richiesta del login (stato,
intestazioni, corpo degli errori; mai il corpo delle richieste né le risposte riuscite).
`ALTKEEPER_ANISETTE_URL=https://...` usa un server anisette tuo (per esempio `anisette-v3-server`) al
posto di quello predefinito; deve iniziare con `http://` o `https://`. Se quello predefinito smette di funzionare, vedi "Se qualcosa va storto".

`altkeeper --help` (anche `altkeeper login --help`) mostra la sintassi nella lingua configurata.

Il progetto si chiamava altrefresh: i vecchi nomi `ALTREFRESH_*` funzionano ancora (se ci sono
entrambi vince `ALTKEEPER_*`). `altkeeper --version` stampa la versione.

## Se qualcosa va storto

- **"manca il pairing remoto"**: nella cartella non c'è `rp-pairing.plist`. Rifai il pairing
  (passo 3).
- **"pairing con l'iPhone non riuscito" / `early eof`**: l'iPhone non riconosce più quel pairing.
  Rifallo, oppure copia un `rp-pairing.plist` funzionante da un altro computer.
- **"non trovo l'iPhone in rete", "No route to host", timeout**: l'iPhone non è in rete (in standby,
  fuori casa). Se avevi indicato un indirizzo che non è più giusto, il programma cerca il telefono da
  solo; se vuoi un indirizzo fisso, sull'iPhone apri Impostazioni, Wi-Fi, la (i) della tua rete e
  imposta l'indirizzo Wi-Fi privato su **Fisso** o disattivato (la dicitura dipende dalla versione di
  iOS), poi dai all'iPhone un IP fisso nel router.
- **AltStore dice "AltServer could not be found"**: vedi il passo 4. Controlla che il server sia
  acceso, sulla stessa rete, che AltStore abbia il permesso Rete locale, e leggi l'output del server.
- **AltStore mostra un errore e nel log compare `not provisioned for this server (-45061)` o
  `reprovision required`**: il server anisette pubblico predefinito non accetta l'identità salvata in
  `state/anisette_state` (il server è cambiato o ha problemi). Metti da parte il file
  (`mv state/anisette_state state/anisette_state.old`) e avvia con un altro server, per esempio
  `ALTKEEPER_ANISETTE_URL=https://ani3.stikstore.app altkeeper altserver`. Si segnala che funziona
  anche `https://mac-ani.jkcoxson.com`. Lascia la variabile impostata in modo permanente (profilo
  della shell, `.env` del compose, oppure `Environment=` in un'unità systemd), altrimenti al prossimo
  avvio si torna al server predefinito. Dopo la creazione della nuova identità Apple può chiedere di
  nuovo un codice di verifica. I server pubblici vanno e vengono: se fallisce ancora, prova un altro,
  oppure usane uno tuo (`anisette-v3-server`).
- **Un refresh da AltStore sembra bloccato subito dopo `PrepareAppRequest`**: AltStore manda prima
  l'app al server, e può pesare ben più di 100 MB. Con un Wi-Fi debole ci vogliono minuti; il server
  scrive `app in arrivo: X di Y byte` ogni pochi secondi mentre arriva, e rinuncia dopo 10 minuti in
  totale. Tieni AltStore aperto con lo schermo acceso e l'iPhone vicino al router.
- **Errori 429 o 503 al login**: Apple sta limitando le richieste. Non riprovare a ripetizione.
  Lancia con `ALTKEEPER_DEBUG=1` e, se persiste, apri una issue con le righe che iniziano con
  `[gs #` (non contengono segreti).
- **Un'app non si apre dopo un rinnovo**: reinstallala con AltStore o con lo strumento che usi di
  solito. Se avevi tolto un profilo con `profile-remove`, rimettilo con `profile-restore`.

## Sicurezza

Questi file contengono segreti. Sono ignorati da git e vanno tenuti con permessi `600`:

- `rp-pairing.plist`: le chiavi del pairing con il telefono.
- `account.json`: l'Apple ID e, se hai scelto di salvarla, la password **in chiaro**.
- `state/`: l'identità anisette del "dispositivo" e il `serverID` di AltServer.
- `notify.conf`: il topic di ntfy.
- `profili-salvati/`: le copie dei profili tolti dal telefono con `profile-remove`.

Per segnalare una vulnerabilità in privato vedi [SECURITY.md](SECURITY.md).

Non incollare mai la password, `account.json` o il file del pairing in una issue o in una chat.
Non usare `RUST_LOG=debug` su log condivisi: a quel livello `idevice` stampa le chiavi del pairing.
Tieni la macchina che li conserva dentro la rete di casa.

**La porta di AltServer (49500) non ha un accesso, come il vero AltServer**: chiunque sulla tua rete
parli il protocollo può chiedere i dati anisette o mandare profili e app all'iPhone accoppiato. Non
esporre quella porta fuori dalla rete di casa.

## Come funziona

1. L'iPhone (iOS 17+) annuncia in rete il servizio Bonjour `_remotepairing._tcp`.
2. Con un pairing remoto già creato, il programma fa il *pair-verify*, chiede al telefono di aprire
   un tunnel e lo raggiunge con TLS-PSK ([`idevice`](https://github.com/jkcoxson/idevice)).
3. Dentro il tunnel legge e installa i profili con `misagent`, e installa o toglie le app con
   `installation_proxy`.
4. In modalità AltServer, AltStore trova la macchina con Bonjour (`_altserver._tcp`, con un record
   `serverID`) e manda richieste JSON su TCP (la lunghezza a 32 bit little-endian, poi il JSON), una
   connessione per operazione: dati anisette, profili da installare o togliere, e l'app firmata.
5. Per il rinnovo senza controllo, con l'Apple ID ([`isideload`](https://github.com/nab138/isideload))
   scarica i profili nuovi per gli App ID esistenti.

Il vecchio Wi-Fi sync (`netmuxd`, AltServer-Linux) su iOS recenti viene rifiutato dal telefono: il
pairing remoto è il canale che funziona.

## Note sulla copia di `isideload` in `vendor/`

`isideload` 0.3.17 ha due problemi con i server di login di Apple, quindi `vendor/isideload` è
una copia modificata dell'originale (MIT, © nab138):

- **503 al login.** Si presenta come `com.apple.dt.Xcode`, che Apple rifiuta.
  `src/anisette/remote_v3/mod.rs` ora dichiara `com.apple.akd/1.0` (identità `Mac15,7`,
  macOS 27.0) e l'User-Agent AuthKit che usa AltSign.
- **429 alla prova della password.** Il bordo di Apple lascia passare poche richieste su una
  stessa connessione e rifiuta le altre. Il login ne manda tre di fila (URL bag, `init`,
  `complete`), quindi la terza prendeva 429. AltSign lo evita con una connessione nuova per
  ogni richiesta (`ALTAppleAPI+Authentication.swift`, ramo notarized); `src/auth/grandslam.rs`
  ora fa lo stesso (solo HTTP/1.1, niente riuso delle connessioni, `Connection: close`). Su
  macOS usa anche il TLS di sistema. Quale di queste modifiche sia decisiva non è stato isolato.

Espone anche alcuni accessori in sola lettura di `AnisetteData`, usati per rispondere ad AltStore.

## Crediti e licenza

Costruito su [`idevice`](https://github.com/jkcoxson/idevice) (Jackson Coxson) e
[`isideload`](https://github.com/nab138/isideload) (nab138), entrambi con licenza MIT. La
correzione del login segue quello che fa [AltSign](https://github.com/rileytestut/AltSign)
(Riley Testut), e i messaggi di AltServer seguono il protocollo di
[AltStore](https://github.com/altstoreio/AltStore). Licenza di questo progetto: [MIT](LICENSE).
