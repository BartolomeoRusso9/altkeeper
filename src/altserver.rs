//! Funzione AltServer: AltStore Classic, sul telefono, ci trova via Bonjour e ci manda le sue
//! richieste, così il tasto *Refresh All* e l'installazione delle app funzionano senza AltServer
//! su un computer e senza VPN.
//!
//! Il protocollo è quello di `Shared/Server Protocol/ServerProtocol.swift` di AltStore: TCP
//! semplice, ogni messaggio è un intero a 32 bit (little-endian) con la lunghezza e poi un JSON.
//! AltStore apre una connessione nuova per ogni operazione. Il servizio Bonjour è
//! `_altserver._tcp` e DEVE avere il record TXT `serverID`, altrimenti AltStore lo ignora.
//!
//! Cosa fa per ogni richiesta:
//! - `AnisetteDataRequest`: dati anisette (stessa identità del login), per il login di AltStore.
//! - `InstallProvisioningProfilesRequest`: installa i profili che AltStore ha scaricato da Apple
//!   (è il rinnovo). Non toglie mai profili dal telefono, nemmeno se `activeProfiles` lo chiede.
//! - `RemoveProvisioningProfilesRequest`: toglie i profili di quei bundle.
//! - `PrepareAppRequest` + byte dell'IPA + `BeginInstallationRequest`: installa l'app già firmata
//!   da AltStore, con risposte di avanzamento fino a 1.0.
//! - `RemoveAppRequest`: disinstalla un'app.
//! - `EnableUnsignedCodeExecutionRequest` (JIT): non supportata, risponde con un errore.
//!
//! Ogni connessione gira in un thread con un runtime suo (il collegamento al telefono non è
//! `Send`) e una richiesta alla volta.

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use idevice::{RsdService, installation_proxy::InstallationProxyClient};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;

use crate::{PAIRING_FILE, apple, phone, profile};

const SERVICE_TYPE: &str = "_altserver._tcp.local.";
const MAX_JSON: usize = 4 * 1024 * 1024;
/// L'IPA si tiene in memoria: oltre questa soglia si rifiuta.
const MAX_IPA: u64 = 1024 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(120);
const IPA_TIMEOUT: Duration = Duration::from_secs(600);

// Codici di `ALTServerError` (Shared/Categories/NSError+ALTServerError.h).
const ERR_DEVICE_NOT_FOUND: i32 = 3;
const ERR_INVALID_REQUEST: i32 = 5;
const ERR_INVALID_APP: i32 = 7;
const ERR_INSTALLATION_FAILED: i32 = 8;
const ERR_UNSUPPORTED_IOS: i32 = 10;
const ERR_UNKNOWN_REQUEST: i32 = 11;
const ERR_INVALID_ANISETTE: i32 = 13;
const ERR_APP_DELETION_FAILED: i32 = 16;

/// Una sola operazione alla volta sul telefono. Si prende SOLO mentre si usa il telefono:
/// AltStore lascia aperta una connessione mentre ne apre un'altra, e un blocco tenuto per tutta
/// la connessione fa scadere la seconda (e AltStore mostra "AltServer could not be found").
static WORK: Mutex<()> = Mutex::new(());

pub(crate) fn phone_lock() -> std::sync::MutexGuard<'static, ()> {
    WORK.lock().unwrap_or_else(|e| e.into_inner())
}
/// Nome completo del servizio annunciato, per toglierlo alla chiusura.
static SERVICE_FULLNAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();

#[derive(Clone)]
struct Ctx {
    phone: Option<SocketAddr>,
}

fn log(msg: &str) {
    println!("[altserver] {msg}");
}

// ---------------------------------------------------------------------------------------------
// Avvio: ascolto e annuncio Bonjour
// ---------------------------------------------------------------------------------------------

/// Si mette in ascolto sulla porta indicata (0 = una a caso), annuncia il servizio e torna:
/// le connessioni le gestiscono thread propri. Restituisce la porta.
pub fn start(port: u16, phone: Option<SocketAddr>) -> Result<u16, String> {
    let listener = TcpListener::bind(("0.0.0.0", port))
        .map_err(|e| crate::tr!(format!("AltServer: non riesco ad ascoltare sulla porta {port}: {e}"), format!("AltServer: cannot listen on port {port}: {e}")))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let id = server_id()?;
    register_bonjour(port, &id)?;
    log(&crate::tr!(format!("in ascolto sulla porta {port}, serverID {}...", &id[..8]), format!("listening on port {port}, serverID {}...", &id[..8])));

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let ctx = Ctx { phone };
            let _ = std::thread::Builder::new().stack_size(16 * 1024 * 1024).spawn(move || {
                let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build()
                else {
                    return;
                };
                rt.block_on(async move {
                    if stream.set_nonblocking(true).is_err() {
                        return;
                    }
                    let Ok(stream) = TcpStream::from_std(stream) else { return };
                    handle_connection(stream, ctx, &peer).await;
                });
            });
        }
    });
    Ok(port)
}

/// Identificativo stabile del server (32 caratteri esadecimali casuali), in `state/altserver-id`.
fn server_id() -> Result<String, String> {
    let path = Path::new("state").join("altserver-id");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if s.len() >= 8 {
            return Ok(s);
        }
    }
    let mut bytes = [0u8; 16];
    // Generatore del sistema operativo, uguale su Linux, macOS e Windows (rustls è già una
    // dipendenza: nessun crate in più).
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| crate::tr!("non riesco a generare il serverID: numeri casuali non disponibili", "cannot generate serverID: secure randomness unavailable").to_string())?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::create_dir_all("state");
    std::fs::write(&path, &id).map_err(|e| crate::tr!(format!("non riesco a salvare il serverID: {e}"), format!("cannot save serverID: {e}")))?;
    Ok(id)
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        // Su Windows, se `hostname` non risponde, c'è sempre COMPUTERNAME.
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|s| s.trim().split('.').next().unwrap_or("").to_string())
        // Un nome fatto solo di cifre (a volte è un pezzo di IP) non dice niente.
        .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| "altkeeper".into())
}

/// Il nome con cui questo server si presenta, lo stesso usato nell'annuncio Bonjour.
pub fn instance_name() -> String {
    format!("altkeeper-{}", hostname())
}

/// Il serverID: AltStore riconosce il server da questo, non dall'indirizzo.
pub fn id() -> Result<String, String> {
    server_id()
}

fn register_bonjour(port: u16, id: &str) -> Result<(), String> {
    let daemon = phone::mdns().ok_or(crate::tr!("Bonjour non disponibile", "Bonjour unavailable"))?;
    let host = hostname();
    let props = [("serverID", id)];
    let info = mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &format!("altkeeper-{host}"),
        // Nome host Bonjour stabile e unico, che non dipende dal nome del computer.
        &format!("altkeeper-{}.local.", &id[..8]),
        "",
        port,
        &props[..],
    )
    .map_err(|e| format!("Bonjour: {e}"))?
    .enable_addr_auto();
    let _ = SERVICE_FULLNAME.set(info.get_fullname().to_string());
    daemon.register(info).map_err(|e| format!("Bonjour: {e}"))?;
    Ok(())
}

/// Toglie l'annuncio Bonjour mandando il "commiato": AltStore e gli altri telefoni non tengono
/// in memoria un server che non c'è più.
pub fn stop() {
    if let (Some(daemon), Some(name)) = (phone::mdns(), SERVICE_FULLNAME.get()) {
        if let Ok(done) = daemon.unregister(name) {
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
        log(crate::tr!("annuncio Bonjour tolto", "Bonjour announcement removed"));
    }
}

// ---------------------------------------------------------------------------------------------
// Messaggi
// ---------------------------------------------------------------------------------------------

/// Legge un messaggio: `None` se l'altro ha chiuso tra un messaggio e l'altro.
async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut len = [0u8; 4];
    match tokio::time::timeout(IO_TIMEOUT, r.read_exact(&mut len)).await {
        Err(_) => return Err(crate::tr!("nessuna richiesta entro il tempo limite", "no request received before timeout").into()),
        Ok(Err(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            ) =>
        {
            return Ok(None);
        }
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(_)) => {}
    }
    let n = i32::from_le_bytes(len);
    if n <= 0 || n as usize > MAX_JSON {
        return Err(crate::tr!(format!("lunghezza del messaggio non valida: {n}"), format!("invalid message length: {n}")));
    }
    let mut body = vec![0u8; n as usize];
    tokio::time::timeout(IO_TIMEOUT, r.read_exact(&mut body))
        .await
        .map_err(|_| crate::tr!("messaggio incompleto: tempo scaduto", "incomplete message: timed out").to_string())?
        .map_err(|e| e.to_string())?;
    Ok(Some(body))
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, v: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    let mut out = (body.len() as i32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    w.write_all(&out).await.map_err(|e| e.to_string())
}

fn simple(identifier: &str) -> Value {
    json!({ "version": 1, "identifier": identifier })
}

fn progress(p: f64) -> Value {
    json!({ "version": 1, "identifier": "InstallationProgressResponse", "progress": p })
}

/// `ErrorResponse` v3 con il solo codice (`errorCode`, il campo storico).
fn error(code: i32) -> Value {
    json!({ "version": 3, "identifier": "ErrorResponse", "errorCode": code })
}

// ---------------------------------------------------------------------------------------------
// Una connessione
// ---------------------------------------------------------------------------------------------

async fn handle_connection(stream: TcpStream, ctx: Ctx, peer: &str) {
    let (mut rd, wr) = stream.into_split();
    let wr = Rc::new(tokio::sync::Mutex::new(wr));
    log(&crate::tr!(format!("{peer}: connesso"), format!("{peer}: connected")));

    loop {
        let frame = match read_frame(&mut rd).await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                log(&format!("{peer}: {e}"));
                break;
            }
        };
        let req: Value = match serde_json::from_slice(&frame) {
            Ok(v) => v,
            Err(_) => {
                let _ = send(&wr, &error(ERR_INVALID_REQUEST)).await;
                break;
            }
        };
        let id = req["identifier"].as_str().unwrap_or("").to_string();
        log(&format!("{peer}: {id}"));

        let reply = match id.as_str() {
            // Box::pin: questi futuri sono grandi (soprattutto in debug) e altrimenti finirebbero
            // tutti inline nello stato di `handle_connection`, sullo stack del thread.
            "AnisetteDataRequest" => Box::pin(anisette()).await,
            "InstallProvisioningProfilesRequest" => Box::pin(install_profiles(&ctx, &req)).await,
            "RemoveProvisioningProfilesRequest" => Box::pin(remove_profiles(&ctx, &req)).await,
            "RemoveAppRequest" => Box::pin(remove_app(&ctx, &req)).await,
            "EnableUnsignedCodeExecutionRequest" => {
                log(crate::tr!("JIT (EnableUnsignedCodeExecution) non supportato", "JIT (EnableUnsignedCodeExecution) is not supported"));
                error(ERR_UNSUPPORTED_IOS)
            }
            "PrepareAppRequest" => match Box::pin(receive_and_install(&mut rd, &wr, &ctx, &req)).await {
                Ok(()) => progress(1.0),
                Err(code) => error(code),
            },
            _ => error(ERR_UNKNOWN_REQUEST),
        };
        let what = match reply["errorCode"].as_i64() {
            Some(code) => crate::tr!(format!("errore {code}"), format!("error {code}")),
            None => reply["identifier"].as_str().unwrap_or("?").to_string(),
        };
        if let Err(e) = send(&wr, &reply).await {
            log(&crate::tr!(format!("{peer}: risposta ({what}) non consegnata: {e}"), format!("{peer}: response ({what}) could not be delivered: {e}")));
            break;
        }
        log(&format!("{peer}: -> {what}"));
    }
    log(&crate::tr!(format!("{peer}: chiuso"), format!("{peer}: disconnected")));
}

async fn send(wr: &Rc<tokio::sync::Mutex<OwnedWriteHalf>>, v: &Value) -> Result<(), String> {
    let mut g = wr.lock().await;
    write_frame(&mut *g, v).await
}

async fn phone_link(ctx: &Ctx) -> Result<phone::PhoneLink, i32> {
    match Box::pin(phone::connect_auto(ctx.phone, PAIRING_FILE, |_: &str| {})).await {
        Ok((addr, link)) => {
            log(&crate::tr!(format!("telefono a {addr}"), format!("phone at {addr}")));
            Ok(link)
        }
        Err(e) => {
            log(&crate::tr!(format!("telefono non raggiungibile: {e}"), format!("phone is unreachable: {e}")));
            Err(ERR_DEVICE_NOT_FOUND)
        }
    }
}

/// Gli ultimi dati anisette: AltStore li chiede a ogni operazione, e generarli richiede
/// un paio di secondi e due richieste di rete. Richieste in parallelo condividono il risultato.
static ANISETTE_CACHE: tokio::sync::Mutex<Option<(std::time::Instant, Value)>> =
    tokio::sync::Mutex::const_new(None);
const ANISETTE_TTL: Duration = Duration::from_secs(20);

async fn anisette() -> Value {
    let mut cache = ANISETTE_CACHE.lock().await;
    if let Some((at, v)) = cache.as_ref() {
        if at.elapsed() < ANISETTE_TTL {
            return json!({ "version": 1, "identifier": "AnisetteDataResponse", "anisetteData": v });
        }
    }
    let started = std::time::Instant::now();
    match apple::anisette_for_altstore(Path::new(".")).await {
        Ok(v) => {
            log(&crate::tr!(format!("dati anisette pronti in {:.1} s", started.elapsed().as_secs_f32()), format!("anisette data ready in {:.1} s", started.elapsed().as_secs_f32())));
            *cache = Some((std::time::Instant::now(), v.clone()));
            json!({ "version": 1, "identifier": "AnisetteDataResponse", "anisetteData": v })
        }
        Err(e) => {
            log(&crate::tr!(format!("dati anisette non disponibili: {e}"), format!("anisette data unavailable: {e}")));
            error(ERR_INVALID_ANISETTE)
        }
    }
}

/// I profili arrivano come stringhe base64 (`[Data]` di Swift).
fn decode_profiles(req: &Value) -> Option<Vec<Vec<u8>>> {
    req["provisioningProfiles"]
        .as_array()?
        .iter()
        .map(|v| v.as_str().and_then(crate::web::b64_decode))
        .collect()
}

async fn install_profiles(ctx: &Ctx, req: &Value) -> Value {
    let Some(blobs) = decode_profiles(req) else {
        return error(ERR_INVALID_REQUEST);
    };
    if !blobs.is_empty() {
        let _work = phone_lock();
        let mut link = match phone_link(ctx).await {
            Ok(l) => l,
            Err(code) => return error(code),
        };
        for blob in blobs {
            let name = profile::parse(&blob)
                .map(|p| p.bundle_id.unwrap_or(p.name))
                .unwrap_or_else(|| "?".into());
            if let Err(e) = link.misagent.install(blob).await {
                log(&crate::tr!(format!("profilo {name} non installato: {e:?}"), format!("profile {name} failed to install: {e:?}")));
                return error(ERR_INSTALLATION_FAILED);
            }
            log(&crate::tr!(format!("profilo installato: {name}"), format!("profile installed: {name}")));
        }
    }
    if let Some(active) = req["activeProfiles"].as_array() {
        log(&crate::tr!(
            format!("activeProfiles ({} bundle) ricevuto: non tolgo profili dal telefono", active.len()),
            format!("activeProfiles ({} bundles) received: leaving existing phone profiles untouched", active.len())
        ));
    }
    simple("InstallProvisioningProfilesResponse")
}

async fn remove_profiles(ctx: &Ctx, req: &Value) -> Value {
    let wanted: Vec<String> = req["bundleIdentifiers"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_lowercase)).collect())
        .unwrap_or_default();
    if !wanted.is_empty() {
        let _work = phone_lock();
        let mut link = match phone_link(ctx).await {
            Ok(l) => l,
            Err(code) => return error(code),
        };
        let raw = match link.misagent.copy_all().await {
            Ok(r) => r,
            Err(e) => {
                log(&crate::tr!(format!("non riesco a leggere i profili: {e:?}"), format!("cannot read profiles: {e:?}")));
                return error(ERR_INSTALLATION_FAILED);
            }
        };
        for p in raw.iter().filter_map(|r| profile::parse(r)) {
            let matches = p
                .bundle_id
                .as_ref()
                .is_some_and(|b| wanted.contains(&b.to_lowercase()));
            if matches {
                match link.misagent.remove(&p.uuid).await {
                    Ok(()) => log(&crate::tr!(format!("profilo tolto: {}", p.bundle_id.unwrap_or_default()), format!("profile removed: {}", p.bundle_id.unwrap_or_default()))),
                    Err(e) => log(&crate::tr!(format!("profilo {} non tolto: {e:?}", p.uuid), format!("profile {} not removed: {e:?}", p.uuid))),
                }
            }
        }
    }
    simple("RemoveProvisioningProfilesResponse")
}

async fn remove_app(ctx: &Ctx, req: &Value) -> Value {
    let Some(bundle) = req["bundleIdentifier"].as_str().filter(|b| !b.is_empty()) else {
        return error(ERR_INVALID_REQUEST);
    };
    let _work = phone_lock();
    let mut link = match phone_link(ctx).await {
        Ok(l) => l,
        Err(code) => return error(code),
    };
    let result = Box::pin(async {
        let mut inst = InstallationProxyClient::connect_rsd(&mut link.adapter, &mut link.hs)
            .await
            .map_err(|e| format!("{e:?}"))?;
        inst.uninstall(bundle, None).await.map_err(|e| format!("{e:?}"))
    })
    .await;
    match result {
        Ok(()) => {
            log(&crate::tr!(format!("app disinstallata: {bundle}"), format!("app uninstalled: {bundle}")));
            simple("RemoveAppResponse")
        }
        Err(e) => {
            log(&crate::tr!(format!("app {bundle} non disinstallata: {e}"), format!("app {bundle} could not be uninstalled: {e}")));
            error(ERR_APP_DELETION_FAILED)
        }
    }
}

/// `PrepareAppRequest`: dopo il JSON arrivano `contentSize` byte dell'IPA, poi un
/// `BeginInstallationRequest`; l'installazione manda risposte di avanzamento.
async fn receive_and_install(
    rd: &mut tokio::net::tcp::OwnedReadHalf,
    wr: &Rc<tokio::sync::Mutex<OwnedWriteHalf>>,
    ctx: &Ctx,
    req: &Value,
) -> Result<(), i32> {
    let size = req["contentSize"].as_u64().unwrap_or(0);
    if size == 0 || size > MAX_IPA {
        log(&crate::tr!(format!("dimensione dell'app non valida: {size}"), format!("invalid app size: {size}")));
        return Err(ERR_INVALID_REQUEST);
    }
    log(&crate::tr!(format!("attendo l'app da AltStore: {size} byte"), format!("waiting for app from AltStore: {size} bytes")));
    let mut ipa = vec![0u8; size as usize];
    let mut got = 0usize;
    let mut last_log = std::time::Instant::now();
    let deadline = tokio::time::Instant::now() + IPA_TIMEOUT;
    while got < ipa.len() {
        match tokio::time::timeout_at(deadline, rd.read(&mut ipa[got..])).await {
            Ok(Ok(0)) => {
                log(&crate::tr!(format!("AltStore ha chiuso la connessione dopo {got} di {size} byte"), format!("AltStore disconnected after {got} of {size} bytes")));
                return Err(ERR_INVALID_REQUEST);
            }
            Ok(Ok(n)) => {
                got += n;
                if last_log.elapsed() >= Duration::from_secs(5) {
                    log(&crate::tr!(format!("app in arrivo: {got} di {size} byte"), format!("receiving app: {got} of {size} bytes")));
                    last_log = std::time::Instant::now();
                }
            }
            Ok(Err(e)) => {
                log(&crate::tr!(format!("errore leggendo l'app dopo {got} di {size} byte: {e}"), format!("error receiving app after {got} of {size} bytes: {e}")));
                return Err(ERR_INVALID_REQUEST);
            }
            Err(_) => {
                log(&crate::tr!(format!("app non ricevuta per intero: {got} di {size} byte"), format!("app not fully received: {got} of {size} bytes")));
                return Err(ERR_INVALID_REQUEST);
            }
        }
    }
    log(&crate::tr!(format!("app ricevuta: {} MB", size / 1_048_576), format!("app received: {} MB", size / 1_048_576)));

    // Il messaggio successivo deve essere BeginInstallationRequest.
    let next = match read_frame(rd).await {
        Ok(Some(f)) => serde_json::from_slice::<Value>(&f).unwrap_or(Value::Null),
        _ => return Err(ERR_INVALID_REQUEST),
    };
    if next["identifier"] != "BeginInstallationRequest" {
        return Err(ERR_UNKNOWN_REQUEST);
    }
    if let Some(active) = next["activeProfiles"].as_array() {
        log(&crate::tr!(
            format!("activeProfiles ({} bundle) ricevuto: non tolgo profili dal telefono", active.len()),
            format!("activeProfiles ({} bundles) received: leaving existing phone profiles untouched", active.len())
        ));
    }
    // Un IPA è uno zip.
    if !ipa.starts_with(b"PK") {
        log(crate::tr!("il file ricevuto non è un IPA", "received file is not an IPA"));
        return Err(ERR_INVALID_APP);
    }

    let _work = phone_lock();
    let mut link = phone_link(ctx).await?;
    let for_cb = wr.clone();
    let callback = move |(pct, ()): (u64, ())| {
        let wr = for_cb.clone();
        async move {
            // 100 lo manda solo la fine, quando l'installazione è davvero finita.
            let _ = send(&wr, &progress(pct.min(99) as f64 / 100.0)).await;
        }
    };
    Box::pin(idevice::utils::installation::install_bytes_with_callback_rsd(
        &mut link.adapter,
        &mut link.hs,
        ipa,
        None,
        callback,
        (),
    ))
    .await
    .map_err(|e| {
        log(&crate::tr!(format!("installazione non riuscita: {e:?}"), format!("installation failed: {e:?}")));
        ERR_INSTALLATION_FAILED
    })?;
    log(crate::tr!("app installata", "app installed"));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn messaggi_con_la_lunghezza_davanti_in_little_endian() {
        let (mut a, mut b) = duplex(1024);
        write_frame(&mut a, &json!({"identifier": "X", "version": 1})).await.unwrap();
        // 4 byte di lunghezza + JSON
        let mut raw = [0u8; 4];
        b.read_exact(&mut raw).await.unwrap();
        let n = i32::from_le_bytes(raw) as usize;
        let mut body = vec![0u8; n];
        b.read_exact(&mut body).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["identifier"], "X");

        // e il giro inverso con read_frame
        write_frame(&mut b, &json!({"a": 1})).await.unwrap();
        let got = read_frame(&mut a).await.unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&got).unwrap()["a"], 1);
    }

    #[tokio::test]
    async fn lunghezze_assurde_e_chiusura() {
        let (mut a, mut b) = duplex(64);
        a.write_all(&(-5i32).to_le_bytes()).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
        let (mut a, mut b) = duplex(64);
        a.write_all(&((MAX_JSON as i32) + 1).to_le_bytes()).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
        let (a, mut b) = duplex(64);
        drop(a);
        assert!(read_frame(&mut b).await.unwrap().is_none()); // chiusura pulita
    }

    #[test]
    fn forme_delle_risposte_come_le_decodifica_altstore() {
        assert_eq!(error(11), json!({"version": 3, "identifier": "ErrorResponse", "errorCode": 11}));
        assert_eq!(
            simple("RemoveAppResponse"),
            json!({"version": 1, "identifier": "RemoveAppResponse"})
        );
        assert_eq!(progress(0.5)["progress"], 0.5);
        assert_eq!(progress(0.5)["identifier"], "InstallationProgressResponse");
    }

    #[test]
    fn profili_base64_dalla_richiesta() {
        // "abc" e "hello" in base64
        let req = json!({"provisioningProfiles": ["YWJj", "aGVsbG8="]});
        assert_eq!(decode_profiles(&req).unwrap(), vec![b"abc".to_vec(), b"hello".to_vec()]);
        assert_eq!(decode_profiles(&json!({"provisioningProfiles": []})).unwrap().len(), 0);
        assert!(decode_profiles(&json!({"provisioningProfiles": ["a$b"]})).is_none());
        assert!(decode_profiles(&json!({})).is_none());
    }

    /// Fa girare `handle_connection` contro un client finto su TCP locale e restituisce le
    /// risposte alle richieste date (frame già pronti).
    async fn dialogo(frames: Vec<Vec<u8>>, risposte: usize) -> Vec<Value> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = async {
            let (s, _) = listener.accept().await.unwrap();
            handle_connection(s, Ctx { phone: None }, "test").await;
        };
        let client = async {
            let mut s = TcpStream::connect(addr).await.unwrap();
            for f in frames {
                s.write_all(&f).await.unwrap();
            }
            let mut out = Vec::new();
            for _ in 0..risposte {
                let f = read_frame(&mut s).await.unwrap().unwrap();
                out.push(serde_json::from_slice::<Value>(&f).unwrap());
            }
            drop(s); // chiude: il server esce dal ciclo
            out
        };
        let ((), out) = tokio::join!(server, client);
        out
    }

    fn frame(v: Value) -> Vec<u8> {
        let body = serde_json::to_vec(&v).unwrap();
        let mut out = (body.len() as i32).to_le_bytes().to_vec();
        out.extend(body);
        out
    }

    #[tokio::test]
    async fn richieste_senza_telefono_ne_apple() {
        let out = dialogo(
            vec![
                frame(json!({"version": 1, "identifier": "QualcosaDiNuovo"})),
                frame(json!({"version": 1, "identifier": "EnableUnsignedCodeExecutionRequest", "udid": "x"})),
                frame(json!({"version": 1, "identifier": "InstallProvisioningProfilesRequest",
                             "udid": "x", "provisioningProfiles": []})),
                frame(json!({"version": 1, "identifier": "RemoveProvisioningProfilesRequest",
                             "udid": "x", "bundleIdentifiers": []})),
                frame(json!({"version": 1, "identifier": "InstallProvisioningProfilesRequest",
                             "udid": "x", "provisioningProfiles": ["a$b"]})),
            ],
            5,
        )
        .await;
        assert_eq!(out[0]["identifier"], "ErrorResponse");
        assert_eq!(out[0]["errorCode"], ERR_UNKNOWN_REQUEST);
        assert_eq!(out[1]["errorCode"], ERR_UNSUPPORTED_IOS);
        assert_eq!(out[2]["identifier"], "InstallProvisioningProfilesResponse");
        assert_eq!(out[3]["identifier"], "RemoveProvisioningProfilesResponse");
        assert_eq!(out[4]["errorCode"], ERR_INVALID_REQUEST);
    }

    #[tokio::test]
    async fn installazione_rifiuta_dimensioni_e_file_non_validi() {
        // dimensione zero
        let out = dialogo(
            vec![frame(json!({"version": 1, "identifier": "PrepareAppRequest", "udid": "x", "contentSize": 0}))],
            1,
        )
        .await;
        assert_eq!(out[0]["errorCode"], ERR_INVALID_REQUEST);

        // 4 byte che non sono uno zip, poi BeginInstallationRequest
        let mut frames = vec![frame(
            json!({"version": 1, "identifier": "PrepareAppRequest", "udid": "x", "contentSize": 4}),
        )];
        frames.push(b"abcd".to_vec());
        frames.push(frame(json!({"version": 3, "identifier": "BeginInstallationRequest", "bundleIdentifier": "a.b"})));
        let out = dialogo(frames, 1).await;
        assert_eq!(out[0]["errorCode"], ERR_INVALID_APP);

        // dopo l'IPA arriva la cosa sbagliata
        let mut frames = vec![frame(
            json!({"version": 1, "identifier": "PrepareAppRequest", "udid": "x", "contentSize": 2}),
        )];
        frames.push(b"PK".to_vec());
        frames.push(frame(json!({"version": 1, "identifier": "AnisetteDataRequest"})));
        let out = dialogo(frames, 1).await;
        assert_eq!(out[0]["errorCode"], ERR_UNKNOWN_REQUEST);
    }

    #[tokio::test]
    async fn una_connessione_aperta_non_blocca_le_altre() {
        // AltStore lascia aperta una connessione mentre ne apre un'altra.
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                // Il server serve ogni connessione per conto suo, come nel programma vero.
                let server = tokio::task::spawn_local(async move {
                    loop {
                        let (s, _) = listener.accept().await.unwrap();
                        tokio::task::spawn_local(async move {
                            handle_connection(s, Ctx { phone: None }, "t").await;
                        });
                    }
                });

                let mut a = TcpStream::connect(addr).await.unwrap();
                a.write_all(&frame(json!({"version": 1, "identifier": "Boh"}))).await.unwrap();
                let r = read_frame(&mut a).await.unwrap().unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&r).unwrap()["errorCode"],
                    ERR_UNKNOWN_REQUEST
                );
                // `a` resta aperta: `b` deve avere risposta lo stesso.
                let mut b = TcpStream::connect(addr).await.unwrap();
                b.write_all(&frame(json!({"version": 1, "identifier": "Boh"}))).await.unwrap();
                let r = tokio::time::timeout(Duration::from_secs(2), read_frame(&mut b))
                    .await
                    .expect("la seconda connessione non deve aspettare la prima")
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&r).unwrap()["errorCode"],
                    ERR_UNKNOWN_REQUEST
                );
                drop(a);
                drop(b);
                server.abort();
            })
            .await;
    }

    #[tokio::test]
    async fn json_rotto_chiude_con_errore() {
        let mut bad = (4i32).to_le_bytes().to_vec();
        bad.extend(b"nope");
        let out = dialogo(vec![bad], 1).await;
        assert_eq!(out[0]["errorCode"], ERR_INVALID_REQUEST);
    }
}
