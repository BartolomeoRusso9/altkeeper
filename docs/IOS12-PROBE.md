# Probe lockdown USB per iOS 12

Questo probe è il primo controllo da eseguire prima di refactorizzare `PhoneLink`. Non modifica il
codice di produzione di AltKeeper e non scrive nulla sull'iPhone: legge la versione, avvia una
sessione lockdown e legge l'elenco dei profili con Misagent.

## Preparazione

1. Collega l'iPhone via USB, sbloccalo e conferma che si fidi di questo computer.
2. Assicurati che il servizio `usbmuxd` sia raggiungibile. Se la piattaforma non usa il socket
   predefinito, imposta `USBMUXD_SOCKET_ADDRESS` secondo la configurazione già usata da AltKeeper.
   Per esempio, in PowerShell con Apple Devices/iTunes:

   ```powershell
   $env:USBMUXD_SOCKET_ADDRESS = "127.0.0.1:27015"
   ```

3. Dalla radice del repository esegui prima la verifica di compilazione:

   ```sh
   cargo check --bin ios12_lockdown_probe
   ```

4. Con l'iPhone collegato, esegui:

   ```sh
   cargo run --bin ios12_lockdown_probe
   ```

## Come interpretare il risultato

- Fallimento di `LockdownClient::connect`: il trasporto USB o il servizio lockdown non è disponibile.
- Fallimento di `get_pairing_file` / `start_session`: controllare pairing e avvio sessione. `start_session`
  è particolarmente importante perché verifica anche la negoziazione TLS usata dal percorso classico.
- `start_session` riuscito ma `DeviceName` fallito: salvare l'errore completo; il percorso dopo l'avvio
  della sessione necessita di ulteriore indagine.
- Lockdown riuscito ma `MisagentClient::connect` o `copy_all` fallisce: il problema è nel servizio
  Misagent o nella sua apertura tramite il provider, non nel parsing della versione.
- Tutte le fasi riuscite: è una prova concreta che il trasporto lockdown USB e la lettura Misagent
  funzionano su quel dispositivo. Non dimostra ancora che l'installazione IPA o la firma Apple ID
  gratuito vengano accettate da iOS 12.

Il probe usa le API di `idevice` già referenziate dal progetto, ma non posso compilarlo o verificarlo
contro un iPhone da questo ambiente. Se non compila con il `Cargo.lock` del repository, conserva il
primo errore Rust e correggi l'API prima di procedere alla refactor.

## Prima di proseguire

Soltanto dopo che il probe termina con `OK`, applicare separatamente la modifica di architettura
(versione → trasporto legacy/RSD → `PhoneLink`) e rifare i test. La versione minima supportata, il
rinnovo dei profili, l'installazione IPA e l'integrazione AltServer devono restare marcati come non
verificati finché non vengono collaudati su hardware. Le versioni iOS 13–16 vanno dichiarate non
testate finché non si dispone di prove anche su quelle versioni.
