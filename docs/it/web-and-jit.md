# Pagina web e JIT

## Avvia la pagina web

Per accedere da altri dispositivi fidati della LAN, imposta un PIN e associa il server web a un
indirizzo di rete:

```sh
ALTKEEPER_WEB_PIN='il-tuo-pin-privato' ./AltKeeper serve \
  --bind 0.0.0.0:8787 --altserver
```

Apri `http://<indirizzo-server>:8787`. Usa un nome utente qualsiasi e il PIN come password. Un
bind web non locale richiede `ALTKEEPER_WEB_PIN` (almeno quattro caratteri). Un server limitato a
loopback non richiede il PIN. Il servizio web non usa HTTPS: mantienilo su una rete fidata e non
inoltrare la porta direttamente su Internet.

La pagina mostra raggiungibilità del telefono, app e scadenze, log e stato dei rinnovi. Offre anche
accesso Apple, pairing, profili dispositivo, scelta Anisette, pianificazione rinnovi e gestione dei
profili. Per password e pianificazione, vedi [Account e rinnovi](accounts-and-renewals.md).

## Installa un IPA dal computer

Il caricamento IPA dalla pagina web firma e installa il file selezionato usando l'Apple ID del
profilo dispositivo attivo. L'operazione può registrare l'iPhone e creare App ID e certificati di
sviluppo nel team Apple. Il limite di caricamento è 1 GiB. Mantieni privata la pagina e controlla
il log dell'operazione.

L'opzione JIT facoltativa prova ad avviare l'app installata e ad abilitare il JIT. Se l'attivazione
fallisce, l'app resta installata e il log segnala l'errore.

## Requisiti e stato del JIT

Il JIT può essere richiesto dalla pagina per un'app in esecuzione, durante l'installazione IPA
supportata o da una richiesta AltStore. Servono Modalità sviluppatore attiva e un'immagine
Developer Disk Image compatibile già montata sull'iPhone. AltKeeper non monta automaticamente
l'immagine. L'attivazione JIT non è ancora stata verificata completamente su un dispositivo reale.

L'API HTTP è specifica di AltKeeper. Non implementa il protocollo Husk/StikJIT: i client devono
integrare esplicitamente l'API di AltKeeper.

## API per client compatibili

URL base: `http://<indirizzo-server>:8787`. Se è impostato un PIN, usa l'autenticazione HTTP Basic
(nome utente qualsiasi, PIN come password). Le richieste POST devono contenere
`X-Requested-With: altkeeper`. I lavori partono in background: avvia un lavoro e interroga
`/api/state` finché `job.running` diventa `false`.

Prima carica l'elenco dei processi attualmente in esecuzione:

```sh
export ALTKEEPER_URL='http://192.168.1.20:8787'
export ALTKEEPER_WEB_PIN='il-tuo-pin'
curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
  -H 'X-Requested-With: altkeeper' -H 'Content-Type: application/json' \
  -d '{}' "$ALTKEEPER_URL/api/jit/apps"
while :; do
  state=$(curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
    "$ALTKEEPER_URL/api/state") || exit 1
  printf '%s\n' "$state" | jq '{job, jit_apps}' || exit 1
  running=$(printf '%s\n' "$state" | jq -er '.job.running | tostring') || exit 1
  [ "$running" = false ] && break
  sleep 1
done
```

Quando il lavoro è terminato, usa un PID presente nell'elenco `jit_apps` restituito:

```sh
curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
  -H 'X-Requested-With: altkeeper' -H 'Content-Type: application/json' \
  -d '{"pid":1234}' "$ALTKEEPER_URL/api/jit/enable"
curl -fsS -u "altkeeper:$ALTKEEPER_WEB_PIN" \
  "$ALTKEEPER_URL/api/state" | jq '.job'
```

Aggiorna l'elenco prima di abilitare il JIT. AltKeeper accetta solo un PID dell'elenco caricato più
di recente e verifica che il processo sia ancora un'app in esecuzione. Per un profilo diverso da
quello predefinito, aggiungi `X-AltKeeper-Device: <profile-id>` a ogni richiesta. Non esporre mai
l'API su Internet.
