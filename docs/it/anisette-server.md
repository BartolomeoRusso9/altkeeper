# Server Anisette V3 locale

AltKeeper può ospitare facoltativamente un servizio Anisette V3. Non si avvia da solo ed è
indipendente da AltServer e dalla pagina web.

## Fornisci le librerie native

L'operatore deve fornire entrambe le librerie native: AltKeeper non le scarica e non le include.

```text
<cartella-dati>/anisette-libraries/lib/<architettura>/libstoreservicescore.so
<cartella-dati>/anisette-libraries/lib/<architettura>/libCoreADI.so
```

Usa il nome di cartella previsto dal motore:

| Architettura host | Nome cartella |
| --- | --- |
| x86-64 | `x86_64` |
| x86 | `x86` |
| ARM64 | `arm64-v8a` |
| ARM 32 bit | `armeabi-v7a` |

Usa librerie che sei autorizzato a utilizzare e rispetta le relative licenze. La cartella radice
predefinita è `<cartella-dati>/anisette-libraries`; `--anisette-libs <cartella>` la sostituisce.
Il percorso deve contenere le sottocartelle `lib/<architettura>/` mostrate sopra.

## Avvia il listener

Mantienilo su un'interfaccia LAN o VPN fidata. Esempio con la pagina web:

```sh
ALTKEEPER_WEB_PIN='il-tuo-pin-privato' ./AltKeeper serve \
  --bind 0.0.0.0:8787 \
  --anisette-bind 192.168.1.20:8788
```

Su Windows usa l'eseguibile nativo da PowerShell. `--anisette-libs` è facoltativo se usi il
percorso predefinito. In Docker su Linux, monta le librerie in sola lettura in
`/data/anisette-libraries` e aggiungi `--anisette-bind 192.168.1.20:8788` al comando `serve`.

Seleziona **AltKeeper local server** nelle impostazioni Anisette della pagina web quando vuoi che
AltKeeper usi questo server. Un altro client sulla LAN può usare `http://192.168.1.20:8788`.

## Scegli quale server Anisette usa AltKeeper

Dalla pagina web puoi scegliere il server locale AltKeeper, il server pubblico isideload configurato
(`https://ani.jkcoxson.com`) oppure un URL HTTP(S) personalizzato. Il server predefinito è
`https://ani3.stikstore.app`. L'URL selezionato viene salvato in `state/anisette-url`.

Se imposti `ALTKEEPER_ANISETTE_URL`, questa variabile prevale e blocca la scelta dalla pagina web.
Rimuovila per poter scegliere dall'interfaccia. Cancellando la selezione salvata si ripristina il
server predefinito. Quando cambia il server effettivo, AltKeeper mette da parte la vecchia identità
Anisette; Apple potrebbe chiederti di verificare di nuovo l'account.

## Sicurezza di rete e dati

Il protocollo Anisette V3 non prevede autenticazione né TLS. Il PIN della pagina web **non**
protegge questo listener. Ascolta solo su un'interfaccia raggiungibile da dispositivi fidati,
limita l'accesso con il firewall e non esporre la porta direttamente su Internet.

Lo stato di provisioning è temporaneo e salvato in `state/anisette-provisioning/`. Se il servizio
non si avvia, controlla che entrambi i file si trovino nella cartella esatta dell'architettura e
che il processo possa leggerli.
