# Account e rinnovi

AltKeeper offre due flussi distinti:

- **Aggiornamento AltServer:** AltStore gestisce l'accesso Apple sull'iPhone. Per questo flusso
  AltKeeper non ha bisogno della password dell'Apple ID.
- **Rinnovo indipendente:** AltKeeper accede ad Apple per rinnovare i profili senza aspettare
  AltStore. Serve un Apple ID il cui team abbia firmato le app.

## Accedi

Dalla cartella dati persistente, esegui:

```sh
./AltKeeper login tuo-apple-id@example.com
```

Completa le richieste per password e verifica a due fattori. Salvare la password consente i rinnovi
senza intervento. AltKeeper salva l'eventuale password in `account.json` **in chiaro**: limita
l'accesso alla cartella dati e proteggi i backup. Non inviare questo file ad altri.

Controlla account e profili installati:

```sh
./AltKeeper apps
./AltKeeper phone
```

## Rinnova manualmente

Prima simula il rinnovo:

```sh
./AltKeeper renew --dry-run --min-days 7
```

Poi rinnova i profili che scadono entro la soglia scelta:

```sh
./AltKeeper renew --min-days 7
```

La soglia predefinita è 3 giorni. Un rinnovo normale controlla prima le app installate e salta i
profili che non corrispondono ad app presenti. Se non riesce a leggere l'elenco delle app installate,
AltKeeper registra un avviso e passa a tutti i profili noti del team configurato. Se la lettura
riesce ma restituisce zero app, non esegue rinnovi.

Usa `--bundle <bundle-id>` per scegliere un'app. `--force` ignora la normale soglia di scadenza.
Con `--installed-only` puoi richiedere un controllo riuscito delle app installate anche durante un
rinnovo CLI forzato. Per tutte le opzioni esegui `altkeeper renew --help`.

### Registra automaticamente gli App ID mancanti

L'opzione **Registra automaticamente gli App ID mancanti** della pagina web è disattivata per
impostazione predefinita. Se la attivi, il rinnovo registra un App ID mancante prima di scaricarne
il profilo, consumando la quota dell'Apple ID gratuito di 10 App ID ogni 7 giorni. Se la quota è
esaurita, il rinnovo fallisce esplicitamente invece di saltare l'app; le dry-run non registrano.

## Pianifica i rinnovi

Scegli un solo sistema di pianificazione:

- L'opzione **Automatic renewal** della pagina web esegue un tentativo al giorno, all'ora UTC
  selezionata. È disattivata inizialmente e richiede server acceso, password Apple ID salvata e
  iPhone raggiungibile. Ogni profilo dispositivo ha la propria pianificazione. È consentita una
  sola operazione sul telefono alla volta.
- Gli script di esempio [`renew.sh`](../../examples/renew.sh) e
  [`renew.ps1`](../../examples/renew.ps1) possono essere avviati da cron o dall'Utilità di
  pianificazione di Windows.

Non attivare sia la pianificazione integrata sia cron/Utilità di pianificazione, a meno che tu non
voglia deliberatamente due tentativi. Lo scheduler esterno deve usare la cartella dati corretta e
proteggere log e credenziali.

## Più iPhone e account

Nella pagina web usa il selettore dispositivi e **Add device** per creare un profilo nominativo per
un altro iPhone. Seleziona un profilo prima di associare il telefono o accedere con il relativo
Apple ID. Ogni profilo conserva account, pairing, indirizzi salvati, impostazioni, pianificazione,
log e backup separati. Il profilo predefinito mantiene i dati esistenti; i nuovi profili sono
salvati in `devices/`.

AltServer resta un singolo servizio di rete e usa il profilo dispositivo selezionato. Non avviare
più copie di AltKeeper per servire più profili contemporaneamente.

## Rimuovi e ripristina profili

La pagina web può rimuovere i singoli profili di provisioning dopo una conferma. Prima della
rimozione, AltKeeper salva e verifica una copia in `profili-salvati/`. Se rimuovi un profilo ancora
necessario a un'app installata, l'app potrebbe non aprirsi. Ripristina una copia con:

```sh
./AltKeeper profile-restore profili-salvati/<file-profilo>
```

Il rinnovo non rimuove i vecchi profili e non revoca gli App ID Apple Developer.
