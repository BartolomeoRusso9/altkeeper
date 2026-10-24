//! Interfaccia web per chi vive in casa: stato delle app, "Rinnova ora" e accesso all'Apple ID
//! (con il codice 2FA inserito dal browser). Comando `altkeeper serve`.
//!
//! Sicurezza: di default ascolta solo su questo computer. Per aprirla alla rete di casa serve un
//! PIN (HTTP Basic: nome utente qualsiasi, password = PIN). Le azioni (POST) richiedono
//! un'intestazione che una pagina di un altro sito non può mandare. Niente HTTPS: pensata per la
//! rete di casa; per uscire dalla rete metti davanti un proxy con TLS.
//!
//! Il lavoro con telefono e Apple gira in un thread dedicato (i loro oggetti non sono `Send`),
//! una sola operazione alla volta.

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use idevice::{RsdService, services::lockdown::LockdownClient};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{PAIRING_FILE, apple, phone, profile};

const INDEX_HTML: &str = include_str!("web/index.html");
const RENEW_LOG: &str = "renew.log";
const MAX_JOB_LINES: usize = 300;
const MAX_IPA_BYTES: usize = 1024 * 1024 * 1024;

// ---------------------------------------------------------------------------------------------
// Stato condiviso
// ---------------------------------------------------------------------------------------------

struct App {
    pin: Option<String>,
    phone: Option<SocketAddr>,
    /// C'è già un lavoro in corso (controllo, rinnovo o login).
    busy: AtomicBool,
    inner: Mutex<Inner>,
    tfa: Arc<apple::TfaChannel>,
}

#[derive(Default)]
struct Inner {
    snapshot: Snapshot,
    job: JobView,
    login: LoginView,
}

#[derive(Default, Clone)]
struct Snapshot {
    checked_at: Option<u64>,
    checking: bool,
    reachable: Option<bool>,
    address: Option<String>,
    error: Option<String>,
    detail: Option<String>,
    profiles_total: usize,
    apps: Vec<AppView>,
}

#[derive(Clone, PartialEq, Debug)]
struct AppView {
    bundle: String,
    label: String,
    days: i64,
    expires: String,
    level: &'static str,
}

#[derive(Default, Clone)]
struct JobView {
    kind: String,
    running: bool,
    lines: Vec<String>,
    ok: Option<bool>,
}

#[derive(Default, Clone)]
struct LoginView {
    /// "", "working", "done" o "failed".
    phase: &'static str,
    message: String,
}

impl App {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push_line(&self, s: &str) {
        let mut g = self.lock();
        g.job.lines.push(s.to_string());
        if g.job.lines.len() > MAX_JOB_LINES {
            g.job.lines.remove(0);
        }
    }
}

/// Segna il lavoro come finito quando viene lasciato (anche se il thread va in panico).
struct BusyGuard(Arc<App>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

fn try_busy(app: &Arc<App>) -> Option<BusyGuard> {
    app.busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| BusyGuard(app.clone()))
}

/// Lancia `make(app)` in un thread con un runtime tutto suo.
fn spawn_job<Fut>(guard: BusyGuard, make: impl FnOnce(Arc<App>) -> Fut + Send + 'static)
where
    Fut: std::future::Future<Output = ()>,
{
    let app = guard.0.clone();
    // Più stack del predefinito (2 MB): i futuri del telefono e di Apple sono grandi, soprattutto in debug.
    let _ = std::thread::Builder::new().stack_size(16 * 1024 * 1024).spawn(move || {
        let _guard = guard;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(make(app.clone()));
        }));
        if outcome.is_err() {
            let mut g = app.lock();
            g.snapshot.checking = false;
            g.job.running = false;
            g.job.ok = Some(false);
            g.job
                .lines
                .push("Internal error: the operation was interrupted.".into());
            if g.login.phase == "working" {
                g.login = LoginView {
                    phase: "failed",
                    message: "Internal error.".into(),
                };
            }
        }
    });
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------
// Avvio
// ---------------------------------------------------------------------------------------------

/// Controlla indirizzo e PIN. Si chiama prima di avviare qualsiasi altra cosa (AltServer
/// compreso), così una configurazione sbagliata non lascia annunci Bonjour orfani.
pub fn check_options(bind: SocketAddr, pin: &Option<String>) -> Result<(), String> {
    match pin {
        None if !bind.ip().is_loopback() => Err(crate::tr!(
            "per ascoltare fuori da questo computer serve un PIN: imposta ALTKEEPER_WEB_PIN (o usa --pin)",
            "listening outside this computer requires a PIN: set ALTKEEPER_WEB_PIN (or use --pin)"
        )
            .into()),
        Some(p) if p.chars().count() < 4 => Err(crate::tr!("il PIN deve avere almeno 4 caratteri", "the PIN must contain at least 4 characters").into()),
        _ => Ok(()),
    }
}

pub async fn serve(
    bind: SocketAddr,
    pin: Option<String>,
    phone: Option<SocketAddr>,
) -> Result<(), String> {
    check_options(bind, &pin)?;

    let app = Arc::new(App {
        pin,
        phone,
        busy: AtomicBool::new(false),
        inner: Mutex::new(Inner::default()),
        tfa: Arc::new(apple::TfaChannel::default()),
    });
    let router = Router::new()
        .route("/", get(index))
        .route("/api/state", get(state))
        .route("/api/check", post(check))
        .route("/api/renew", post(renew))
        .route(
            "/api/install",
            post(install).layer(DefaultBodyLimit::max(MAX_IPA_BYTES + 1024 * 1024)),
        )
        .route("/api/login", post(login))
        .route("/api/login/code", post(login_code))
        .route("/api/login/resend", post(login_resend))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app.clone());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| crate::tr!(format!("non riesco ad ascoltare su {bind}: {e}"), format!("cannot listen on {bind}: {e}")))?;
    println!("{}", crate::tr!(
        format!("Interfaccia web su http://{bind} ({})", if app.pin.is_some() {
            "protetta da PIN: il nome utente può essere qualsiasi"
        } else { "raggiungibile solo da questo computer" }),
        format!("Web interface at http://{bind} ({})", if app.pin.is_some() {
            "PIN-protected: any username is accepted"
        } else { "only accessible from this computer" })
    ));
    axum::serve(listener, router).await.map_err(|e| format!("{e:?}"))
}

// ---------------------------------------------------------------------------------------------
// Protezione
// ---------------------------------------------------------------------------------------------

async fn guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    if app.pin.is_none() && !host_is_local(req.headers()) {
        return (StatusCode::FORBIDDEN, "host non ammesso").into_response();
    }
    if let Some(pin) = &app.pin {
        if !basic_password_ok(req.headers(), pin) {
            // Rallenta chi prova PIN a caso.
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut res = (StatusCode::UNAUTHORIZED, "serve il PIN").into_response();
            res.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"altkeeper\""),
            );
            return res;
        }
    }
    if req.method() == Method::POST
        && req.headers().get("x-requested-with").map(|v| v.as_bytes()) != Some(b"altkeeper")
    {
        return (StatusCode::FORBIDDEN, "richiesta non valida").into_response();
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; connect-src 'self'; style-src 'unsafe-inline'; \
             script-src 'unsafe-inline'; base-uri 'none'; form-action 'none'",
        ),
    );
    res
}

/// Senza PIN si ascolta solo in locale: accetta solo `localhost` e gli indirizzi di loopback
/// (evita che un sito esterno, con un nome che punta qui, usi il servizio).
fn host_is_local(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

fn basic_password_ok(headers: &HeaderMap, pin: &str) -> bool {
    let Some(value) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(encoded) = value.strip_prefix("Basic ") else {
        return false;
    };
    let Some(raw) = b64_decode(encoded.trim()) else {
        return false;
    };
    let Ok(text) = String::from_utf8(raw) else {
        return false;
    };
    let password = text.split_once(':').map(|(_, p)| p).unwrap_or(&text);
    ct_eq(password.as_bytes(), pin.as_bytes())
}

/// Confronto a tempo costante (sulla lunghezza massima), per non svelare il PIN dai tempi.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

pub(crate) fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Pagine e API
// ---------------------------------------------------------------------------------------------

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

fn json_err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

fn json_ok() -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({ "ok": true })))
}

const BUSY_MSG: &str = "Another operation is already running: please wait for it to finish.";

async fn state(State(app): State<Arc<App>>) -> Json<Value> {
    let account = apple::read_account(Path::new("."));
    let pairing = Path::new(PAIRING_FILE).exists();

    // Se i dati sul telefono sono vecchi, si riguarda in background senza far aspettare la pagina.
    let stale = {
        let g = app.lock();
        pairing && !g.snapshot.checking && g.snapshot.checked_at.is_none_or(|t| now() - t > 60)
    };
    if stale {
        if let Some(guard) = try_busy(&app) {
            app.lock().snapshot.checking = true;
            spawn_job(guard, run_check);
        }
    }

    let history = std::fs::read_to_string(RENEW_LOG)
        .map(|t| parse_history(&t, 6))
        .unwrap_or_default();
    let g = app.lock();
    let asking = app.tfa.asking.load(Ordering::SeqCst);
    let login_phase = if asking { "needs_code" } else { g.login.phase };
    let code_error = app.tfa.last_error.lock().ok().and_then(|e| e.clone());

    Json(json!({
        "now": now(),
        "busy": app.busy.load(Ordering::SeqCst),
        "pairing": pairing,
        "account": {
            "present": account.is_some(),
            "apple_id": account.as_ref().map(|a| a.apple_id.clone()),
            "password_saved": account.as_ref().is_some_and(|a| !a.password.is_empty()),
            "team": account.as_ref().is_some_and(|a| a.team_id.is_some()),
        },
        "phone": {
            "checked_at": g.snapshot.checked_at,
            "checking": g.snapshot.checking,
            "reachable": g.snapshot.reachable,
            "address": g.snapshot.address,
            "error": g.snapshot.error,
            "detail": g.snapshot.detail,
            "profiles_total": g.snapshot.profiles_total,
        },
        "apps": g.snapshot.apps.iter().map(|a| json!({
            "bundle": a.bundle, "label": a.label, "days": a.days,
            "expires": a.expires, "level": a.level,
        })).collect::<Vec<_>>(),
        "job": {
            "kind": g.job.kind, "running": g.job.running,
            "lines": g.job.lines, "ok": g.job.ok,
        },
        "login": {
            "phase": login_phase,
            "message": g.login.message,
            "code_error": if asking { code_error } else { None },
        },
        "history": history,
    }))
}

async fn check(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    app.lock().snapshot.checking = true;
    spawn_job(guard, run_check);
    json_ok()
}

#[derive(Deserialize)]
struct RenewReq {
    /// "due" rinnova solo le app in scadenza, "now" le rinnova tutte.
    mode: String,
}

async fn renew(
    State(app): State<Arc<App>>,
    Json(req): Json<RenewReq>,
) -> (StatusCode, Json<Value>) {
    let force = match req.mode.as_str() {
        "now" => true,
        "due" => false,
        _ => return json_err(StatusCode::BAD_REQUEST, "Invalid mode"),
    };
    match apple::read_account(Path::new(".")) {
        Some(a) if !a.password.is_empty() && a.team_id.is_some() => {}
        _ => {
            return json_err(
                StatusCode::BAD_REQUEST,
                "Sign in with your Apple ID first and choose to save the password.",
            );
        }
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    {
        let mut g = app.lock();
        g.job = JobView {
            kind: if force { "renew_all".into() } else { "renew_due".into() },
            running: true,
            lines: Vec::new(),
            ok: None,
        };
    }
    spawn_job(guard, move |app| run_renew(app, force));
    json_ok()
}

struct UploadedIpa(PathBuf);

impl Drop for UploadedIpa {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct UploadInProgress {
    path: PathBuf,
    file: Option<std::fs::File>,
    size: usize,
    keep: bool,
}

impl UploadInProgress {
    fn finish(mut self) -> UploadedIpa {
        drop(self.file.take());
        self.keep = true;
        UploadedIpa(self.path.clone())
    }
}

impl Drop for UploadInProgress {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn new_upload_file() -> Result<(PathBuf, std::fs::File), String> {
    let dir = Path::new("state").join("uploads");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create upload directory: {e}"))?;
    crate::fsperm::restrict_dir(&dir).map_err(|e| format!("Cannot protect upload directory: {e}"))?;

    let mut random = [0u8; 16];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut random)
        .map_err(|_| "Cannot generate a temporary upload name.".to_string())?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("install-{name}.ipa"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|e| format!("Cannot create temporary upload: {e}"))?;
    if let Err(e) = crate::fsperm::restrict_file(&path) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("Cannot protect upload: {e}"));
    }
    Ok((path, file))
}

async fn receive_ipa(multipart: &mut Multipart) -> Result<UploadedIpa, (StatusCode, String)> {
    use std::io::Write as _;

    let mut upload: Option<UploadInProgress> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid upload: {e}")))?
    {
        if field.name() != Some("ipa") || upload.is_some() {
            return Err((StatusCode::BAD_REQUEST, "Send one IPA file in the 'ipa' field.".into()));
        }
        let filename = field.file_name().unwrap_or("");
        if !filename.to_ascii_lowercase().ends_with(".ipa") {
            return Err((StatusCode::BAD_REQUEST, "Choose a file with the .ipa extension.".into()));
        }
        let (path, file) = new_upload_file()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        upload = Some(UploadInProgress {
            path,
            file: Some(file),
            size: 0,
            keep: false,
        });
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Upload failed: {e}")))?
        {
            let Some(upload) = upload.as_mut() else {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Upload state was lost.".into()));
            };
            let next_size = upload.size.saturating_add(chunk.len());
            if next_size > MAX_IPA_BYTES {
                return Err((StatusCode::PAYLOAD_TOO_LARGE, "IPA exceeds the 1 GiB limit.".into()));
            }
            upload.file.as_mut().ok_or_else(|| {
                (StatusCode::INTERNAL_SERVER_ERROR, "Upload file was closed unexpectedly.".into())
            })?.write_all(&chunk)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot save upload: {e}")))?;
            upload.size = next_size;
        }
    }

    let upload = upload
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Choose an IPA file to install.".into()))?;
    if upload.size == 0 {
        return Err((StatusCode::BAD_REQUEST, "The IPA file is empty.".into()));
    }
    Ok(upload.finish())
}

async fn install(
    State(app): State<Arc<App>>,
    mut multipart: Multipart,
) -> (StatusCode, Json<Value>) {
    match apple::read_account(Path::new(".")) {
        Some(a) if !a.password.is_empty() && a.team_id.is_some() => {}
        _ => {
            return json_err(
                StatusCode::BAD_REQUEST,
                "Sign in and save the Apple ID password before installing from a PC.",
            );
        }
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    let uploaded = match receive_ipa(&mut multipart).await {
        Ok(file) => file,
        Err((status, message)) => return json_err(status, &message),
    };
    {
        let mut g = app.lock();
        g.job = JobView {
            kind: "install".into(),
            running: true,
            lines: vec!["IPA uploaded. Signing and installing on the iPhone…".into()],
            ok: None,
        };
    }
    spawn_job(guard, move |app| run_install(app, uploaded));
    json_ok()
}

#[derive(Deserialize)]
struct LoginReq {
    apple_id: String,
    password: String,
    #[serde(default = "yes")]
    save_password: bool,
}

fn yes() -> bool {
    true
}

async fn login(
    State(app): State<Arc<App>>,
    Json(req): Json<LoginReq>,
) -> (StatusCode, Json<Value>) {
    let id = req.apple_id.trim().to_string();
    if id.is_empty() || id.len() > 200 || req.password.is_empty() || req.password.len() > 200 {
        return json_err(StatusCode::BAD_REQUEST, "Enter your Apple ID and password.");
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    app.lock().login = LoginView { phase: "working", message: String::new() };
    *app.tfa.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let save = req.save_password;
    let password = req.password;
    spawn_job(guard, move |app| run_login(app, id, password, save));
    json_ok()
}

#[derive(Deserialize)]
struct CodeReq {
    code: String,
}

async fn login_code(
    State(app): State<Arc<App>>,
    Json(req): Json<CodeReq>,
) -> (StatusCode, Json<Value>) {
    let code = req.code.trim();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return json_err(StatusCode::BAD_REQUEST, "The code must contain 6 digits.");
    }
    if app.tfa.answer(apple::TfaAnswer::Code(code.to_string())) {
        json_ok()
    } else {
        json_err(StatusCode::CONFLICT, "Apple is not waiting for a code.")
    }
}

async fn login_resend(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    if app.tfa.answer(apple::TfaAnswer::Resend) {
        json_ok()
    } else {
        json_err(StatusCode::CONFLICT, "Apple is not waiting for a code.")
    }
}

// ---------------------------------------------------------------------------------------------
// I tre lavori
// ---------------------------------------------------------------------------------------------

/// Guarda cosa c'è sul telefono (senza toccare Apple).
async fn run_check(app: Arc<App>) {
    app.lock().snapshot.checking = true;
    let team = apple::read_account(Path::new(".")).and_then(|a| a.team_id);

    let result = tokio::time::timeout(Duration::from_secs(45), async {
        let (addr, mut link) = phone::connect_auto(app.phone, PAIRING_FILE, |_: &str| {}).await?;
        let profiles = crate::phone_profiles(&mut link).await?;
        Ok::<_, String>((addr, profiles))
    })
    .await;

    let mut g = app.lock();
    g.snapshot.checking = false;
    g.snapshot.checked_at = Some(now());
    match result {
        Ok(Ok((addr, profiles))) => {
            g.snapshot.reachable = Some(true);
            g.snapshot.address = Some(addr.to_string());
            g.snapshot.error = None;
            g.snapshot.detail = None;
            g.snapshot.profiles_total = profiles.len();
            g.snapshot.apps = team.as_deref().map(|t| app_views(&profiles, t)).unwrap_or_default();
        }
        Ok(Err(e)) => {
            g.snapshot.reachable = Some(false);
            g.snapshot.error = Some(friendly_error(&e));
            g.snapshot.detail = Some(e.chars().take(400).collect());
        }
        Err(_) => {
            g.snapshot.reachable = Some(false);
            g.snapshot.error = Some(friendly_error("non risponde"));
            g.snapshot.detail = Some("nessuna risposta entro 45 secondi".into());
        }
    }
}

async fn run_renew(app: Arc<App>, force: bool) {
    let mut say = |s: &str| app.push_line(&english_job_line(s));
    let result = crate::cmd_renew(app.phone, false, force, 3, &mut say).await;

    let ok = result.is_ok();
    if let Err(e) = &result {
        app.push_line(&format!("Error: {}", short_error(e)));
    }
    let lines = {
        let mut g = app.lock();
        g.job.running = false;
        g.job.ok = Some(ok);
        g.job.lines.clone()
    };
    append_log(&lines, ok);
    // Poi si aggiorna lo stato mostrato in pagina.
    run_check(app).await;
}

async fn run_install(app: Arc<App>, upload: UploadedIpa) {
    let result = install_ipa(&app, &upload).await;
    let ok = result.is_ok();
    if let Err(e) = result {
        app.push_line(&format!("Error: {}", short_error(&e)));
    } else {
        app.push_line("Installation completed.");
    }
    {
        let mut g = app.lock();
        g.job.running = false;
        g.job.ok = Some(ok);
    }
    run_check(app).await;
}

async fn install_ipa(app: &Arc<App>, upload: &UploadedIpa) -> Result<(), String> {
    let account = apple::load_account(Path::new("."))?;
    let mut session = apple::open_session_web(
        Path::new("."),
        &account.apple_id,
        &account.password,
        app.tfa.clone(),
    )
    .await?;
    let team = apple::pick_team(&mut session, account.team_id.as_deref(), false).await?;

    app.push_line("Connecting to the iPhone to read its device identity…");
    let (device_name, udid) = {
        let _work = crate::altserver::phone_lock();
        let (_, mut link) = phone::connect_auto(app.phone, PAIRING_FILE, |_: &str| {}).await?;
        let mut lockdown = LockdownClient::connect_rsd(&mut link.adapter, &mut link.hs)
            .await
            .map_err(|e| format!("Cannot query iPhone identity through trusted lockdown: {e}"))?;
        let name = lockdown
            .get_value(Some("DeviceName"), None)
            .await
            .map_err(|e| format!("Cannot read the iPhone device name: {e}"))?
            .as_string()
            .ok_or("The iPhone returned an invalid device name.")?
            .to_string();
        let udid = lockdown
            .get_value(Some("UniqueDeviceID"), None)
            .await
            .map_err(|e| format!("Cannot read the iPhone UDID: {e}"))?
            .as_string()
            .ok_or("The iPhone returned an invalid UDID.")?
            .to_string();
        (name, udid)
    };

    app.push_line("Signing the IPA with the configured Apple ID…");
    let signed_app = apple::sign_ipa(
        session,
        &account,
        team,
        &device_name,
        &udid,
        &upload.0,
    )
    .await?;

    app.push_line("Installing the signed app over Wi-Fi…");
    let _work = crate::altserver::phone_lock();
    let (_, mut link) = phone::connect_auto(app.phone, PAIRING_FILE, |_: &str| {}).await?;
    // Il telefono manda decine di notifiche con la stessa percentuale: una riga ogni 10%.
    let last_logged = std::sync::atomic::AtomicI64::new(-1);
    let started = std::time::Instant::now();
    isideload::sideload::install::install_app_rsd(
        &mut link.adapter,
        &mut link.hs,
        &signed_app.path,
        |percent| {
            if let Some(step) = progress_step(percent, last_logged.load(Ordering::Relaxed)) {
                last_logged.store(step, Ordering::Relaxed);
                app.push_line(&format!(
                    "Installation progress: {step}% ({}s)",
                    started.elapsed().as_secs()
                ));
            }
        },
    )
    .await
    .map_err(|e| format!("Installation failed: {e}"))?;
    Ok(())
}

/// La percentuale da scrivere nel log, o `None` se non è ancora cambiata di 10 punti
/// dall'ultima scritta (`last`, -1 se nessuna). Il 100% si scrive sempre, una volta sola.
fn progress_step(percent: u64, last: i64) -> Option<i64> {
    let p = percent.min(100) as i64;
    let step = if p >= 100 { 100 } else { p / 10 * 10 };
    (step > last).then_some(step)
}

async fn run_login(app: Arc<App>, apple_id: String, password: String, save: bool) {
    let dir = Path::new(".");
    let outcome: Result<String, String> = async {
        let mut sess = apple::open_session_web(dir, &apple_id, &password, app.tfa.clone()).await?;
        let team = apple::pick_team(&mut sess, None, false).await?;
        apple::save_account(
            dir,
            &apple::Account {
                apple_id: apple_id.clone(),
                password: if save { password.clone() } else { String::new() },
                team_id: Some(team.team_id.clone()),
            },
        )?;
        Ok(team.name.clone().unwrap_or_else(|| team.team_id.clone()))
    }
    .await;

    let mut g = app.lock();
    g.login = match outcome {
        Ok(team) => LoginView {
            phase: "done",
            message: if save {
                format!("Signed in successfully (team {team}). The password is saved for renewals.")
            } else {
                format!(
                    "Signed in successfully (team {team}), but the password was not saved: \
                     automatic renewals cannot run."
                )
            },
        },
        Err(e) => LoginView { phase: "failed", message: short_apple_error(&e) },
    };
    // I dati del telefono possono cambiare in base al team: il prossimo controllo li rifà.
    g.snapshot.checked_at = None;
}

// ---------------------------------------------------------------------------------------------
// Pezzi puri (testati sotto)
// ---------------------------------------------------------------------------------------------

fn label_of(bundle: &str, team: &str) -> String {
    bundle.strip_suffix(&format!(".{team}")).unwrap_or(bundle).to_string()
}

fn level_for(days: i64) -> &'static str {
    if days <= 1 {
        "urgent"
    } else if days <= 3 {
        "soon"
    } else {
        "ok"
    }
}

/// Le app del team, una riga per bundle (il profilo più recente), dalla più urgente.
fn app_views(profiles: &[profile::PhoneProfile], team: &str) -> Vec<AppView> {
    let mine = profiles.iter().filter(|p| p.team_id.as_deref() == Some(team));
    let mut views: Vec<AppView> = profile::latest_by_bundle(mine)
        .into_iter()
        .filter_map(|p| {
            let bundle = p.bundle_id.clone()?;
            let days = p.days_left().unwrap_or(-1);
            Some(AppView {
                label: label_of(&bundle, team),
                bundle,
                days,
                expires: p.expires_text(),
                level: level_for(days),
            })
        })
        .collect();
    views.sort_by(|a, b| a.days.cmp(&b.days).then_with(|| a.label.cmp(&b.label)));
    views
}

/// Traduce gli errori tecnici in una frase comprensibile.
fn friendly_error(err: &str) -> String {
    let e = err.to_lowercase();
    if e.contains("manca il pairing") {
        "iPhone pairing is missing. Set it up once over USB (see the guide)."
            .into()
    } else if e.contains("early eof") || e.contains("pairing con l'iphone non riuscito") {
        "The iPhone no longer recognizes the pairing: set it up again over USB.".into()
    } else if e.contains("no route to host")
        || e.contains("hostunreachable")
        || e.contains("non risponde")
        || e.contains("non trovo l'iphone")
        || e.contains("timed out")
        || e.contains("refused")
    {
        "I cannot find the iPhone on the network. Is it on and connected to your home Wi-Fi?".into()
    } else {
        short_error(err)
    }
}

fn short_error(err: &str) -> String {
    let one_line = err
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(err);
    let translated = if let Some(detail) = one_line.strip_prefix("login Apple ID non riuscito: ") {
        format!("Apple ID sign-in failed: {detail}")
    } else if let Some(detail) = one_line.strip_prefix("sessione sviluppatore non riuscita: ") {
        format!("Developer session failed: {detail}")
    } else if let Some(detail) = one_line.strip_prefix("registrazione iPhone non riuscita: ") {
        format!("iPhone registration failed: {detail}")
    } else if one_line.starts_with("pairing con l'iPhone non riuscito") {
        "iPhone pairing failed. Set it up again over USB.".to_string()
    } else if one_line.starts_with("manca account.json: ") {
        "account.json is missing: first run `altkeeper login <apple-id>`.".to_string()
    } else if let Some((path, _)) = one_line
        .strip_prefix("manca il pairing remoto ")
        .and_then(|s| s.split_once(": crealo con "))
    {
        format!("Remote pairing file {path} is missing. Create it with `altkeeper pair-usb`.")
    } else {
        if let Some(team) = one_line
            .strip_prefix("il team ")
            .and_then(|s| s.strip_suffix(" non esiste su questo Apple ID"))
        {
            format!("Team {team} does not exist for this Apple ID.")
        } else {
            match one_line {
                "nessun team sviluppatore su questo Apple ID" => {
                    "No developer team is available for this Apple ID.".to_string()
                }
                "password non salvata: rifai `altkeeper login` e scegli di salvarla" => {
                    "Password not saved: run `altkeeper login` again and choose to save it."
                        .to_string()
                }
                "il telefono non risponde via USB" => {
                    "The iPhone is not responding over USB.".to_string()
                }
                "non trovo l'iPhone in rete: è acceso e sul Wi-Fi di casa?" => {
                    "Cannot find the iPhone on the network. Is it on and connected to your home Wi-Fi?"
                        .to_string()
                }
                _ => one_line.to_string(),
            }
        }
    };
    translated.chars().take(400).collect()
}

fn english_job_line(line: &str) -> String {
    if line == "Collegato via USB" {
        return "Connected via USB".into();
    }
    if line == "Cerco il telefono in rete..." {
        return "Searching the network for the iPhone...".into();
    }
    if line == "Bonjour non risponde: cerco sulla rete locale..." {
        return "Bonjour did not respond; scanning the local network...".into();
    }
    if let Some(address) = line.strip_prefix("Telefono trovato a ") {
        return format!("Found iPhone at {address}");
    }
    if let Some(address) = line
        .strip_prefix("Trovato un telefono a ")
        .and_then(|s| s.strip_suffix(", provo..."))
    {
        return format!("Found an iPhone at {address}; trying to connect...");
    }
    if let Some(address) = line
        .split_once(" non risponde, cerco il telefono in rete...")
        .map(|(address, _)| address)
    {
        return format!("{address} did not respond; searching the network...");
    }
    if let Some(detail) = line
        .strip_prefix("USB non riuscita (")
        .and_then(|s| s.strip_suffix("), provo in Wi-Fi..."))
    {
        return format!("USB connection failed ({}); trying Wi-Fi...", short_error(detail));
    }
    if let Some(date) = line.strip_prefix("      nuovo profilo valido fino al ") {
        return format!("      New profile valid until {date}");
    }
    line.replace(" scade ", " expires ")
        .replace(" giorni)", " days)")
}

/// Dall'errore lungo del login prende la frase utile, e traduce i casi più comuni.
fn short_apple_error(err: &str) -> String {
    let e = err.to_lowercase();
    if e.contains("-20101") || e.contains("entered incorrectly") {
        return "Incorrect Apple ID or password.".into();
    }
    if e.contains("429") || e.contains("too many requests") {
        return "Apple is rate-limiting requests. Wait a while, then try once."
            .into();
    }
    if e.contains("aborted") || e.contains("abort") {
        return "Sign-in canceled: the code did not arrive in time.".into();
    }
    let last = err
        .lines()
        .filter(|l| l.contains('●'))
        .last()
        .map(|l| l.trim().trim_start_matches('●').trim().to_string());
    last.unwrap_or_else(|| short_error(err)).chars().take(240).collect()
}

/// Ultimi giri di `renew.log`, dal più recente. Legge sia le righe del cron sia quelle scritte
/// dalla pagina: `=== data`, l'output di `renew` e `esito: N`.
fn parse_history(text: &str, max: usize) -> Vec<Value> {
    let mut blocks: Vec<(String, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        if let Some(when) = line.strip_prefix("=== ") {
            blocks.push((when.trim().to_string(), Vec::new()));
        } else if let Some((_, lines)) = blocks.last_mut() {
            lines.push(line);
        }
    }
    blocks
        .into_iter()
        .rev()
        .take(max)
        .map(|(when, lines)| {
            let code = lines
                .iter()
                .rev()
                .find_map(|l| l.strip_prefix("esito:").or_else(|| l.strip_prefix("result:")))
                .and_then(|c| c.trim().parse::<i32>().ok());
            let installed = lines
                .iter()
                .filter(|l| {
                    l.contains("installato sul telefono") || l.contains("installed on the phone")
                })
                .count();
            let summary = if code.is_some_and(|c| c != 0) {
                let last = lines
                    .iter()
                    .rev()
                    .map(|l| l.trim())
                    .find(|l| {
                        !l.is_empty() && !l.starts_with("esito:") && !l.starts_with("result:")
                    })
                    .unwrap_or("errore");
                friendly_error(last)
            } else if installed > 0 {
                format!("Renewed {installed} apps")
            } else if lines
                .iter()
                .any(|l| {
                    l.contains("Niente da rinnovare")
                        || l.contains("non serve rinnovare")
                        || l.contains("Nothing to renew")
                        || l.contains("no renewal needed")
                })
            {
                "Nothing to renew".to_string()
            } else {
                "Done".to_string()
            };
            json!({ "when": when, "ok": code == Some(0), "summary": summary })
        })
        .collect()
}

/// Ora locale come la scrive `date` (per allinearsi al log del cron). Su Windows `date` non è
/// un programma ma un comando interno di cmd: si chiede a PowerShell, con lo stesso formato.
fn local_time() -> String {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("powershell");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Date -Format 'yyyy-MM-dd HH:mm:ss'",
        ]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = std::process::Command::new("date");
        c.arg("+%F %T");
        c
    };
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| format!("epoch {}", now()))
}

fn append_log(lines: &[String], ok: bool) {
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(RENEW_LOG) else {
        return;
    };
    let _ = writeln!(f, "=== {} (interfaccia web)", local_time());
    for l in lines {
        let _ = writeln!(f, "{l}");
    }
    let _ = writeln!(f, "result: {}", if ok { 0 } else { 1 });
}

// ---------------------------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[test]
    fn il_progresso_si_scrive_ogni_dieci_punti() {
        let mut last = -1;
        let mut out = vec![];
        for p in [0, 0, 5, 9, 10, 10, 68, 68, 69, 70, 71, 74, 99, 100, 100] {
            if let Some(s) = super::progress_step(p, last) {
                last = s;
                out.push(s);
            }
        }
        assert_eq!(out, vec![0, 10, 60, 70, 90, 100]);
    }

    use super::*;
    use std::time::Duration;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn confronto_a_tempo_costante() {
        assert!(ct_eq(b"1234", b"1234"));
        assert!(!ct_eq(b"1234", b"1235"));
        assert!(!ct_eq(b"1234", b"12345"));
        assert!(!ct_eq(b"", b"1"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn base64_come_lo_scrive_il_browser() {
        assert_eq!(b64_decode("YTpiMTIzNA==").unwrap(), b"a:b1234");
        assert_eq!(b64_decode("OjEyMzQ=").unwrap(), b":1234");
        assert_eq!(b64_decode("").unwrap(), b"");
        assert!(b64_decode("a$b").is_none());
    }

    #[test]
    fn il_pin_si_legge_dalla_password_di_basic() {
        // "utente:1234" e ":1234" (nome utente vuoto)
        let ok = headers(&[("authorization", "Basic dXRlbnRlOjEyMzQ=")]);
        assert!(basic_password_ok(&ok, "1234"));
        let vuoto = headers(&[("authorization", "Basic OjEyMzQ=")]);
        assert!(basic_password_ok(&vuoto, "1234"));
        assert!(!basic_password_ok(&ok, "9999"));
        assert!(!basic_password_ok(&headers(&[]), "1234"));
        assert!(!basic_password_ok(&headers(&[("authorization", "Bearer 1234")]), "1234"));
        // una password con i due punti dentro: vale tutto dopo il primo
        let due_punti = headers(&[("authorization", "Basic dTphOmI=")]); // u:a:b
        assert!(basic_password_ok(&due_punti, "a:b"));
    }

    #[test]
    fn senza_pin_solo_host_locali() {
        assert!(host_is_local(&headers(&[("host", "localhost:8787")])));
        assert!(host_is_local(&headers(&[("host", "127.0.0.1:8787")])));
        assert!(host_is_local(&headers(&[("host", "[::1]:8787")])));
        assert!(!host_is_local(&headers(&[("host", "evil.example:8787")])));
        assert!(!host_is_local(&headers(&[("host", "192.168.1.4:8787")])));
        assert!(!host_is_local(&headers(&[])));
    }

    #[test]
    fn etichette_e_livelli() {
        assert_eq!(label_of("com.spotify.client.TEAM", "TEAM"), "com.spotify.client");
        assert_eq!(label_of("com.altro", "TEAM"), "com.altro");
        assert_eq!(level_for(-1), "urgent");
        assert_eq!(level_for(1), "urgent");
        assert_eq!(level_for(3), "soon");
        assert_eq!(level_for(6), "ok");
    }

    fn profile(uuid: &str, team: &str, bundle: &str, days: u64) -> profile::PhoneProfile {
        profile::PhoneProfile {
            uuid: uuid.into(),
            name: "n".into(),
            team_id: Some(team.into()),
            bundle_id: Some(bundle.into()),
            expires: Some(plist::Date::from(
                SystemTime::now() + Duration::from_secs(days * 86_400 + 3_600),
            )),
        }
    }

    #[test]
    fn le_app_sono_quelle_del_team_una_per_bundle_dalla_piu_urgente() {
        let all = vec![
            profile("1", "T", "com.b.T", 6),
            profile("2", "T", "com.a.T", 2),
            profile("3", "T", "com.a.T", 6), // rinnovato: vale questo
            profile("4", "ALTRO", "com.store", 70),
        ];
        let v = app_views(&all, "T");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].label, "com.a");
        assert_eq!(v[0].days, 6);
        assert_eq!(v[1].label, "com.b");
        assert!(v.iter().all(|a| a.level == "ok"));
    }

    #[test]
    fn errori_in_parole_semplici() {
        assert!(friendly_error("manca il pairing remoto rp-pairing.plist: crealo").contains("missing"));
        assert!(friendly_error("pairing con l'iPhone non riuscito (Socket(... early eof ...))")
            .contains("no longer recognizes"));
        assert!(friendly_error("Os { code: 113, kind: HostUnreachable, message: \"No route to host\" }")
            .contains("cannot find the iPhone"));
        assert!(friendly_error("non trovo l'iPhone in rete: è acceso").contains("cannot find"));
        assert_eq!(friendly_error("boh"), "boh");
    }

    #[test]
    fn messaggi_web_in_inglese() {
        assert_eq!(english_job_line("Collegato via USB"), "Connected via USB");
        assert_eq!(
            english_job_line("192.168.1.2 non risponde, cerco il telefono in rete..."),
            "192.168.1.2 did not respond; searching the network..."
        );
        assert_eq!(
            english_job_line("      nuovo profilo valido fino al 2026-12-01"),
            "      New profile valid until 2026-12-01"
        );
        assert_eq!(
            short_error("nessun team sviluppatore su questo Apple ID"),
            "No developer team is available for this Apple ID."
        );
        assert_eq!(
            short_error("login Apple ID non riuscito: temporary failure"),
            "Apple ID sign-in failed: temporary failure"
        );
    }

    #[test]
    fn errori_di_apple_in_parole_semplici() {
        let lungo = "login Apple ID non riuscito: \n ● Failed to log in\n ● GrandSlam error\n \
                     ● Auth error -20101: Your account information was entered incorrectly.";
        assert_eq!(short_apple_error(lungo), "Incorrect Apple ID or password.");
        assert!(short_apple_error("HTTP status client error (429 Too Many Requests)")
            .contains("rate-limiting"));
        assert_eq!(
            short_apple_error("x\n ● uno\n ● due cose non previste"),
            "due cose non previste"
        );
    }

    #[test]
    fn storico_dal_log_del_cron_e_della_pagina() {
        let log = "\
=== 2026-09-21 19:01:16
Collegamento al telefono 192.168.1.146:49152...
    com.spotify.client.TEAM: scade tra 6 giorni, non serve rinnovare
Niente da rinnovare: non accedo ad Apple.
esito: 0
=== 2026-09-21 19:14:54
Collegamento al telefono...
Error: \"Os { code: 113, kind: HostUnreachable, message: \\\"No route to host\\\" }\"
esito: 1
=== 2026-09-25 21:17:00 (interfaccia web)
    com.spotify.client.TEAM: scade tra 2 giorni, scarico un profilo nuovo...
      installato sul telefono
   io.github.x: scade tra 2 giorni, scarico un profilo nuovo...
      installato sul telefono
esito: 0
";
        let h = parse_history(log, 5);
        assert_eq!(h.len(), 3);
        // dal più recente
        assert_eq!(h[0]["summary"], "Renewed 2 apps");
        assert_eq!(h[0]["ok"], true);
        assert!(h[0]["when"].as_str().unwrap().contains("interfaccia web"));
        assert_eq!(h[1]["ok"], false);
        assert!(h[1]["summary"].as_str().unwrap().contains("cannot find the iPhone"));
        assert_eq!(h[2]["summary"], "Nothing to renew");
        assert_eq!(parse_history(log, 1).len(), 1);
        assert!(parse_history("", 5).is_empty());
        assert!(parse_history("righe senza intestazione\n", 5).is_empty());
    }
}
