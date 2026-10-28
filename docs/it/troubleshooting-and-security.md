# Problemi e sicurezza

## Problemi comuni

| Problema | Controlli |
| --- | --- |
| Manca il pairing remoto | Controlla che `rp-pairing.plist` sia nella cartella dati attiva. Se serve, rifai il pairing via USB. |
| Il pairing fallisce o mostra `early eof` | Il telefono potrebbe non riconoscere più il pairing. Ripetilo e tieni l'iPhone sbloccato durante la procedura. |
| L'iPhone non viene trovato o va in timeout | Collega l'iPhone al Wi-Fi, verifica che i dispositivi si raggiungano e controlla l'eventuale indirizzo fisso `--phone ip:porta`. |
| AltStore dice “AltServer could not be found” | Controlla che AltKeeper sia attivo, che AltStore abbia il permesso Rete locale e che ci sia un solo AltServer. Leggi il log del server. |
| AltStore segnala Anisette `-45061` o `reprovision required` | Prova un altro URL Anisette dalla pagina web. Cambiando server l'identità salvata viene sostituita e Apple potrebbe chiedere di nuovo la verifica. |
| Il refresh si blocca dopo `PrepareAppRequest` | L'iPhone potrebbe stare ancora caricando un'app pesante. Tieni AltStore aperto, usa una rete Wi-Fi stabile e controlla il log di trasferimento. |
| L'accesso Apple restituisce 429 o 503 | Apple potrebbe limitare le richieste. Non riprovare in ciclo; aspetta prima di tentare di nuovo. |
| Un'app non si apre dopo la rimozione di un profilo | Ripristina il profilo di provisioning salvato con `profile-restore`. |
| Il server Anisette locale non si avvia | Controlla entrambi i file nativi, la cartella dell'architettura e i permessi di lettura. Vedi [Server Anisette locale](anisette-server.md). |

Per la sintassi completa, esegui `altkeeper --help`. Imposta `ALTKEEPER_DEBUG=1` per i problemi di
accesso; condividi solo le righe specifiche `[gs #...]` dopo aver verificato che non contengano dati
privati. Non attivare `RUST_LOG=debug` nei log da condividere: i log di basso livello possono
contenere le chiavi di pairing.

## Proteggi la cartella dati

Considera segreti questi file e cartelle:

- `rp-pairing.plist`: chiavi usate per associare l'iPhone.
- `account.json`: Apple ID e, se salvata, password in chiaro.
- `state/`: identità Anisette e stato AltServer.
- `notify.conf`: topic delle notifiche.
- `profili-salvati/`: copie dei profili di provisioning rimossi.

Limita l'accesso ai file, cifra i backup e non allegarli a issue o chat. Per segnalare una
vulnerabilità in privato, segui [SECURITY.md](../../SECURITY.md).

## Esposizione in rete

- **AltServer TCP 49500:** non ha autenticazione, come AltServer stesso. Non esporlo fuori da una
  LAN o VPN fidata.
- **Pagina web TCP 8787:** usa `ALTKEEPER_WEB_PIN` con bind non locale. Non usa HTTPS: mantienila su
  una rete fidata.
- **Listener Anisette:** non ha autenticazione né TLS e il PIN web non lo protegge. Associalo solo
  a interfacce fidate.
- Evita l'inoltro di porte pubbliche. Usa una VPN privata e regole firewall sull'host.
