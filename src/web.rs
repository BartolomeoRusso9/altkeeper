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
    extract::{DefaultBodyLimit, Multipart, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower::ServiceExt as _;

use crate::{apple, devices, phone, profile, PAIRING_FILE};

const INDEX_HTML: &str = include_str!("web/index.html");
const APPLE_TOUCH_ICON: &[u8] = include_bytes!("web/apple-touch-icon.png");
const FAVICON_32: &[u8] = include_bytes!("web/favicon-32.png");
const RENEW_LOG: &str = "renew.log";
const AUTO_RENEW_CONFIG: &str = "state/auto-renew.json";
const DISCORD_WEBHOOK_FILE: &str = "state/discord-webhook";
const TELEGRAM_CONFIG_FILE: &str = "state/telegram-bot.json";
const AUTO_PAIR_OFF_FILE: &str = "state/auto-pair-off";
const APP_STATUS_FILE: &str = "state/app-renew-status.json";
const RETRY_SECONDS: u64 = 30 * 60;
const MAX_RETRIES: u8 = 2;
const DAY_SECS: u64 = 24 * 60 * 60;
const MAX_JOB_LINES: usize = 300;
const CHECK_TIMEOUT: Duration = Duration::from_secs(45 + 12 * phone::MAX_SAVED_ADDRESSES as u64);
const MAX_IPA_BYTES: usize = 1024 * 1024 * 1024;
const PENDING_IPA_TTL: u64 = 15 * 60;
static ANY_PROFILE_BUSY: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------------------------
// Stato condiviso
// ---------------------------------------------------------------------------------------------

struct App {
    dir: PathBuf,
    device_id: String,
    device_name: String,
    phone: Option<SocketAddr>,
    /// C'è già un lavoro in corso (controllo, rinnovo o login).
    busy: AtomicBool,
    inner: Mutex<Inner>,
    tfa: Arc<apple::TfaChannel>,
    auto_renew: Mutex<AutoRenewConfig>,
    pending_ipa: Mutex<Option<PendingIpa>>,
    renewal_wait: crate::renewal_wait::Control,
}

struct DeviceRuntime {
    app: Arc<App>,
    router: Router,
}

struct DeviceManager {
    root: PathBuf,
    pin: Option<String>,
    profiles: tokio::sync::RwLock<std::collections::HashMap<String, DeviceRuntime>>,
    devices: tokio::sync::RwLock<Vec<devices::Device>>,
    active_id: Mutex<String>,
}

#[derive(Default, Clone)]
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
    profiles: Vec<ProfileView>,
    jit_apps: Vec<JitAppView>,
}

#[derive(Clone)]
struct JitAppView {
    pid: u32,
    name: String,
    real_app_name: String,
}

#[derive(Clone, PartialEq, Debug)]
struct AppView {
    bundle: String,
    label: String,
    days: i64,
    expires: String,
    level: &'static str,
    members: Vec<MemberView>,
}

#[derive(Clone, PartialEq, Debug)]
struct MemberView {
    bundle: String,
    label: String,
    days: i64,
    expires: String,
}

#[derive(Clone, PartialEq, Debug)]
struct ProfileView {
    uuid: String,
    bundle: String,
    root: String,
    label: String,
    days: i64,
    expires: String,
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
        crate::log::line(&format!("[web] {s}"));
        let mut g = self.lock();
        g.job.lines.push(s.to_string());
        if g.job.lines.len() > MAX_JOB_LINES {
            g.job.lines.remove(0);
        }
    }
}

/// Segna il lavoro come finito quando viene lasciato (anche se il thread va in panico).
struct BusyGuard {
    app: Arc<App>,
    _auto_pair_pause: phone::AutoPairPauseGuard,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.app.busy.store(false, Ordering::SeqCst);
        ANY_PROFILE_BUSY.store(false, Ordering::SeqCst);
    }
}

fn try_busy(app: &Arc<App>) -> Option<BusyGuard> {
    ANY_PROFILE_BUSY
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .and_then(|_| {
            app.busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .ok()
                .map(|_| BusyGuard {
                    app: app.clone(),
                    _auto_pair_pause: phone::pause_auto_pairing(),
                })
                .or_else(|| {
                    ANY_PROFILE_BUSY.store(false, Ordering::SeqCst);
                    None
                })
        })
}

/// Lancia `make(app)` in un thread con un runtime tutto suo.
fn spawn_job<Fut>(guard: BusyGuard, make: impl FnOnce(Arc<App>) -> Fut + Send + 'static)
where
    Fut: std::future::Future<Output = ()>,
{
    let app = guard.app.clone();
    // Più stack del predefinito (2 MB): i futuri del telefono e di Apple sono grandi, soprattutto in debug.
    let _ = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AutoRenewConfig {
    enabled: bool,
    /// HH:MM in UTC (no daylight saving), independent of container TZ.
    time: String,
    #[serde(default = "default_min_days")]
    min_days: u8,
    /// UTC calendar day of the most recent automatic attempt.
    last_attempt_day: Option<u64>,
    #[serde(default)]
    retry_count: u8,
    #[serde(default)]
    retry_at: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    pub(crate) auto_register_app_ids: bool,
}
fn default_min_days() -> u8 {
    3
}

impl Default for AutoRenewConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            time: "06:00".into(),
            min_days: 3,
            last_attempt_day: None,
            retry_count: 0,
            retry_at: None,
            last_error: None,
            auto_register_app_ids: false,
        }
    }
}

fn parse_utc_time(value: &str) -> Option<u64> {
    let (hh, mm) = value.split_once(':')?;
    if hh.len() != 2 || mm.len() != 2 {
        return None;
    }
    if !hh.bytes().chain(mm.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hour: u64 = hh.parse().ok()?;
    let minute: u64 = mm.parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 3600 + minute * 60)
}

fn utc_day(now_secs: u64) -> u64 {
    now_secs / DAY_SECS
}

fn utc_seconds_of_day(now_secs: u64) -> u64 {
    now_secs % DAY_SECS
}

fn next_auto_run(config: &AutoRenewConfig, now_secs: u64) -> Option<u64> {
    if !config.enabled {
        return None;
    }
    if let Some(retry_at) = config.retry_at {
        return Some(retry_at);
    }
    let target = parse_utc_time(&config.time)?;
    let today = utc_day(now_secs);
    let can_run_today =
        utc_seconds_of_day(now_secs) < target && config.last_attempt_day != Some(today);
    let scheduled_day = if can_run_today { today } else { today + 1 };
    Some(scheduled_day * DAY_SECS + target)
}

pub(crate) fn load_auto_renew(dir: &Path) -> AutoRenewConfig {
    let raw = match std::fs::read_to_string(dir.join(AUTO_RENEW_CONFIG)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AutoRenewConfig::default(),
        Err(e) => {
            crate::log::err(&format!(
                "[auto-renew] Cannot read config ({e}); automatic renewal disabled"
            ));
            return AutoRenewConfig::default();
        }
    };
    match serde_json::from_str::<AutoRenewConfig>(&raw) {
        Ok(config)
            if parse_utc_time(&config.time).is_some() && (1..=6).contains(&config.min_days) =>
        {
            config
        }
        _ => {
            crate::log::err("[auto-renew] Invalid schedule config; automatic renewal disabled");
            AutoRenewConfig::default()
        }
    }
}

fn save_auto_renew(dir: &Path, config: &AutoRenewConfig) -> Result<(), String> {
    let state = dir.join("state");
    std::fs::create_dir_all(&state).map_err(crate::phone::dbg)?;
    crate::fsperm::restrict_dir(&state).map_err(crate::phone::dbg)?;
    let data = serde_json::to_vec_pretty(config).map_err(crate::phone::dbg)?;
    // Atomic write: avoid resetting the persistent on/off state after an interrupted write.
    let tmp = state.join("auto-renew.json.tmp");
    crate::fsperm::write_private(&tmp, &data).map_err(crate::phone::dbg)?;
    std::fs::rename(tmp, dir.join(AUTO_RENEW_CONFIG)).map_err(crate::phone::dbg)
}

fn retryable_renew_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    ![
        "2fa",
        "incorrect password",
        "invalid password",
        "invalid credentials",
        "unauthorized",
        "forbidden",
        "rate.limit",
        "too many requests",
        "429",
        "apple id sign-in",
        "apple id login",
    ]
    .iter()
    .any(|part| lower.contains(part))
}

fn validate_discord_webhook(url: &str) -> bool {
    let prefixes = [
        "https://discord.com/api/webhooks/",
        "https://discordapp.com/api/webhooks/",
    ];
    let Some(rest) = prefixes.iter().find_map(|prefix| url.strip_prefix(prefix)) else {
        return false;
    };
    let mut parts = rest.split('/');
    let Some(id) = parts.next() else { return false };
    let Some(token) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && id.len() >= 10
        && token.len() >= 20
        && id.bytes().all(|b| b.is_ascii_digit())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn discord_configured(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(DISCORD_WEBHOOK_FILE))
        .ok()
        .is_some_and(|url| validate_discord_webhook(url.trim()))
}

fn send_discord_failure(dir: &Path, error: &str) -> Result<(), String> {
    let message = format!(
        "AltKeeper: automatic renewal failed after retries.\n{}",
        error.chars().take(240).collect::<String>()
    );
    send_discord_message(dir, &message)
}

fn send_discord_message(dir: &Path, message: &str) -> Result<(), String> {
    use std::io::Write;
    let url = std::fs::read_to_string(dir.join(DISCORD_WEBHOOK_FILE)).map_err(crate::phone::dbg)?;
    let url = url.trim();
    if !validate_discord_webhook(url) {
        return Err("Invalid Discord webhook configuration".into());
    }
    // Pass the sensitive webhook URL via stdin (curl config), not process arguments.
    let body = json!({ "content": message }).to_string();
    let mut child = std::process::Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "12",
            "--request",
            "POST",
            "--header",
            "Content-Type: application/json",
            "--data-raw",
            &body,
            "--config",
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(crate::phone::dbg)?;
    // Validated webhook has no quote/backslash/newline, so it is safe for curl config.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("url = \"{url}\"\n").as_bytes())
            .map_err(crate::phone::dbg)?;
    }
    let output = child.wait_with_output().map_err(crate::phone::dbg)?;
    if output.status.success() {
        Ok(())
    } else {
        Err("Discord rejected webhook notification".into())
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct TelegramConfig {
    bot_token: String,
    chat_id: String,
}

fn validate_telegram_token(token: &str) -> bool {
    let Some((bot_id, secret)) = token.split_once(':') else { return false };
    bot_id.len() >= 5 && bot_id.len() <= 20 && secret.len() >= 20 && secret.len() <= 120
        && bot_id.bytes().all(|b| b.is_ascii_digit())
        && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_telegram_chat_id(chat_id: &str) -> bool {
    if chat_id.is_empty() || chat_id.len() > 64 { return false; }
    if let Some(rest) = chat_id.strip_prefix('-') {
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
    } else if let Some(rest) = chat_id.strip_prefix('@') {
        rest.len() >= 4 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    } else {
        chat_id.bytes().all(|b| b.is_ascii_digit())
    }
}

fn load_telegram_config(dir: &Path) -> Option<TelegramConfig> {
    let raw = std::fs::read_to_string(dir.join(TELEGRAM_CONFIG_FILE)).ok()?;
    let cfg: TelegramConfig = serde_json::from_str(&raw).ok()?;
    if validate_telegram_token(cfg.bot_token.trim()) && validate_telegram_chat_id(cfg.chat_id.trim()) {
        Some(TelegramConfig {
            bot_token: cfg.bot_token.trim().to_string(),
            chat_id: cfg.chat_id.trim().to_string(),
        })
    } else {
        None
    }
}

fn telegram_configured(dir: &Path) -> bool {
    load_telegram_config(dir).is_some()
}

fn send_telegram_failure(dir: &Path, error: &str) -> Result<(), String> {
    let message = format!(
        "⚠️ AltKeeper: automatic renewal failed after retries.\n{}",
        error.chars().take(240).collect::<String>()
    );
    send_telegram_message(dir, &message)
}

fn send_telegram_message(dir: &Path, message: &str) -> Result<(), String> {
    use std::io::Write;
    let Some(cfg) = load_telegram_config(dir) else {
        return Err("Telegram bot is not configured".into());
    };
    let url = format!("https://api.telegram.org/bot{}/sendMessage", cfg.bot_token);
    let body = json!({
        "chat_id": cfg.chat_id,
        "text": message,
    }).to_string();
    let mut child = std::process::Command::new("curl")
        .args([
            "--fail", "--silent", "--show-error", "--max-time", "12",
            "--request", "POST", "--header", "Content-Type: application/json",
            "--data-raw", &body, "--config", "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(crate::phone::dbg)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(format!("url = \"{url}\"\n").as_bytes()).map_err(crate::phone::dbg)?;
    }
    let output = child.wait_with_output().map_err(crate::phone::dbg)?;
    if output.status.success() {
        Ok(())
    } else {
        Err("Telegram rejected message. Check Bot Token, Chat ID, and make sure you sent /start to the bot.".into())
    }
}


async fn auto_renew_loop(app: Arc<App>) {
    loop {
        let now_secs = now();
        let guard = {
            let config = app.auto_renew.lock().unwrap_or_else(|e| e.into_inner());
            let retry_due = config.retry_at.is_some_and(|at| at <= now_secs);
            let scheduled_due = parse_utc_time(&config.time)
                .is_some_and(|target| utc_seconds_of_day(now_secs) >= target)
                && config.last_attempt_day != Some(utc_day(now_secs));
            if config.enabled && (retry_due || scheduled_due) {
                try_busy(&app)
            } else {
                None
            }
        };
        if let Some(guard) = guard {
            let mut config = app.auto_renew.lock().unwrap_or_else(|e| e.into_inner());
            if config.enabled {
                let is_retry = config.retry_at.is_some_and(|t| t <= now_secs);
                let is_new_day =
                    config.last_attempt_day != Some(utc_day(now_secs)) && config.retry_at.is_none();
                if is_retry || is_new_day {
                    let mut updated = config.clone();
                    if is_retry {
                        updated.retry_count = updated.retry_count.saturating_add(1);
                    } else {
                        updated.last_attempt_day = Some(utc_day(now_secs));
                        updated.retry_count = 0;
                    }
                    updated.retry_at = None;
                    if let Err(err) = save_auto_renew(&app.dir, &updated) {
                        crate::log::err(&format!("[auto-renew] Could not persist attempt: {err}"));
                    } else {
                        let threshold = updated.min_days as i64;
                        let attempt = updated.retry_count + 1;
                        *config = updated;
                        drop(config);
                        app.lock().job = JobView {
                            kind: "auto_renew".into(),
                            running: true,
                            lines: vec![format!("Scheduled renewal attempt {attempt}/3 (UTC).")],
                            ok: None,
                        };
                        spawn_job(guard, move |app| {
                            run_renew(app, false, None, true, threshold, true)
                        });
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
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
    dir: PathBuf,
) -> Result<(), String> {
    check_options(bind, &pin)?;
    phone::start_auto_pair_monitor();

    let registered = devices::load(&dir)?;
    let active_id = devices::active_id(&dir, &registered)?;
    let mut runtimes = std::collections::HashMap::new();
    for device in &registered {
        let runtime = create_device_runtime(&dir, device.clone(), phone);
        runtimes.insert(device.id.clone(), runtime);
    }
    let manager = Arc::new(DeviceManager {
        root: dir,
        pin: pin.clone(),
        profiles: tokio::sync::RwLock::new(runtimes),
        devices: tokio::sync::RwLock::new(registered),
        active_id: Mutex::new(active_id.clone()),
    });
    if let Some(runtime) = manager.profiles.read().await.get(&active_id) {
        crate::altserver::set_profile_dir(runtime.app.dir.clone());
    }
    let router = device_manager_router(manager.clone());

    let listener = tokio::net::TcpListener::bind(bind).await.map_err(|e| {
        crate::tr!(
            format!("non riesco ad ascoltare su {bind}: {e}"),
            format!("cannot listen on {bind}: {e}")
        )
    })?;
    crate::log::line(&crate::tr!(
        format!(
            "Interfaccia web su http://{bind} ({})",
            if manager.pin.is_some() {
                "protetta da PIN: il nome utente può essere qualsiasi"
            } else {
                "raggiungibile solo da questo computer"
            }
        ),
        format!(
            "Web interface at http://{bind} ({})",
            if manager.pin.is_some() {
                "PIN-protected: any username is accepted"
            } else {
                "only accessible from this computer"
            }
        )
    ));
    let cleanup_root = manager.root.clone();
    tokio::spawn(async move {
        loop {
            match profile::cleanup_backups_if_due(&cleanup_root, SystemTime::now()) {
                Ok(Some(removed)) => crate::log::line(&format!(
                    "[backup-cleanup] Monthly sweep: removed {removed} profiles expired over 30 days ago"
                )),
                Ok(None) => {}
                Err(e) => crate::log::err(&format!("[backup-cleanup] Failed: {e}")),
            }
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
    axum::serve(listener, router)
        .await
        .map_err(|e| format!("{e:?}"))
}

fn device_manager_router(manager: Arc<DeviceManager>) -> Router {
    Router::new()
        .route("/api/devices", get(list_devices).post(add_device))
        .route("/api/devices/select", post(select_device))
        .route("/", get(index))
        .route("/apple-touch-icon.png", get(apple_touch_icon))
        .route("/favicon-32.png", get(favicon_32))
        .fallback(dispatch_device_request)
        .layer(middleware::from_fn_with_state(
            manager.clone(),
            manager_guard,
        ))
        .with_state(manager)
}

fn create_device_runtime(
    root: &Path,
    device: devices::Device,
    phone: Option<SocketAddr>,
) -> DeviceRuntime {
    let dir = device.directory(root);
    cleanup_orphaned_ipa_uploads(&dir);
    let app = Arc::new(App {
        dir: dir.clone(),
        device_id: device.id.clone(),
        device_name: device.name.clone(),
        phone,
        busy: AtomicBool::new(false),
        inner: Mutex::new(Inner::default()),
        tfa: Arc::new(apple::TfaChannel::default()),
        auto_renew: Mutex::new(load_auto_renew(&dir)),
        pending_ipa: Mutex::new(None),
        renewal_wait: crate::renewal_wait::Control::default(),
    });
    tokio::spawn(auto_renew_loop(app.clone()));
    DeviceRuntime {
        router: profile_router(app.clone()),
        app,
    }
}

fn profile_router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/state", get(state))
        .route("/api/logs", get(logs))
        .route("/api/check", post(check))
        .route("/api/jit/apps", post(load_jit_apps))
        .route("/api/jit/enable", post(enable_jit))
        .route("/api/phone-addresses", post(update_phone_addresses))
        .route("/api/auto-pair", post(update_auto_pair))
        .route("/api/autostart", post(update_autostart))
        .route("/api/renew", post(renew))
        .route("/api/renew/cancel-wait", post(cancel_renew_wait))
        .route("/api/pair-usb", post(pair_usb))
        .route("/api/auto-renew", post(update_auto_renew))
        .route("/api/discord", post(update_discord))
        .route("/api/discord/test", post(test_discord))
        .route("/api/telegram", post(update_telegram))
        .route("/api/telegram/test", post(test_telegram))
        .route("/api/anisette", post(update_anisette))
        .route("/api/anisette/test", post(test_anisette))
        .route("/api/profile/remove", post(remove_profile))
        .route("/api/profile/restore", post(restore_profile))
        .route("/api/profile/backup/remove", post(remove_backup))
        .route(
            "/api/install",
            post(install).layer(DefaultBodyLimit::max(MAX_IPA_BYTES + 1024 * 1024)),
        )
        .route(
            "/api/install/inspect",
            post(inspect_install).layer(DefaultBodyLimit::max(MAX_IPA_BYTES + 1024 * 1024)),
        )
        .route("/api/install/confirm", post(confirm_install))
        .route("/api/login", post(login))
        .route("/api/login/code", post(login_code))
        .route("/api/login/resend", post(login_resend))
        .route("/api/logout", post(logout))
        .with_state(app.clone())
}

async fn list_devices(State(manager): State<Arc<DeviceManager>>) -> Json<Value> {
    let devices = manager.devices.read().await;
    let selected = manager
        .active_id
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    Json(json!({
        "devices": devices.iter().map(|device| json!({
            "id": device.id,
            "name": device.name,
        })).collect::<Vec<_>>(),
        "selected": selected,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddDeviceReq {
    name: String,
}

async fn add_device(
    State(manager): State<Arc<DeviceManager>>,
    Json(req): Json<AddDeviceReq>,
) -> (StatusCode, Json<Value>) {
    let mut devices = manager.devices.write().await;
    let device = match devices::add(&manager.root, &mut devices, &req.name) {
        Ok(device) => device,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let runtime = create_device_runtime(
        &manager.root,
        device.clone(),
        manager
            .profiles
            .read()
            .await
            .values()
            .next()
            .and_then(|runtime| runtime.app.phone),
    );
    manager
        .profiles
        .write()
        .await
        .insert(device.id.clone(), runtime);
    (
        StatusCode::CREATED,
        Json(json!({ "ok": true, "device": device })),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectDeviceReq {
    id: String,
}

async fn select_device(
    State(manager): State<Arc<DeviceManager>>,
    Json(req): Json<SelectDeviceReq>,
) -> (StatusCode, Json<Value>) {
    let profiles = manager.profiles.read().await;
    let Some(runtime) = profiles.get(&req.id) else {
        return json_err(StatusCode::NOT_FOUND, "Device profile not found.");
    };
    if let Err(error) = devices::save_active(&manager.root, &req.id) {
        return json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Cannot save selected device: {error}"),
        );
    }
    let directory = runtime.app.dir.clone();
    drop(profiles);
    *manager
        .active_id
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = req.id.clone();
    crate::altserver::set_profile_dir(directory);
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "selected": req.id })),
    )
}

async fn dispatch_device_request(
    State(manager): State<Arc<DeviceManager>>,
    request: Request,
) -> Response {
    let requested = request
        .headers()
        .get("x-altkeeper-device")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            manager
                .active_id
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        });
    let profiles = manager.profiles.read().await;
    let Some(runtime) = profiles.get(&requested) else {
        return json_err(
            StatusCode::NOT_FOUND,
            "Device profile not found. Refresh the device list.",
        )
        .into_response();
    };
    runtime
        .router
        .clone()
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {})
}

async fn manager_guard(
    State(manager): State<Arc<DeviceManager>>,
    req: Request,
    next: Next,
) -> Response {
    let public_icon = req.method() == Method::GET
        && matches!(
            req.uri().path(),
            "/apple-touch-icon.png" | "/favicon-32.png"
        );
    if !public_icon && manager.pin.is_none() && !host_is_local(req.headers()) {
        return (StatusCode::FORBIDDEN, "host non ammesso").into_response();
    }
    if !public_icon {
        if let Some(pin) = &manager.pin {
            if !basic_password_ok(req.headers(), pin) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let mut res = (StatusCode::UNAUTHORIZED, "serve il PIN").into_response();
                res.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static("Basic realm=\"altkeeper\""),
                );
                return res;
            }
        }
    }
    if req.method() == Method::POST
        && req.headers().get("x-requested-with").map(|v| v.as_bytes()) != Some(b"altkeeper")
    {
        return (StatusCode::FORBIDDEN, "richiesta non valida").into_response();
    }
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; connect-src 'self'; img-src 'self' data:; style-src 'unsafe-inline'; \
             script-src 'unsafe-inline'; base-uri 'none'; form-action 'none'",
        ),
    );
    res
}

// ---------------------------------------------------------------------------------------------
// Protezione
// ---------------------------------------------------------------------------------------------

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

pub fn discover_lan_ip_for(phone_hint: Option<&str>) -> Option<String> {
    let mut probes: Vec<String> = Vec::new();
    if let Some(h) = phone_hint {
        let host = h.split(':').next().unwrap_or(h).trim();
        if !host.is_empty() && host != "127.0.0.1" && host != "localhost" {
            probes.push(format!("{host}:80"));
        }
    }
    probes.push("192.168.1.1:80".to_string());
    probes.push("10.255.255.255:1".to_string());
    for target in probes {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if s.connect(&target).is_ok() {
                if let Ok(addr) = s.local_addr() {
                    let ip = addr.ip();
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        return Some(ip.to_string());
                    }
                }
            }
        }
    }
    None
}

fn basic_password_ok(headers: &HeaderMap, pin: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
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

fn png_response(bytes: &'static [u8]) -> Response {
    let mut response = Response::new(axum::body::Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response
}

async fn apple_touch_icon() -> Response {
    png_response(APPLE_TOUCH_ICON)
}
async fn favicon_32() -> Response {
    png_response(FAVICON_32)
}

fn json_err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

fn json_ok() -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({ "ok": true })))
}

fn json_job_accepted(job: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::ACCEPTED,
        Json(json!({ "ok": true, "job": job })),
    )
}

const BUSY_MSG: &str = "Another operation is already running: please wait for it to finish.";

/// Le righe di log dopo `?since=N` (0 = tutte quelle in memoria), per la sezione Logs.
async fn logs(uri: Uri) -> Json<Value> {
    let since = uri
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("since=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let (seq, lines) = crate::log::since(since);
    Json(json!({ "seq": seq, "lines": lines }))
}


#[cfg(any(windows, target_os = "macos"))]
fn is_autostart_active() -> bool {
    crate::tray::is_autostart_enabled()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn is_autostart_active() -> bool {
    false
}

async fn state(State(app): State<Arc<App>>) -> Json<Value> {
    expire_pending_ipa(&app);
    let account = apple::read_account(&app.dir);
    let pairing = app.dir.join(PAIRING_FILE).exists();

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

    let history = std::fs::read_to_string(app.dir.join(RENEW_LOG))
        .map(|t| parse_history(&t, 6))
        .unwrap_or_default();
    // Copy the view and release its mutex before reading the schedule. Never
    // hold the UI and scheduler mutexes together (retry logging also uses UI).
    let g = app.lock().clone();
    let schedule = app
        .auto_renew
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let next_scheduled = next_auto_run(&schedule, now());
    let asking = app.tfa.asking.load(Ordering::SeqCst);
    let login_phase = if asking { "needs_code" } else { g.login.phase };
    let code_error = app.tfa.last_error.lock().ok().and_then(|e| e.clone());
    let phone_addresses = phone::saved_addresses_in(&app.dir);
    let is_renewal = g.job.kind.starts_with("renew") || g.job.kind == "auto_renew";
    let anisette = apple::anisette_choice(&app.dir);

    Json(json!({
        "now": now(),
        "busy": app.busy.load(Ordering::SeqCst) || ANY_PROFILE_BUSY.load(Ordering::SeqCst),
        "pairing": pairing,
        "device": { "id": app.device_id, "name": app.device_name },
        "autostart": {
            "supported": cfg!(any(windows, target_os = "macos")),
            "enabled": is_autostart_active(),
        },
        "auto_pair": {
            "enabled": app.device_id == "default" && phone::auto_pair_configured_in(&app.dir),
            "locked": app.device_id != "default"
                || crate::env::is_set("NO_USB")
                || crate::env::is_set("NO_AUTO_PAIR"),
        },
        "auto_renew": {
            "enabled": schedule.enabled,
            "auto_register_app_ids": schedule.auto_register_app_ids,
            "time": schedule.time,
            "min_days": schedule.min_days,
            "retry_count": schedule.retry_count,
            "retry_at": schedule.retry_at,
            "last_error": schedule.last_error,
            "discord_configured": discord_configured(&app.dir),
            "telegram_configured": telegram_configured(&app.dir),
            "telegram_chat_id": load_telegram_config(&app.dir).map(|c| c.chat_id),
            "time_zone": "UTC",
            "next_run": next_scheduled,
        },
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
        "phone_addresses": {
            "addresses": phone_addresses.as_ref().map(|addresses|
                addresses.iter().map(ToString::to_string).collect::<Vec<_>>()).unwrap_or_default(),
            "error": phone_addresses.err(),
        },
        "anisette": match anisette {
            Ok(choice) => json!({
                "url": choice.url.clone(),
                "source": choice.source,
                "locked": choice.locked,
                "default": choice.url == apple::DEFAULT_ANISETTE_URL,
            }),
            Err(error) => json!({ "error": error }),
        },
        "quotas": {
            "installed_apps": g.snapshot.apps.len(),
            "max_installed_apps": 3,
            // Ogni profilo più recente per bundle (app principale + estensioni) è un App ID.
            "app_ids_used": g.snapshot.apps.iter().map(|a| a.members.len()).sum::<usize>(),
            "max_app_ids": 10,
        },
        "apps": g.snapshot.apps.iter().map(|a| json!({
            "bundle": a.bundle, "label": a.label, "days": a.days,
            "expires": a.expires, "level": a.level,
            "members": a.members.iter().map(|m| json!({
                "bundle":m.bundle,"label":m.label,"days":m.days,"expires":m.expires
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "profiles": g.snapshot.profiles.iter().map(|p| json!({
            "uuid": p.uuid, "bundle": p.bundle, "root": p.root, "label": p.label,
            "days": p.days, "expires": p.expires,
        })).collect::<Vec<_>>(),
        "jit_apps": g.snapshot.jit_apps.iter().map(|app| json!({
            "pid": app.pid, "name": app.name, "real_app_name": app.real_app_name,
        })).collect::<Vec<_>>(),
        "job": {
            "kind": g.job.kind, "running": g.job.running,
            "lines": g.job.lines, "ok": g.job.ok,
            "waiting_for_phone": is_renewal && g.job.running && app.renewal_wait.is_waiting(),
            "cancel_requested": is_renewal && app.renewal_wait.is_canceled(),
            "canceled": is_renewal && g.job.ok == Some(false) && app.renewal_wait.is_canceled(),
        },
        "login": {
            "phase": login_phase,
            "message": g.login.message,
            "code_error": if asking { code_error } else { None },
        },
        "history": history,
        "backups": account.as_ref().and_then(|a| a.team_id.as_deref())
            .map(|team| backup_views(&app.dir, team)).unwrap_or_default(),
        "lan_ip": discover_lan_ip_for(g.snapshot.address.as_deref()),
        "app_status": std::fs::read_to_string(app.dir.join(APP_STATUS_FILE)).ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .unwrap_or(json!({})),
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

async fn load_jit_apps(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    app.lock().job = JobView {
        kind: "jit_apps".into(),
        running: true,
        lines: vec!["Reading apps currently running on the iPhone…".into()],
        ok: None,
    };
    spawn_job(guard, run_load_jit_apps);
    json_job_accepted("jit_apps")
}

async fn run_load_jit_apps(app: Arc<App>) {
    let _work = crate::altserver::phone_lock();
    let result = phone::with_timeout("Listing running apps", Duration::from_secs(45), async {
        let (_, mut link) =
            phone::connect_auto_in(&app.dir, app.phone, PAIRING_FILE, |_: &str| {}).await?;
        link.running_apps().await
    })
    .await;
    let mut g = app.lock();
    match result {
        Ok(mut processes) => {
            processes.sort_by(|left, right| {
                left.real_app_name
                    .cmp(&right.real_app_name)
                    .then_with(|| left.name.cmp(&right.name))
                    .then_with(|| left.pid.cmp(&right.pid))
            });
            g.snapshot.jit_apps = processes
                .into_iter()
                .map(|process| JitAppView {
                    pid: process.pid,
                    name: process.name,
                    real_app_name: process.real_app_name,
                })
                .collect();
            let count = g.snapshot.jit_apps.len();
            g.job.lines.push(format!("Found {} running app(s).", count));
            g.job.ok = Some(true);
        }
        Err(error) => {
            g.snapshot.jit_apps.clear();
            g.job.lines.push(format!("Error: {}", short_error(&error)));
            g.job.ok = Some(false);
        }
    }
    g.job.running = false;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnableJitReq {
    pid: u32,
}

async fn enable_jit(
    State(app): State<Arc<App>>,
    Json(req): Json<EnableJitReq>,
) -> (StatusCode, Json<Value>) {
    if req.pid == 0
        || !app
            .lock()
            .snapshot
            .jit_apps
            .iter()
            .any(|process| process.pid == req.pid)
    {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Refresh the running-app list and select an app.",
        );
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    app.lock().job = JobView {
        kind: "jit_enable".into(),
        running: true,
        lines: vec![format!("Enabling JIT for process {}…", req.pid)],
        ok: None,
    };
    spawn_job(guard, move |app| run_enable_jit(app, req.pid));
    json_job_accepted("jit_enable")
}

async fn run_enable_jit(app: Arc<App>, pid: u32) {
    let _work = crate::altserver::phone_lock();
    let result = phone::with_timeout("Enabling JIT", Duration::from_secs(60), async {
        let (_, mut link) =
            phone::connect_auto_in(&app.dir, app.phone, PAIRING_FILE, |_: &str| {}).await?;
        link.enable_jit(pid).await
    })
    .await;
    let mut g = app.lock();
    match result {
        Ok(()) => {
            g.job.lines.push(format!("JIT enabled for process {pid}."));
            g.job.ok = Some(true);
        }
        Err(error) => {
            g.job.lines.push(format!("Error: {}", short_error(&error)));
            g.job.ok = Some(false);
        }
    }
    g.job.running = false;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhoneAddressesReq {
    addresses: Vec<String>,
}

async fn update_phone_addresses(
    State(app): State<Arc<App>>,
    Json(req): Json<PhoneAddressesReq>,
) -> (StatusCode, Json<Value>) {
    if let Err(error) = phone::validate_addresses(&req.addresses) {
        return json_err(StatusCode::BAD_REQUEST, &error);
    }
    match phone::save_addresses_in(&app.dir, &req.addresses) {
        Ok(addresses) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "addresses": addresses.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })),
        ),
        Err(error) => json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Cannot save iPhone addresses: {error}"),
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AutoPairReq {
    enabled: bool,
}

async fn update_auto_pair(
    State(app): State<Arc<App>>,
    Json(req): Json<AutoPairReq>,
) -> (StatusCode, Json<Value>) {
    if app.device_id != "default" {
        return json_err(
            StatusCode::CONFLICT,
            "Automatic USB pairing is currently managed by the default device profile.",
        );
    }
    if req.enabled && (crate::env::is_set("NO_USB") || crate::env::is_set("NO_AUTO_PAIR")) {
        return json_err(
            StatusCode::CONFLICT,
            "Automatic pairing is disabled by ALTKEEPER_NO_USB or ALTKEEPER_NO_AUTO_PAIR.",
        );
    }
    if req.enabled {
        match std::fs::remove_file(app.dir.join(AUTO_PAIR_OFF_FILE)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return json_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Cannot enable automatic pairing: {error}"),
                )
            }
        }
    } else {
        let state = app.dir.join("state");
        if let Err(error) = std::fs::create_dir_all(&state)
            .and_then(|_| crate::fsperm::restrict_dir(&state))
            .and_then(|_| crate::fsperm::write_private(&app.dir.join(AUTO_PAIR_OFF_FILE), b"1"))
        {
            return json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Cannot disable automatic pairing: {error}"),
            );
        }
    }
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "enabled": req.enabled })),
    )
}

async fn pair_usb(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    app.lock().job = JobView {
        kind: "pair_usb".into(),
        running: true,
        lines: vec!["Pairing iPhone over USB…".into()],
        ok: None,
    };
    let output = app.dir.join(PAIRING_FILE);
    spawn_job(guard, move |app| run_pair_usb(app, output));
    json_ok()
}

async fn run_pair_usb(app: Arc<App>, output: PathBuf) {
    let result = phone::pair_usb(&output.to_string_lossy()).await;
    match &result {
        Ok(()) => app.push_line("USB pairing created successfully."),
        Err(error) => app.push_line(&format!("Error: {error}")),
    }
    let mut g = app.lock();
    g.job.running = false;
    g.job.ok = Some(result.is_ok());
}

async fn cancel_renew_wait(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    let g = app.lock();
    if !g.job.running || !app.renewal_wait.is_waiting() {
        return json_err(StatusCode::CONFLICT, "No iPhone response wait is active.");
    }
    app.renewal_wait.cancel();
    json_ok()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnisetteReq {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    clear: bool,
}

async fn update_anisette(
    State(app): State<Arc<App>>,
    Json(req): Json<AnisetteReq>,
) -> (StatusCode, Json<Value>) {
    let Some(_guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    if req.clear && req.url.is_some() {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Choose a URL or restore the default, not both.",
        );
    }
    if !req.clear && req.url.is_none() {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Supply an anisette URL or set clear=true.",
        );
    }
    let choice = match apple::save_anisette_choice(
        &app.dir,
        if req.clear { None } else { req.url.as_deref() },
    ) {
        Ok(choice) => choice,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    crate::altserver::clear_anisette_cache().await;
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "url": choice.url.clone(),
            "source": choice.source,
            "locked": choice.locked,
            "default": choice.url == apple::DEFAULT_ANISETTE_URL,
        })),
    )
}

#[derive(Deserialize, Default)]
struct TestAnisetteReq {
    #[serde(default)]
    url: Option<String>,
}

async fn test_anisette(
    State(app): State<Arc<App>>,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let req: TestAnisetteReq = serde_json::from_slice(&body).unwrap_or_default();
    let Some(_guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    match apple::test_anisette_url(&app.dir, req.url.as_deref()).await {
        Ok(_) => json_ok(),
        Err(error) => json_err(
            StatusCode::BAD_GATEWAY,
            &format!("Anisette server test failed: {error}"),
        ),
    }
}

#[derive(Deserialize)]
struct AutoRenewReq {
    enabled: bool,
    time: String,
    #[serde(default = "default_min_days")]
    min_days: u8,
    #[serde(default)]
    auto_register_app_ids: Option<bool>,
}

async fn update_auto_renew(
    State(app): State<Arc<App>>,
    Json(req): Json<AutoRenewReq>,
) -> (StatusCode, Json<Value>) {
    let Some(target) = parse_utc_time(&req.time) else {
        return json_err(StatusCode::BAD_REQUEST, "Use a valid 24-hour time (HH:MM).");
    };
    if !(1..=6).contains(&req.min_days) {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Renewal threshold must be 1-6 days.",
        );
    }
    if req.enabled {
        match apple::read_account(&app.dir) {
            Some(acc) if !acc.password.is_empty() && acc.team_id.is_some() => {}
            _ => {
                return json_err(
                    StatusCode::BAD_REQUEST,
                    "Save your Apple ID password before enabling automatic renewal.",
                )
            }
        }
    }
    let mut config = app.auto_renew.lock().unwrap_or_else(|e| e.into_inner());
    let mut updated = config.clone();
    // Turning on or changing the clock time after today's scheduled time
    // must NOT immediately trigger an unrequested refresh.
    if req.enabled
        && (!config.enabled || config.time != req.time)
        && utc_seconds_of_day(now()) >= target
    {
        updated.last_attempt_day = Some(utc_day(now()));
    }
    updated.enabled = req.enabled;
    if let Some(value) = req.auto_register_app_ids {
        updated.auto_register_app_ids = value;
    }
    updated.time = req.time;
    updated.min_days = req.min_days;
    if !req.enabled {
        updated.retry_at = None;
        updated.retry_count = 0;
    }
    if let Err(err) = save_auto_renew(&app.dir, &updated) {
        return json_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Cannot save schedule: {err}"),
        );
    }
    *config = updated;
    json_ok()
}

fn backup_views(dir: &Path, team: &str) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(dir.join(crate::SAVED_DIR)) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mobileprovision") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(p) = profile::parse(&bytes) else {
            continue;
        };
        if p.team_id.as_deref() != Some(team)
            || path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_none_or(|stem| !stem.eq_ignore_ascii_case(&p.uuid))
        {
            continue;
        }
        result.push(json!({
            "uuid": p.uuid, "bundle": p.bundle_id, "expires":p.expires_text(),
            "days": p.days_left()
        }));
    }
    result.sort_by(|a, b| a["bundle"].as_str().cmp(&b["bundle"].as_str()));
    result
}

#[derive(Deserialize)]
struct DiscordReq {
    #[serde(default)]
    webhook: Option<String>,
    #[serde(default)]
    clear: bool,
}

async fn update_discord(
    State(app): State<Arc<App>>,
    Json(req): Json<DiscordReq>,
) -> (StatusCode, Json<Value>) {
    if req.clear {
        match std::fs::remove_file(app.dir.join(DISCORD_WEBHOOK_FILE)) {
            Ok(()) => return json_ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return json_ok(),
            Err(_) => {
                return json_err(StatusCode::INTERNAL_SERVER_ERROR, "Could not clear webhook")
            }
        }
    }
    let Some(webhook) = req.webhook else {
        return json_err(StatusCode::BAD_REQUEST, "Supply a Discord webhook URL.");
    };
    if !validate_discord_webhook(&webhook) {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Expected a valid Discord webhook URL.",
        );
    }
    let state = app.dir.join("state");
    if std::fs::create_dir_all(&state).is_err()
        || crate::fsperm::restrict_dir(&state).is_err()
        || crate::fsperm::write_private(&app.dir.join(DISCORD_WEBHOOK_FILE), webhook.as_bytes())
            .is_err()
    {
        return json_err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save webhook");
    }
    json_ok()
}

/// User-initiated delivery check; never creates or renews a profile.
async fn test_discord(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    if !discord_configured(&app.dir) {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Configure a Discord webhook first.",
        );
    }
    // curl is synchronous; leave Tokio free while we wait for Discord.
    let dir = app.dir.clone();
    let sent = tokio::task::spawn_blocking(move || {
        send_discord_message(
            &dir,
            "AltKeeper test notification: Discord webhook is working. No renewal was performed.",
        )
    })
    .await;
    match sent {
        Ok(Ok(())) => json_ok(),
        _ => json_err(
            StatusCode::BAD_GATEWAY,
            "Discord test delivery failed. Check webhook permissions and network access.",
        ),
    }
}

#[derive(Deserialize)]
struct TelegramReq {
    #[serde(default)]
    bot_token: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    clear: bool,
}

async fn update_telegram(
    State(app): State<Arc<App>>,
    Json(req): Json<TelegramReq>,
) -> (StatusCode, Json<Value>) {
    if req.clear {
        match std::fs::remove_file(app.dir.join(TELEGRAM_CONFIG_FILE)) {
            Ok(()) => return json_ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return json_ok(),
            Err(_) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, "Could not clear Telegram config"),
        }
    }
    let token = req.bot_token.unwrap_or_default().trim().to_string();
    let chat_id = req.chat_id.unwrap_or_default().trim().to_string();
    if !validate_telegram_token(&token) {
        return json_err(StatusCode::BAD_REQUEST, "Enter a valid Telegram Bot Token (e.g. 123456789:AAF...).");
    }
    if !validate_telegram_chat_id(&chat_id) {
        return json_err(StatusCode::BAD_REQUEST, "Enter a valid Telegram Chat ID (e.g. 987654321 or -100...).");
    }
    let data = match serde_json::to_vec_pretty(&TelegramConfig { bot_token: token, chat_id }) {
        Ok(d) => d,
        Err(_) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, "Could not serialize Telegram config"),
    };
    let state_dir = app.dir.join("state");
    if std::fs::create_dir_all(&state_dir).is_err()
        || crate::fsperm::restrict_dir(&state_dir).is_err()
        || crate::fsperm::write_private(&app.dir.join(TELEGRAM_CONFIG_FILE), &data).is_err()
    {
        return json_err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save Telegram config");
    }
    json_ok()
}

async fn test_telegram(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    if !telegram_configured(&app.dir) {
        return json_err(StatusCode::BAD_REQUEST, "Configure and save your Telegram Bot Token and Chat ID first.");
    }
    let dir = app.dir.clone();
    let sent = tokio::task::spawn_blocking(move || {
        send_telegram_message(
            &dir,
            "✅ AltKeeper test notification: Your Telegram bot is connected and working! No renewal was performed."
        )
    }).await;
    match sent {
        Ok(Ok(())) => json_ok(),
        Ok(Err(e)) => json_err(StatusCode::BAD_GATEWAY, &e),
        _ => json_err(StatusCode::BAD_GATEWAY, "Telegram test delivery failed."),
    }
}

#[derive(Deserialize)]
struct RemoveBackupReq {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    uuids: Vec<String>,
}

async fn remove_backup(
    State(app): State<Arc<App>>,
    Json(req): Json<RemoveBackupReq>,
) -> (StatusCode, Json<Value>) {
    if app.busy.load(Ordering::SeqCst) || ANY_PROFILE_BUSY.load(Ordering::SeqCst) {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    }
    let team = apple::read_account(&app.dir).and_then(|a| a.team_id);
    let Some(team) = team else {
        return json_err(StatusCode::BAD_REQUEST, "Sign in before managing profile backups.");
    };
    let mut targets = req.uuids;
    if targets.is_empty() {
        if let Some(u) = req.uuid {
            targets.push(u);
        }
    }
    targets.retain(|u| !u.trim().is_empty());
    if targets.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "No backup specified for removal.");
    }
    let existing = backup_views(&app.dir, &team);
    for u in &targets {
        if !existing.iter().any(|p| p["uuid"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(u))) {
            return json_err(StatusCode::NOT_FOUND, "Profile backup was not found.");
        }
    }
    for u in &targets {
        if u.len() <= 64 && u.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            let path = app.dir.join(crate::SAVED_DIR).join(format!("{u}.mobileprovision"));
            let _ = std::fs::remove_file(&path);
            crate::log::line(&format!("[backup] Deleted saved profile backup {u}."));
        }
    }
    json_ok()
}

#[derive(Deserialize)]
struct RestoreReq {
    uuid: String,
}

async fn restore_profile(
    State(app): State<Arc<App>>,
    Json(req): Json<RestoreReq>,
) -> (StatusCode, Json<Value>) {
    let team = apple::read_account(&app.dir).and_then(|a| a.team_id);
    let Some(team) = team else {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Sign in before restoring profiles.",
        );
    };
    if !backup_views(&app.dir, &team)
        .iter()
        .any(|p| p["uuid"].as_str() == Some(&req.uuid))
    {
        return json_err(
            StatusCode::NOT_FOUND,
            "Verified profile backup was not found.",
        );
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    app.lock().job = JobView {
        kind: "restore_profile".into(),
        running: true,
        lines: Vec::new(),
        ok: None,
    };
    spawn_job(guard, move |app| run_restore_profile(app, req.uuid, team));
    json_ok()
}

async fn run_restore_profile(app: Arc<App>, uuid: String, team: String) {
    let result: Result<String, String> = async {
        // Only original verified UUID-named files for the configured Apple team.
        if uuid.len() > 64
            || uuid.is_empty()
            || !uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        {
            return Err("Invalid profile UUID".into());
        }
        let path = app.dir.join(crate::SAVED_DIR).join(format!("{uuid}.mobileprovision"));
        let bytes = std::fs::read(path).map_err(crate::phone::dbg)?;
        let p = profile::parse(&bytes).ok_or("Invalid backup data")?;
        if !p.uuid.eq_ignore_ascii_case(&uuid) || p.team_id.as_deref() != Some(&team) {
            return Err("Backup does not belong to the configured Apple team".into());
        }
        let (_, mut link) =
            phone::connect_auto_in(&app.dir, app.phone, PAIRING_FILE, |_: &str| {}).await?;
        phone::with_timeout(
            "Profile restore",
            Duration::from_secs(60),
            link.install_profile(bytes),
        )
        .await?;
        Ok(format!(
            "Restored profile {}. Original backup retained.",
            uuid
        ))
    }
    .await;
    let ok = result.is_ok();
    match result {
        Ok(line) => app.push_line(&line),
        Err(e) => app.push_line(&format!("Error: {}", short_error(&e))),
    }
    {
        let mut g = app.lock();
        g.job.running = false;
        g.job.ok = Some(ok);
    }
    run_check(app).await;
}

#[derive(Deserialize)]
struct RenewReq {
    /// "due" renews expiring installed apps; "now" all profiles; "single" one bundle.
    mode: String,
    bundle: Option<String>,
}

async fn renew(
    State(app): State<Arc<App>>,
    Json(req): Json<RenewReq>,
) -> (StatusCode, Json<Value>) {
    let (force, installed_only, bundle) = match req.mode.as_str() {
        "now" => (true, false, None),
        "due" => (false, true, None),
        "single" => {
            let Some(b) = req.bundle.filter(|b| !b.is_empty() && b.len() <= 255) else {
                return json_err(StatusCode::BAD_REQUEST, "Select an app to renew.");
            };
            if !app.lock().snapshot.apps.iter().any(|a| a.bundle == b) {
                return json_err(
                    StatusCode::NOT_FOUND,
                    "App is not in the current profile list.",
                );
            }
            (true, false, Some(b))
        }
        _ => return json_err(StatusCode::BAD_REQUEST, "Invalid mode"),
    };
    match apple::read_account(&app.dir) {
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
        app.renewal_wait.reset();
        g.job = JobView {
            kind: if bundle.is_some() {
                "renew_one".into()
            } else if force {
                "renew_all".into()
            } else {
                "renew_due".into()
            },
            running: true,
            lines: Vec::new(),
            ok: None,
        };
    }
    let threshold = app
        .auto_renew
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .min_days as i64;
    spawn_job(guard, move |app| {
        run_renew(app, force, bundle, installed_only, threshold, false)
    });
    json_ok()
}

#[derive(Deserialize)]
struct RemoveProfileReq {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    uuids: Vec<String>,
}

async fn remove_profile(
    State(app): State<Arc<App>>,
    Json(req): Json<RemoveProfileReq>,
) -> (StatusCode, Json<Value>) {
    let mut targets = req.uuids;
    if targets.is_empty() {
        if let Some(u) = req.uuid {
            targets.push(u);
        }
    }
    let mut seen = std::collections::HashSet::new();
    targets.retain(|u| {
        let trimmed = u.trim();
        !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase())
    });
    if targets.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "No profiles specified for removal.");
    }
    {
        let profiles = &app.lock().snapshot.profiles;
        for u in &targets {
            if !profiles.iter().any(|p| p.uuid.eq_ignore_ascii_case(u)) {
                return json_err(
                    StatusCode::NOT_FOUND,
                    "Profile is not in your team's current list. Check again.",
                );
            }
        }
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    {
        let mut g = app.lock();
        g.job = JobView {
            kind: "remove_profile".into(),
            running: true,
            lines: vec![format!("Connecting to the iPhone to back up and remove {} profile(s)…", targets.len())],
            ok: None,
        };
    }
    spawn_job(guard, move |app| run_remove_profiles(app, targets));
    json_ok()
}

/// Remove abandoned uploads from previous processes (the active process owns none at startup).
fn cleanup_orphaned_ipa_uploads(root: &Path) {
    let dir = root.join("state/uploads");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("install-") && name.ends_with(".ipa") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn expire_pending_ipa(app: &App) {
    let mut pending = app.pending_ipa.lock().unwrap_or_else(|e| e.into_inner());
    if pending
        .as_ref()
        .is_some_and(|p| now().saturating_sub(p.created_at) > PENDING_IPA_TTL)
    {
        *pending = None;
    }
}

struct UploadedIpa(PathBuf);

/// One preflight upload only, auto-expiring and deleted on replacement, abort or restart.
struct PendingIpa {
    upload: UploadedIpa,
    extensions: Vec<isideload::sideload::application::IpaExtensionInfo>,
    source_bundle_id: String,
    token: String,
    created_at: u64,
}

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

fn new_upload_file(root: &Path) -> Result<(PathBuf, std::fs::File), String> {
    let dir = root.join("state").join("uploads");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create upload directory: {e}"))?;
    crate::fsperm::restrict_dir(&dir)
        .map_err(|e| format!("Cannot protect upload directory: {e}"))?;

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

async fn receive_ipa(
    multipart: &mut Multipart,
    root: &Path,
) -> Result<UploadedIpa, (StatusCode, String)> {
    use std::io::Write as _;

    let mut upload: Option<UploadInProgress> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid upload: {e}")))?
    {
        if field.name() != Some("ipa") || upload.is_some() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Send one IPA file in the 'ipa' field.".into(),
            ));
        }
        let filename = field.file_name().unwrap_or("");
        if !filename.to_ascii_lowercase().ends_with(".ipa") {
            return Err((
                StatusCode::BAD_REQUEST,
                "Choose a file with the .ipa extension.".into(),
            ));
        }
        let (path, file) =
            new_upload_file(root).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
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
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Upload state was lost.".into(),
                ));
            };
            let next_size = upload.size.saturating_add(chunk.len());
            if next_size > MAX_IPA_BYTES {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "IPA exceeds the 1 GiB limit.".into(),
                ));
            }
            upload
                .file
                .as_mut()
                .ok_or_else(|| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Upload file was closed unexpectedly.".into(),
                    )
                })?
                .write_all(&chunk)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Cannot save upload: {e}"),
                    )
                })?;
            upload.size = next_size;
        }
    }

    let upload = upload.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Choose an IPA file to install.".into(),
        )
    })?;
    if upload.size == 0 {
        return Err((StatusCode::BAD_REQUEST, "The IPA file is empty.".into()));
    }
    Ok(upload.finish())
}

async fn install(
    State(app): State<Arc<App>>,
    mut multipart: Multipart,
) -> (StatusCode, Json<Value>) {
    match apple::read_account(&app.dir) {
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
    let uploaded = match receive_ipa(&mut multipart, &app.dir).await {
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
    spawn_job(guard, move |app| {
        run_install(app, uploaded, None, None, false)
    });
    json_ok()
}

/// Keep old clients' /api/install route intact. The web UI uses inspect/confirm
/// to avoid signing or registering unwanted app extensions.
async fn inspect_install(
    State(app): State<Arc<App>>,
    uri: axum::http::Uri,
    mut multipart: Multipart,
) -> (StatusCode, Json<Value>) {
    match apple::read_account(&app.dir) {
        Some(a) if !a.password.is_empty() && a.team_id.is_some() => {}
        _ => {
            return json_err(
                StatusCode::BAD_REQUEST,
                "Save an Apple ID account before installing.",
            )
        }
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    let uploaded = match receive_ipa(&mut multipart, &app.dir).await {
        Ok(file) => file,
        Err((status, detail)) => return json_err(status, &detail),
    };
    // Cancel any previous selection as soon as a replacement IPA is accepted.
    *app.pending_ipa.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let manifest = match isideload::sideload::application::inspect_ipa(&uploaded.0) {
        Ok(entries) => entries,
        Err(e) => {
            return json_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("Cannot inspect the IPA: {e}"),
            )
        }
    };
    let customize = uri
        .query()
        .is_some_and(|q| q.split('&').any(|part| part == "customize=true"));
    let enable_jit = uri
        .query()
        .is_some_and(|q| q.split('&').any(|part| part == "enable_jit=true"));
    if manifest.extensions.is_empty() && !customize {
        app.lock().job = JobView {
            kind: "install".into(),
            running: true,
            lines: vec!["IPA uploaded (no extensions). Signing and installing…".into()],
            ok: None,
        };
        spawn_job(guard, move |app| {
            run_install(app, uploaded, None, None, enable_jit)
        });
        return (StatusCode::OK, Json(json!({ "installing": true })));
    }
    // For the rare archive containing many extensions, cap the preview surface.
    if manifest.extensions.len() > 100 {
        return json_err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "IPA contains too many extensions.",
        );
    }
    let token = uploaded
        .0
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let preview = manifest
        .extensions
        .iter()
        .map(|ext| {
            json!({
                "bundle_id": ext.bundle_id, "name": ext.name,
            })
        })
        .collect::<Vec<_>>();
    let mut pending = app.pending_ipa.lock().unwrap_or_else(|e| e.into_inner());
    *pending = Some(PendingIpa {
        upload: uploaded,
        extensions: manifest.extensions,
        source_bundle_id: manifest.bundle_id.clone(),
        token: token.clone(),
        created_at: now(),
    });
    (
        StatusCode::OK,
        Json(json!({
            "needs_selection": true, "token": token, "extensions": preview,
            "app_name": manifest.name, "bundle_id": manifest.bundle_id,
            "max_app_ids": 1 + preview.len(),
        })),
    )
}

#[derive(Deserialize)]
struct ConfirmInstallReq {
    token: String,
    keep_extensions: Vec<String>,
    #[serde(default)]
    enable_jit: bool,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    bundle_id: Option<String>,
}

/// Overrides are only accepted for new installs, never a refresh.
struct NewInstallIdentity {
    display_name: Option<String>,
    bundle_id: Option<String>,
    source_bundle_id: String,
}

fn validated_install_identity(
    name: Option<String>,
    bundle: Option<String>,
    original: &str,
) -> Result<Option<NewInstallIdentity>, String> {
    let name = name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let bundle = bundle
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if name
        .as_ref()
        .is_some_and(|n| n.chars().count() > 40 || n.chars().any(char::is_control))
    {
        return Err("App name must be at most 40 characters with no control characters.".into());
    }
    if bundle
        .as_ref()
        .is_some_and(|id| !isideload::sideload::application::valid_custom_bundle_id(id))
    {
        return Err("Custom Bundle ID must be reverse-DNS format (e.g. com.example.clone).".into());
    }
    if bundle.as_deref() == Some(original) {
        return Err("Custom Bundle ID must differ from the original ID.".into());
    }
    if name.is_none() && bundle.is_none() {
        return Ok(None);
    }
    Ok(Some(NewInstallIdentity {
        display_name: name,
        bundle_id: bundle,
        source_bundle_id: original.to_string(),
    }))
}

async fn confirm_install(
    State(app): State<Arc<App>>,
    Json(req): Json<ConfirmInstallReq>,
) -> (StatusCode, Json<Value>) {
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    let mut pending = app.pending_ipa.lock().unwrap_or_else(|e| e.into_inner());
    if pending
        .as_ref()
        .is_some_and(|p| now().saturating_sub(p.created_at) > PENDING_IPA_TTL)
    {
        *pending = None;
    }
    let Some(prepared) = pending.as_ref() else {
        return json_err(StatusCode::GONE, "IPA selection expired. Upload it again.");
    };
    if prepared.token != req.token {
        return json_err(StatusCode::BAD_REQUEST, "Invalid install selection.");
    }
    let allowed: std::collections::HashSet<&str> = prepared
        .extensions
        .iter()
        .map(|e| e.bundle_id.as_str())
        .collect();
    let mut unique = std::collections::HashSet::new();
    if req
        .keep_extensions
        .iter()
        .any(|id| !allowed.contains(id.as_str()) || !unique.insert(id.as_str()))
    {
        return json_err(
            StatusCode::BAD_REQUEST,
            "Unknown or duplicate extension selection.",
        );
    }
    let identity = match validated_install_identity(
        req.display_name,
        req.bundle_id,
        &prepared.source_bundle_id,
    ) {
        Ok(v) => v,
        Err(msg) => return json_err(StatusCode::BAD_REQUEST, &msg),
    };
    let prepared = pending.take().expect("checked pending");
    drop(pending);
    app.lock().job = JobView {
        kind: "install".into(),
        running: true,
        lines: vec![format!(
            "IPA uploaded. Keeping {} of {} extensions.",
            req.keep_extensions.len(),
            prepared.extensions.len()
        )],
        ok: None,
    };
    spawn_job(guard, move |app| {
        run_install(
            app,
            prepared.upload,
            Some(req.keep_extensions),
            identity,
            req.enable_jit,
        )
    });
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
    let id = req.apple_id.trim().to_lowercase();
    if id.is_empty() || id.len() > 200 || req.password.is_empty() || req.password.len() > 200 {
        return json_err(StatusCode::BAD_REQUEST, "Enter your Apple ID and password.");
    }
    let Some(guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    app.lock().login = LoginView {
        phase: "working",
        message: String::new(),
    };
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

async fn logout(State(app): State<Arc<App>>) -> (StatusCode, Json<Value>) {
    let Some(_guard) = try_busy(&app) else {
        return json_err(StatusCode::CONFLICT, BUSY_MSG);
    };
    if let Err(e) = apple::remove_account(&app.dir) {
        return json_err(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    let mut current_cfg = load_auto_renew(&app.dir);
    if current_cfg.enabled {
        current_cfg.enabled = false;
        let _ = save_auto_renew(&app.dir, &current_cfg);
        *app.auto_renew.lock().unwrap() = current_cfg;
    }
    app.lock().login = LoginView {
        phase: "done",
        message: "Signed out successfully.".into(),
    };
    json_ok()
}

// ---------------------------------------------------------------------------------------------
// I tre lavori
// ---------------------------------------------------------------------------------------------

/// Guarda cosa c'è sul telefono (senza toccare Apple).
async fn run_check(app: Arc<App>) {
    app.lock().snapshot.checking = true;
    let team = apple::read_account(&app.dir).and_then(|a| a.team_id);

    let result = tokio::time::timeout(CHECK_TIMEOUT, async {
        let (addr, mut link) =
            phone::connect_auto_in(&app.dir, app.phone, PAIRING_FILE, |_: &str| {}).await?;
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
            g.snapshot.apps = team
                .as_deref()
                .map(|t| app_views(&profiles, t))
                .unwrap_or_default();
            g.snapshot.profiles = team
                .as_deref()
                .map(|t| profile_views(&profiles, t))
                .unwrap_or_default();
        }
        Ok(Err(e)) => {
            g.snapshot.apps.clear();
            g.snapshot.profiles.clear();
            g.snapshot.reachable = Some(false);
            g.snapshot.error = Some(friendly_error(&e));
            g.snapshot.detail = Some(e.chars().take(400).collect());
        }
        Err(_) => {
            g.snapshot.apps.clear();
            g.snapshot.profiles.clear();
            g.snapshot.reachable = Some(false);
            g.snapshot.error = Some(friendly_error("non risponde"));
            g.snapshot.detail = Some(format!(
                "nessuna risposta entro {} secondi",
                CHECK_TIMEOUT.as_secs()
            ));
        }
    }
}

async fn run_renew(
    app: Arc<App>,
    force: bool,
    bundle: Option<String>,
    installed_only: bool,
    min_days: i64,
    automatic: bool,
) {
    let mut say = |s: &str| app.push_line(&english_job_line(s));
    let result = crate::cmd_renew(
        &app.dir,
        app.phone,
        false,
        force,
        min_days,
        bundle.as_deref(),
        installed_only,
        Some(&app.renewal_wait),
        Some(app.tfa.clone()),
        &mut say,
    )
    .await;
    let ok = result.is_ok();
    let error = result.as_ref().err().map(|err| short_error(err));
    if let Some(err) = &error {
        app.push_line(&format!("Error: {err}"));
    }
    let lines = {
        let mut g = app.lock();
        g.job.running = false;
        g.job.ok = Some(ok);
        g.job.lines.clone()
    };
    append_log(&app.dir, &lines, ok);
    save_app_renew_status(&app, &lines, bundle.as_deref());

    if automatic {
        let mut config = app.auto_renew.lock().unwrap_or_else(|e| e.into_inner());
        let mut updated = config.clone();
        let mut notify = false;
        let mut retry_message = None;
        if ok {
            updated.retry_count = 0;
            updated.retry_at = None;
            updated.last_error = None;
        } else {
            let err = error.as_deref().unwrap_or("Unknown renewal error");
            updated.last_error = Some(err.to_owned());
            if updated.enabled && updated.retry_count < MAX_RETRIES && retryable_renew_error(err) {
                updated.retry_at = Some(now() + RETRY_SECONDS);
                retry_message = Some(format!(
                    "Automatic retry {}/{} in 30 minutes.",
                    updated.retry_count + 1,
                    MAX_RETRIES
                ));
            } else {
                updated.retry_at = None;
                notify = true;
            }
        }
        if let Err(e) = save_auto_renew(&app.dir, &updated) {
            crate::log::err(&format!("[auto-renew] Could not persist retry state: {e}"));
            retry_message = None;
        } else {
            *config = updated;
        }
        drop(config);
        // Only announce a persisted retry, after releasing the schedule lock.
        if let Some(message) = retry_message {
            app.push_line(&message);
        }
        if notify && discord_configured(&app.dir) {
            let err = error.as_deref().unwrap_or("Unknown renewal error");
            match send_discord_failure(&app.dir, err) {
                Ok(()) => app.push_line("Discord failure notification sent."),
                Err(e) => app.push_line(&format!("Discord notification failed: {e}")),
            }
        }
        if notify && telegram_configured(&app.dir) {
            let err = error.as_deref().unwrap_or("Unknown renewal error");
            match send_telegram_failure(&app.dir, err) {
                Ok(()) => app.push_line("Telegram failure notification sent."),
                Err(e) => app.push_line(&format!("Telegram notification failed: {e}")),
            }
        }
    }
    run_check(app).await;
}

fn app_renew_updates(
    groups: Vec<AppView>,
    lines: &[String],
    selected: Option<&str>,
    at: u64,
) -> serde_json::Map<String, Value> {
    let renewed: std::collections::HashSet<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("RENEWED "))
        .collect();
    let missing: std::collections::HashSet<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("MISSING_APP_ID "))
        .collect();
    let attempted: std::collections::HashSet<&str> = lines
        .iter()
        .filter_map(|line| line.trim().split_once(": expires in "))
        .filter(|(_, tail)| tail.contains("downloading a new profile"))
        .map(|(bundle, _)| bundle)
        .collect();
    let mut updates = serde_json::Map::new();
    for group in groups {
        if selected.is_some_and(|s| s != group.bundle) {
            continue;
        }
        let in_group = group
            .members
            .iter()
            .filter(|m| renewed.contains(m.bundle.as_str()))
            .count();
        let any_attempted = group
            .members
            .iter()
            .any(|m| attempted.contains(m.bundle.as_str()) || missing.contains(m.bundle.as_str()));
        if in_group == 0 && !any_attempted && selected.is_none() {
            continue;
        }
        let status = if in_group == group.members.len() {
            "success"
        } else if in_group > 0 {
            "partial"
        } else {
            "failed"
        };
        updates.insert(
            group.bundle,
            json!({"status":status,"at":at,"renewed":in_group,
            "total":group.members.len()}),
        );
    }
    updates
}

fn save_app_renew_status(app: &App, lines: &[String], selected: Option<&str>) {
    let groups = app.lock().snapshot.apps.clone();
    let mut existing = std::fs::read_to_string(app.dir.join(APP_STATUS_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(&s).ok())
        .unwrap_or_default();
    existing.extend(app_renew_updates(groups, lines, selected, now()));
    if std::fs::create_dir_all(app.dir.join("state")).is_ok() {
        if let Ok(bytes) = serde_json::to_vec(&existing) {
            let _ = crate::fsperm::write_private(&app.dir.join(APP_STATUS_FILE), &bytes);
        }
    }
}

async fn run_remove_profiles(app: Arc<App>, uuids: Vec<String>) {
    let count = uuids.len();
    let result: Result<(), String> = async {
        let team = apple::read_account(&app.dir)
            .and_then(|a| a.team_id)
            .ok_or("Sign in to select a team before removing profiles.")?;
        let (_, mut link) =
            phone::connect_auto_in(&app.dir, app.phone, PAIRING_FILE, |_: &str| {}).await?;
        for (i, uuid) in uuids.iter().enumerate() {
            let p = crate::remove_profile_with_backup(&app.dir, &mut link, uuid, Some(&team)).await?;
            let line = format!(
                "[{}/{}] Removed profile for {} [{}]. Backup saved in {}.",
                i + 1, count, p.bundle_id.as_deref().unwrap_or("?"), p.uuid, crate::SAVED_DIR
            );
            crate::log::line(&format!("[profile] {line}"));
            app.push_line(&line);
        }
        Ok(())
    }
    .await;
    let ok = result.is_ok();
    if let Err(e) = result {
        let msg = short_error(&e);
        crate::log::err(&format!("[profile] Removal failed: {msg}"));
        app.push_line(&format!("Error: {msg}"));
    }
    {
        let mut g = app.lock();
        g.job.running = false;
        g.job.ok = Some(ok);
    }
    run_check(app).await;
}

async fn run_install(
    app: Arc<App>,
    upload: UploadedIpa,
    selected_extensions: Option<Vec<String>>,
    identity: Option<NewInstallIdentity>,
    enable_jit: bool,
) {
    let result = install_ipa(
        &app,
        &upload,
        selected_extensions.as_deref(),
        identity.as_ref(),
        enable_jit,
    )
    .await;
    let ok = match result {
        Ok(None) => {
            app.push_line("Installation completed.");
            true
        }
        Ok(Some(error)) => {
            app.push_line("Installation completed, but JIT could not be enabled.");
            app.push_line(&format!("JIT error: {}", short_error(&error)));
            false
        }
        Err(error) => {
            app.push_line(&format!("Error: {}", short_error(&error)));
            false
        }
    };
    {
        let mut g = app.lock();
        g.job.running = false;
        g.job.ok = Some(ok);
    }
    run_check(app).await;
}

/// Fail closed when a customized install could overwrite an existing app:
/// iPhones can report either the original ID or the Apple-team-suffixed ID.
fn install_identity_taken(
    installed: &std::collections::HashSet<String>,
    raw_id: &str,
    signed_id: &str,
) -> bool {
    installed.contains(raw_id) || installed.contains(signed_id)
}

async fn install_ipa(
    app: &Arc<App>,
    upload: &UploadedIpa,
    selected_extensions: Option<&[String]>,
    identity: Option<&NewInstallIdentity>,
    enable_jit: bool,
) -> Result<Option<String>, String> {
    let account = apple::load_account(&app.dir)?;
    let mut session = apple::open_session_web(
        &app.dir,
        &account.apple_id,
        &account.password,
        app.tfa.clone(),
    )
    .await?;
    let team = apple::pick_team(&mut session, account.team_id.as_deref(), false).await?;
    let expected_ids = identity.map(|id| {
        let raw = id
            .bundle_id
            .as_deref()
            .unwrap_or(&id.source_bundle_id)
            .to_string();
        let signed = format!("{raw}.{}", team.team_id);
        (raw, signed)
    });

    app.push_line("Connecting to the iPhone to read its device identity…");

    let (device_name, udid, _version) = {
        let _work = crate::altserver::phone_lock();

        let (_, mut link) =
            phone::connect_auto_in(&app.dir, app.phone, PAIRING_FILE, |_: &str| {}).await?;
        if let Some((raw, signed)) = expected_ids.as_ref() {
            let installed = phone::with_timeout(
                "Installed app lookup",
                std::time::Duration::from_secs(20),
                link.installed_app_ids(),
            )
            .await?;
            if install_identity_taken(&installed, raw, signed) {
                return Err(format!("Cannot customize an installed app ({signed}). Choose a different Bundle ID for a separate new install."));
            }
        }
        link.device_identity()
            .await
            .map_err(|e| format!("Cannot query iPhone identity: {e}"))?
    };

    app.push_line("Signing the IPA with the configured Apple ID…");
    let signed_app = apple::sign_ipa(
        session,
        &account,
        team,
        &device_name,
        &udid,
        &upload.0,
        selected_extensions,
        identity.and_then(|i| i.display_name.as_deref()),
        identity.and_then(|i| i.bundle_id.as_deref()),
    )
    .await?;

    let _work = crate::altserver::phone_lock();
    let (via, mut link) =
        phone::connect_auto_in(&app.dir, app.phone, PAIRING_FILE, |_: &str| {}).await?;
    // Check once more immediately before installation to avoid replacing an app
    // that appeared while the IPA was signing.
    if let Some((raw, signed)) = expected_ids.as_ref() {
        let installed = phone::with_timeout(
            "Installed app lookup",
            std::time::Duration::from_secs(20),
            link.installed_app_ids(),
        )
        .await?;
        if install_identity_taken(&installed, raw, signed) {
            return Err(format!(
                "Custom install would replace an existing app ({signed}); blocked."
            ));
        }
    }
    app.push_line(&if via == phone::Via::Usb {
        "Installing the signed app over USB…".to_string()
    } else if crate::install_archive::use_archive_on_wifi() {
        "Packing the signed app into an IPA and installing over Wi-Fi…".to_string()
    } else {
        "Installing the signed app over Wi-Fi (legacy directory mode)…".to_string()
    });
    // Il telefono manda decine di notifiche con la stessa percentuale: una riga ogni 10%.
    let last_logged = std::sync::atomic::AtomicI64::new(-1);
    let started = std::time::Instant::now();
    let progress = |percent: u64| {
        if let Some(step) = progress_step(percent, last_logged.load(Ordering::Relaxed)) {
            last_logged.store(step, Ordering::Relaxed);
            app.push_line(&format!(
                "Installation progress: {step}% ({}s)",
                started.elapsed().as_secs()
            ));
        }
    };
    let install = link.install_app(&signed_app.path, &progress);

    phone::with_timeout("Installation", INSTALL_TIMEOUT, install).await?;
    if !enable_jit {
        return Ok(None);
    }

    app.push_line("Launching the installed app to enable JIT…");
    let jit_result = async {
        let bundle_id = crate::install_archive::signed_bundle_id(&signed_app.path)?;
        let pid = phone::with_timeout(
            "Launching app for JIT",
            Duration::from_secs(30),
            link.launch_app(&bundle_id),
        )
        .await?;
        phone::with_timeout(
            "Enabling JIT",
            Duration::from_secs(30),
            link.enable_jit(pid),
        )
        .await
    }
    .await;
    Ok(jit_result.err())
}

/// Tempo massimo per l'installazione di un'app (upload AFC + installazione).
const INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// La percentuale da scrivere nel log, o `None` se non è ancora cambiata di 10 punti
/// dall'ultima scritta (`last`, -1 se nessuna). Il 100% si scrive sempre, una volta sola.
fn progress_step(percent: u64, last: i64) -> Option<i64> {
    let p = percent.min(100) as i64;
    let step = if p >= 100 { 100 } else { p / 10 * 10 };
    (step > last).then_some(step)
}

async fn run_login(app: Arc<App>, apple_id: String, password: String, save: bool) {
    let dir = &app.dir;
    let outcome: Result<String, String> = async {
        let mut sess = apple::open_session_web(dir, &apple_id, &password, app.tfa.clone()).await?;
        let team = apple::pick_team(&mut sess, None, false).await?;
        apple::save_account(
            dir,
            &apple::Account {
                apple_id: apple_id.clone(),
                password: if save {
                    password.clone()
                } else {
                    String::new()
                },
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
        Err(e) => LoginView {
            phase: "failed",
            message: short_apple_error(&e),
        },
    };
    // I dati del telefono possono cambiare in base al team: il prossimo controllo li rifà.
    g.snapshot.checked_at = None;
}

// ---------------------------------------------------------------------------------------------
// Pezzi puri (testati sotto)
// ---------------------------------------------------------------------------------------------

fn label_of(bundle: &str, team: &str) -> String {
    profile::profile_app_id(bundle, team).to_string()
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

/// Display one host app with all its latest extension profiles nested below.
fn app_views(profiles: &[profile::PhoneProfile], team: &str) -> Vec<AppView> {
    let mine = profiles
        .iter()
        .filter(|p| p.team_id.as_deref() == Some(team));
    let latest = profile::latest_by_bundle(mine);
    let bundles: Vec<&str> = latest
        .iter()
        .filter_map(|p| p.bundle_id.as_deref())
        .collect();
    let mut groups: std::collections::BTreeMap<String, Vec<MemberView>> =
        std::collections::BTreeMap::new();
    for p in &latest {
        let Some(bundle) = p.bundle_id.as_deref() else {
            continue;
        };
        let root = profile::group_root(bundle, &bundles, team).to_owned();
        groups.entry(root).or_default().push(MemberView {
            bundle: bundle.to_owned(),
            label: label_of(bundle, team),
            days: p.days_left().unwrap_or(-1),
            expires: p.expires_text(),
        });
    }
    let mut views = Vec::new();
    for (bundle, mut members) in groups {
        members.sort_by(|a, b| a.bundle.cmp(&b.bundle));
        let earliest = members.iter().min_by_key(|m| m.days).unwrap();
        views.push(AppView {
            label: label_of(&bundle, team),
            bundle,
            days: earliest.days,
            expires: earliest.expires.clone(),
            level: level_for(earliest.days),
            members,
        });
    }
    views.sort_by(|a, b| a.days.cmp(&b.days).then_with(|| a.label.cmp(&b.label)));
    views
}

fn profile_views(profiles: &[profile::PhoneProfile], team: &str) -> Vec<ProfileView> {
    let bundles: Vec<&str> = profiles
        .iter()
        .filter(|p| p.team_id.as_deref() == Some(team))
        .filter_map(|p| p.bundle_id.as_deref())
        .collect();
    let mut views: Vec<_> = profiles
        .iter()
        .filter(|p| p.team_id.as_deref() == Some(team))
        .filter_map(|p| {
            let bundle = p.bundle_id.clone()?;
            let root = profile::group_root(&bundle, &bundles, team).to_owned();
            Some(ProfileView {
                uuid: p.uuid.clone(),
                label: label_of(&bundle, team),
                bundle,
                root,
                days: p.days_left().unwrap_or(-1),
                expires: p.expires_text(),
            })
        })
        .collect();
    views.sort_by(|a, b| {
        a.root
            .cmp(&b.root)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| b.expires.cmp(&a.expires))
    });
    views
}

/// Traduce gli errori tecnici in una frase comprensibile.
fn friendly_error(err: &str) -> String {
    let e = err.to_lowercase();
    if e.contains("manca il pairing") {
        "iPhone pairing is missing. Set it up once over USB (see the guide).".into()
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
    // Signing reports can start with an empty heading and put the actual
    // certificate/IPA/App ID cause below it. Do not discard those lines.
    let trimmed = err.trim_start();
    if let Some(detail) = trimmed
        .strip_prefix("firma dell'app non riuscita:")
        .or_else(|| trimmed.strip_prefix("App signing failed:"))
    {
        let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        return format!("App signing failed: {detail}")
            .chars()
            .take(1200)
            .collect();
    }
    if let Some(detail) = trimmed
        .strip_prefix("login Apple ID non riuscito:")
        .or_else(|| trimmed.strip_prefix("Apple ID sign-in failed:"))
    {
        let e = err.to_lowercase();
        if e.contains("-20101") || e.contains("entered incorrectly") {
            return "Incorrect Apple ID or password.".into();
        }
        if e.contains("429") || e.contains("too many requests") {
            return "Apple is rate-limiting requests. Wait a while, then try once.".into();
        }
        if e.contains("aborted") || e.contains("abort") {
            return "Sign-in canceled: the code did not arrive in time.".into();
        }
        if let Some(last_bullet) = err
            .lines()
            .filter(|l| l.contains('●'))
            .last()
            .map(|l| l.trim().trim_start_matches('●').trim())
            .filter(|l| !l.is_empty())
        {
            return format!("Apple ID sign-in failed: {last_bullet}");
        }
        let clean = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        if !clean.is_empty() {
            return format!("Apple ID sign-in failed: {clean}");
        }
    }
    // iOS reports its limit of three apps per iPhone with a free developer profile as a
    // multi-line message: the `{( ... )}` block lists the apps using up the slots. Keep it,
    // otherwise the log stops at `{(` and hides the very apps the user has to delete.
    if err.contains("maximum number of installed apps using a free developer profile") {
        let detail = err.split_whitespace().collect::<Vec<_>>().join(" ");
        let detail: String = detail.chars().take(1000).collect();
        return format!(
            "{detail} | iOS allows at most 3 apps signed with a free developer profile on one \
             iPhone (expired and offloaded apps count too). The limit is on the iPhone, not on \
             the Apple ID, so another account does not help: delete one of the apps listed \
             above, then try again."
        );
    }
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
        return format!(
            "USB connection failed ({}); trying Wi-Fi...",
            short_error(detail)
        );
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
        return "Apple is rate-limiting requests. Wait a while, then try once.".into();
    }
    if e.contains("aborted") || e.contains("abort") {
        return "Sign-in canceled: the code did not arrive in time.".into();
    }
    let last = err
        .lines()
        .filter(|l| l.contains('●'))
        .last()
        .map(|l| l.trim().trim_start_matches('●').trim().to_string());
    last.unwrap_or_else(|| short_error(err))
        .chars()
        .take(240)
        .collect()
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
                .find_map(|l| {
                    l.strip_prefix("esito:")
                        .or_else(|| l.strip_prefix("result:"))
                })
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
            } else if lines.iter().any(|l| {
                l.contains("Niente da rinnovare")
                    || l.contains("non serve rinnovare")
                    || l.contains("Nothing to renew")
                    || l.contains("no renewal needed")
            }) {
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

fn append_log(dir: &Path, lines: &[String], ok: bool) {
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(RENEW_LOG))
    else {
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
    #[tokio::test]
    async fn api_routes_each_device_to_its_own_account_and_state() {
        let root = tempfile::tempdir().unwrap();
        let mut registered = devices::load(root.path()).unwrap();
        let other = devices::add(root.path(), &mut registered, "Work phone").unwrap();
        let default_dir = registered[0].directory(root.path());
        std::fs::write(
            default_dir.join("account.json"),
            br#"{"apple_id":"personal@example.test","password":"one","team_id":"A"}"#,
        )
        .unwrap();
        std::fs::write(
            other.directory(root.path()).join("account.json"),
            br#"{"apple_id":"work@example.test","password":"two","team_id":"B"}"#,
        )
        .unwrap();
        let runtimes = registered
            .iter()
            .map(|device| {
                (
                    device.id.clone(),
                    create_device_runtime(root.path(), device.clone(), None),
                )
            })
            .collect();
        let manager = Arc::new(DeviceManager {
            root: root.path().to_path_buf(),
            pin: None,
            profiles: tokio::sync::RwLock::new(runtimes),
            devices: tokio::sync::RwLock::new(registered.clone()),
            active_id: Mutex::new("default".into()),
        });
        let router = device_manager_router(manager);

        for (id, expected) in [
            ("default", "personal@example.test"),
            (other.id.as_str(), "work@example.test"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/state")
                        .header(header::HOST, "localhost")
                        .header("x-altkeeper-device", id)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["account"]["apple_id"], expected);
        }
    }

    #[tokio::test]
    async fn api_logs_returns_new_lines() {
        let mine = |v: &Value| -> Vec<String> {
            v["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|l| l.as_str())
                .filter(|l| l.contains("logs-api-test"))
                .map(String::from)
                .collect()
        };
        let (before, _) = crate::log::since(u64::MAX);
        crate::log::line("logs-api-test a");
        crate::log::line("logs-api-test b");
        let Json(v) = logs(format!("/api/logs?since={before}").parse().unwrap()).await;
        let got = mine(&v);
        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("] logs-api-test a"));
        let seq = v["seq"].as_u64().unwrap();
        let Json(v) = logs(format!("/api/logs?since={seq}").parse().unwrap()).await;
        assert!(mine(&v).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_auto_renewal_is_visible_and_cancelable_from_state_api() {
        let app = Arc::new(App {
            dir: PathBuf::from("."),
            device_id: "default".into(),
            device_name: "Default iPhone".into(),
            phone: None,
            busy: AtomicBool::new(true),
            inner: Mutex::new(Inner::default()),
            tfa: Arc::new(apple::TfaChannel::default()),
            auto_renew: Mutex::new(AutoRenewConfig::default()),
            pending_ipa: Mutex::new(None),
            renewal_wait: crate::renewal_wait::Control::default(),
        });
        {
            let mut g = app.lock();
            g.job.kind = "auto_renew".into();
            g.job.running = true;
        }
        let waiting = crate::renewal_wait::await_install(
            || std::future::pending::<Result<(), crate::renewal_wait::AttemptError>>(),
            || std::future::pending::<crate::renewal_wait::Probe>(),
            &app.renewal_wait,
            crate::renewal_wait::Policy {
                grace: Duration::ZERO,
                poll: Duration::from_secs(15),
                max_wait: Duration::from_secs(3600),
            },
            |_| {},
        );
        let cancel_from_ui = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Json(snapshot) = state(State(app.clone())).await;
            assert_eq!(snapshot["job"]["waiting_for_phone"], true);
            assert_eq!(snapshot["job"]["cancel_requested"], false);
            assert_eq!(
                cancel_renew_wait(State(app.clone())).await.0,
                StatusCode::OK
            );
        };
        let (result, ()) = tokio::join!(waiting, cancel_from_ui);
        assert_eq!(result, Err(crate::renewal_wait::CANCELLED.into()));
        assert!(!app.renewal_wait.is_waiting());
    }

    #[test]
    fn signing_error_keeps_cause_after_an_empty_first_line() {
        let cause = "Failed to retrieve certificate identity: test cause";
        for prefix in ["firma dell'app non riuscita:", "App signing failed:"] {
            let error = short_error(&format!("{prefix} \r\n    {cause}\n"));
            assert_eq!(error, format!("App signing failed: {cause}"));
        }
        assert_eq!(
            short_error(&format!("App signing failed:\n{}", "x".repeat(2000)))
                .chars()
                .count(),
            1200
        );
    }

    #[test]
    fn free_profile_limit_error_keeps_the_list_of_apps() {
        let raw = "IPA installation failed: Application verification failed: This device has \
                   reached the maximum number of installed apps using a free developer profile: {(\n    \
                   \"TEAM.com.example.one\",\n    \"TEAM.com.example.two\",\n    \
                   \"TEAM.com.example.three\"\n)}";
        let error = short_error(raw);
        assert!(!error.contains('\n'));
        assert!(error.contains("TEAM.com.example.one"));
        assert!(error.contains("TEAM.com.example.three"));
        assert!(error.contains("not on the Apple ID"));
    }

    #[test]
    fn state_does_not_hold_ui_lock_while_waiting_for_schedule() {
        let app = Arc::new(App {
            dir: PathBuf::from("."),
            device_id: "default".into(),
            device_name: "Default iPhone".into(),
            phone: None,
            busy: AtomicBool::new(true),
            inner: Mutex::new(Inner::default()),
            tfa: Arc::new(apple::TfaChannel::default()),
            auto_renew: Mutex::new(AutoRenewConfig::default()),
            pending_ipa: Mutex::new(None),
            renewal_wait: crate::renewal_wait::Control::default(),
        });
        let schedule = app.auto_renew.lock().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reader = app.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            started_tx.send(()).unwrap();
            let _ = rt.block_on(state(State(reader)));
            let _ = done_tx.send(());
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // A state reader waiting for the scheduler must release the UI mutex.
        // Brief contention while copying the snapshot is fine; sustained
        // contention would block retry logging and recreate the deadlock.
        let until = std::time::Instant::now() + Duration::from_millis(200);
        let mut blocked_since = None;
        let mut sustained_block = false;
        while std::time::Instant::now() < until {
            if app.inner.try_lock().is_ok() {
                blocked_since = None;
            } else {
                let since = blocked_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() > Duration::from_millis(50) {
                    sustained_block = true;
                    break;
                }
            }
            std::thread::yield_now();
        }
        // Release before asserting so even the old behavior cannot hang CI.
        drop(schedule);
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            !sustained_block,
            "state held the UI mutex while waiting for the schedule"
        );
    }

    #[test]
    fn missing_ids_update_failed_and_partial_groups_without_tainting_successes() {
        let profiles = vec![
            profile("a", "TEAM", "com.example.a", 2),
            profile("b", "TEAM", "com.example.b", 2),
            profile("c", "TEAM", "com.example.c", 2),
            profile("cx", "TEAM", "com.example.c.TEAM.extension", 2),
            profile("d", "TEAM", "com.example.d", 6),
        ];
        let groups = app_views(&profiles, "TEAM");
        let lines = vec![
            "RENEWED com.example.a".into(),
            "MISSING_APP_ID com.example.b".into(),
            "RENEWED com.example.c".into(),
            "MISSING_APP_ID com.example.c.TEAM.extension".into(),
        ];
        let updates = app_renew_updates(groups, &lines, None, 123);
        assert_eq!(updates["com.example.a"]["status"], "success");
        assert_eq!(updates["com.example.b"]["status"], "failed");
        assert_eq!(updates["com.example.b"]["renewed"], 0);
        assert_eq!(updates["com.example.c"]["status"], "partial");
        assert_eq!(updates["com.example.c"]["total"], 2);
        assert!(!updates.contains_key("com.example.d"));
        let error = crate::renewal_completion(&["com.example.b"], 2).unwrap_err();
        assert!(retryable_renew_error(&error));
    }

    #[test]
    fn custom_identity_accepts_independent_name_or_bundle_and_rejects_invalid_ids() {
        assert!(validated_install_identity(None, None, "com.example.app")
            .unwrap()
            .is_none());
        let renamed =
            validated_install_identity(Some(" New name ".into()), None, "com.example.app")
                .unwrap()
                .unwrap();
        assert_eq!(renamed.display_name.as_deref(), Some("New name"));
        assert!(renamed.bundle_id.is_none());
        let cloned =
            validated_install_identity(None, Some("com.example.clone".into()), "com.example.app")
                .unwrap()
                .unwrap();
        assert_eq!(cloned.bundle_id.as_deref(), Some("com.example.clone"));
        assert!(cloned.display_name.is_none());
        assert!(validated_install_identity(
            None,
            Some("no_spaces allowed".into()),
            "com.example.app"
        )
        .is_err());
        assert!(validated_install_identity(
            None,
            Some("com.example.app".into()),
            "com.example.app"
        )
        .is_err());
        assert!(validated_install_identity(Some("x".repeat(41)), None, "com.example.app").is_err());
    }

    #[test]
    fn custom_install_never_overwrites_installed_raw_or_team_suffixed_ids() {
        let mut installed = std::collections::HashSet::new();
        assert!(!install_identity_taken(
            &installed,
            "com.example.app",
            "com.example.app.TEAM"
        ));
        installed.insert("com.example.app".to_string());
        assert!(install_identity_taken(
            &installed,
            "com.example.app",
            "com.example.app.TEAM"
        ));
        installed.clear();
        installed.insert("com.example.app.TEAM".to_string());
        assert!(install_identity_taken(
            &installed,
            "com.example.app",
            "com.example.app.TEAM"
        ));
    }

    #[test]
    fn auto_renew_utc_time_validation_and_next_run() {
        assert_eq!(parse_utc_time("06:00"), Some(6 * 3600));
        for invalid in ["24:00", "21:60", "9:01", "abc", "21:17:00"] {
            assert_eq!(parse_utc_time(invalid), None);
        }
        // Day 20 in UTC: scheduled for 06:00, 05:00 currently.
        let day = 20_u64;
        let now_secs = day * DAY_SECS + 5 * 3600;
        let mut conf = AutoRenewConfig {
            enabled: true,
            time: "06:00".into(),
            last_attempt_day: None,
            ..AutoRenewConfig::default()
        };
        assert_eq!(
            next_auto_run(&conf, now_secs),
            Some(day * DAY_SECS + 6 * 3600)
        );
        conf.last_attempt_day = Some(day);
        assert_eq!(
            next_auto_run(&conf, now_secs),
            Some((day + 1) * DAY_SECS + 6 * 3600)
        );
        conf.enabled = false;
        assert_eq!(next_auto_run(&conf, now_secs), None);
    }

    #[test]
    fn safari_icons_are_real_pngs() {
        for icon in [APPLE_TOUCH_ICON, FAVICON_32] {
            assert!(icon.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert!(icon.len() > 100);
        }
    }

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
        assert!(!basic_password_ok(
            &headers(&[("authorization", "Bearer 1234")]),
            "1234"
        ));
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
    fn new_install_identity_validation_keeps_name_and_id_independent() {
        assert!(validated_install_identity(None, None, "com.example.app")
            .unwrap()
            .is_none());
        let renamed =
            validated_install_identity(Some("Cloned app".into()), None, "com.example.app")
                .unwrap()
                .unwrap();
        assert_eq!(renamed.display_name.as_deref(), Some("Cloned app"));
        assert!(renamed.bundle_id.is_none());
        assert_eq!(renamed.source_bundle_id, "com.example.app");
        let cloned =
            validated_install_identity(None, Some("com.example.clone".into()), "com.example.app")
                .unwrap()
                .unwrap();
        assert!(cloned.display_name.is_none());
        assert_eq!(cloned.bundle_id.as_deref(), Some("com.example.clone"));
        assert!(validated_install_identity(
            None,
            Some("com.example.app".into()),
            "com.example.app"
        )
        .is_err());
        assert!(
            validated_install_identity(None, Some("bad bundle id".into()), "com.example.app")
                .is_err()
        );
        assert!(validated_install_identity(Some("x".repeat(41)), None, "com.example.app").is_err());
    }

    #[test]
    fn etichette_e_livelli() {
        assert_eq!(
            label_of("com.spotify.client.TEAM", "TEAM"),
            "com.spotify.client"
        );
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
        assert_eq!(v[0].members.len(), 1);
    }

    #[test]
    fn hosts_group_extensions_and_take_earliest_expiration() {
        let p = vec![
            profile("a", "TEAM", "com.kdt.livecontainer", 6),
            profile("b", "TEAM", "com.kdt.livecontainer.TEAM.LiveProcess", 1),
            profile("c", "TEAM", "com.kdt.livecontainer.TEAM.ShareExtension", 5),
            profile("d", "TEAM", "com.google.ios.youtube", 6),
        ];
        let apps = app_views(&p, "TEAM");
        assert_eq!(apps.len(), 2);
        let live = apps
            .iter()
            .find(|a| a.bundle == "com.kdt.livecontainer")
            .unwrap();
        assert_eq!(live.members.len(), 3);
        assert_eq!(live.days, 1);
        let profiles = profile_views(&p, "TEAM");
        assert!(
            profiles
                .iter()
                .filter(|p| p.root == "com.kdt.livecontainer")
                .count()
                == 3
        );
    }

    #[test]
    fn production_bundle_names_render_as_two_apps_not_six_profiles() {
        let team = "SAVEDTEAM1";
        let all = vec![
            profile("a", team, "com.google.ios.youtube", 6),
            profile(
                "b",
                team,
                "com.google.ios.youtube.PXUL22799Q.OpenYouTube.Extension",
                5,
            ),
            profile("c", team, "com.kdt.livecontainer", 6),
            profile(
                "d",
                team,
                "com.kdt.livecontainer.PXUL22799Q.LaunchAppExtension",
                6,
            ),
            profile("e", team, "com.kdt.livecontainer.PXUL22799Q.LiveProcess", 2),
            profile(
                "f",
                team,
                "com.kdt.livecontainer.PXUL22799Q.ShareExtension",
                6,
            ),
        ];
        let apps = app_views(&all, team);
        assert_eq!(apps.len(), 2);
        let youtube = apps
            .iter()
            .find(|a| a.bundle == "com.google.ios.youtube")
            .unwrap();
        assert_eq!(youtube.members.len(), 2);
        assert_eq!(youtube.days, 5);
        let live = apps
            .iter()
            .find(|a| a.bundle == "com.kdt.livecontainer")
            .unwrap();
        assert_eq!(live.members.len(), 4);
        assert_eq!(live.days, 2);
        let views = profile_views(&all, team);
        assert_eq!(views.iter().filter(|p| p.root == youtube.bundle).count(), 2);
        assert_eq!(views.iter().filter(|p| p.root == live.bundle).count(), 4);
    }

    #[test]
    fn real_raw_profile_ids_from_phone_display_as_two_grouped_apps() {
        // Screenshots show these IDs after the UI has hidden the last
        // ".PXUL22799Q". The phone's raw provisioning IDs contain that
        // extra suffix; testing display-only names missed the regression.
        let team = "PXUL22799Q";
        let all = vec![
            profile("a", team, "com.google.ios.youtube.PXUL22799Q", 6),
            profile(
                "b",
                team,
                "com.google.ios.youtube.PXUL22799Q.OpenYouTube.Extension.PXUL22799Q",
                6,
            ),
            profile("c", team, "com.kdt.livecontainer.PXUL22799Q", 6),
            profile(
                "d",
                team,
                "com.kdt.livecontainer.PXUL22799Q.LaunchAppExtension.PXUL22799Q",
                6,
            ),
            profile(
                "e",
                team,
                "com.kdt.livecontainer.PXUL22799Q.LiveProcess.PXUL22799Q",
                6,
            ),
            profile(
                "f",
                team,
                "com.kdt.livecontainer.PXUL22799Q.ShareExtension.PXUL22799Q",
                6,
            ),
        ];
        let apps = app_views(&all, team);
        assert_eq!(apps.len(), 2, "UI must show two apps, not six profiles");
        let youtube = apps
            .iter()
            .find(|a| a.label == "com.google.ios.youtube")
            .unwrap();
        assert_eq!(youtube.members.len(), 2);
        assert_eq!(youtube.bundle, "com.google.ios.youtube.PXUL22799Q");
        let live = apps
            .iter()
            .find(|a| a.label == "com.kdt.livecontainer")
            .unwrap();
        assert_eq!(live.members.len(), 4);
        assert_eq!(live.bundle, "com.kdt.livecontainer.PXUL22799Q");
        let views = profile_views(&all, team);
        assert_eq!(views.iter().filter(|p| p.root == youtube.bundle).count(), 2);
        assert_eq!(views.iter().filter(|p| p.root == live.bundle).count(), 4);
    }

    #[test]
    fn errori_in_parole_semplici() {
        assert!(
            friendly_error("manca il pairing remoto rp-pairing.plist: crealo").contains("missing")
        );
        assert!(
            friendly_error("pairing con l'iPhone non riuscito (Socket(... early eof ...))")
                .contains("no longer recognizes")
        );
        assert!(friendly_error(
            "Os { code: 113, kind: HostUnreachable, message: \"No route to host\" }"
        )
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
        assert!(
            short_apple_error("HTTP status client error (429 Too Many Requests)")
                .contains("rate-limiting")
        );
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
        assert!(h[1]["summary"]
            .as_str()
            .unwrap()
            .contains("cannot find the iPhone"));
        assert_eq!(h[2]["summary"], "Nothing to renew");
        assert_eq!(parse_history(log, 1).len(), 1);
        assert!(parse_history("", 5).is_empty());
        assert!(parse_history("righe senza intestazione\n", 5).is_empty());
    }
}


#[derive(Deserialize)]
struct AutostartUpdate {
    enabled: bool,
}

async fn update_autostart(
    State(app): State<Arc<App>>,
    Json(payload): Json<AutostartUpdate>,
) -> Response {
    #[cfg(any(windows, target_os = "macos"))]
    {
        if let Err(e) = crate::tray::toggle_autostart(payload.enabled, &app.dir) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
        Json(json!({
            "supported": true,
            "enabled": crate::tray::is_autostart_enabled(),
        }))
        .into_response()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (app, payload);
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "Autostart is not supported on this platform" })),
        )
            .into_response()
    }
}
