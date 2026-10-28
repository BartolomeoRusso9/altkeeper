# AltKeeper

AltKeeper mantiene utilizzabili le app installate su iPhone rinnovando i loro profili di
provisioning. Può anche comportarsi da AltServer per AltStore Classic, così puoi aggiornare o
installare app via Wi-Fi. Puoi usarlo su un server di casa, su Mac o su Windows.

> **Software sperimentale.** AltKeeper usa API e protocolli privati di Apple, che possono cambiare
> senza preavviso. L'attivazione JIT e alcuni flussi di installazione non sono ancora stati
> verificati completamente su dispositivi reali. Usalo con i tuoi dispositivi e il tuo Apple ID;
> non è affiliato ad Apple né ad AltStore.

## Scegli una guida

| Voglio… | Guida |
| --- | --- |
| Configurare AltKeeper e associare l'iPhone | [Guida introduttiva](docs/it/quick-start.md) |
| Scegliere Docker, Windows o macOS | [Installazione e avvio](docs/it/deployment.md) |
| Ospitare un server Anisette V3 | [Server Anisette locale](docs/it/anisette-server.md) |
| Accedere, rinnovare le app o gestire più dispositivi | [Account e rinnovi](docs/it/accounts-and-renewals.md) |
| Usare la pagina web, installare IPA o attivare il JIT | [Web e JIT](docs/it/web-and-jit.md) |
| Collegarsi da fuori casa | [Accesso remoto](docs/it/remote-access.md) |
| Risolvere problemi o proteggere i dati | [Problemi e sicurezza](docs/it/troubleshooting-and-security.md) |

Preferisci l'inglese? Apri il [README in inglese](README.md) o le [guide in inglese](docs/en/README.md).

## In breve

- **Modalità AltServer:** AltStore Classic può aggiornare e installare app tramite AltKeeper.
- **Rinnovo indipendente:** rinnova i profili dalla CLI, dalla pagina web o con una pianificazione
  facoltativa.
- **Profili dispositivo:** pairing, account, impostazioni e rinnovi restano separati per ogni iPhone.
- **Anisette V3 locale:** server facoltativo per Linux, macOS e Windows; le librerie native vanno
  fornite dall'operatore.
- **JIT:** disponibile tramite pagina web e API HTTP per client compatibili. Servono Modalità
  sviluppatore e un'immagine Developer Disk Image compatibile già montata.

CLI e server supportano Linux, macOS e Windows; l'app nella barra dei menu è disponibile su macOS e
Windows. Docker è pensato per host Linux che supportano rete host e Bonjour.

Per segnalare una vulnerabilità in privato, consulta [SECURITY.md](SECURITY.md). Le licenze di terze
parti sono elencate in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
