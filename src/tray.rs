//! Icona nella barra di sistema (Windows) o nella barra dei menu (macOS) e finestra con la pagina
//! web. Comando `altkeeper tray`: AltServer e la pagina girano come con `serve --altserver`, senza
//! terminale. Chiudere la finestra la nasconde soltanto; il programma si ferma solo con "Quit".

use std::io::{self, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
#[cfg(windows)]
use tao::platform::windows::EventLoopBuilderExtWindows;
use tao::window::{Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
#[cfg(windows)]
use winreg::enums::HKEY_CURRENT_USER;
#[cfg(windows)]
use winreg::RegKey;
use wry::{WebView, WebViewBuilder};

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Su Windows il nome del valore nella chiave Run, su macOS l'etichetta del LaunchAgent.
#[cfg(windows)]
const RUN_VALUE: &str = "AltKeeper";
#[cfg(target_os = "macos")]
const RUN_VALUE: &str = "io.github.altkeeper";
const AUTOSTART_MARKER: &str = "autostart-initialised";

#[cfg(windows)]
const AUTOSTART_LABEL: &str = "Start with Windows";
#[cfg(target_os = "macos")]
const AUTOSTART_LABEL: &str = "Start at login";

pub enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    ServerFailed(String),
    Status {
        reachable: bool,
        tip: String,
    },
    Updates(Option<crate::update::Release>),
    AutoPaired,
    PairingDone(Result<(), String>),
    #[cfg_attr(not(windows), allow(dead_code))]
    Restart,
    #[cfg_attr(not(windows), allow(dead_code))]
    UpdateFailed(String),
}

static TRAY_PROXY: std::sync::OnceLock<EventLoopProxy<UserEvent>> = std::sync::OnceLock::new();

pub fn notify_auto_pair() {
    if let Some(proxy) = TRAY_PROXY.get() {
        let _ = proxy.send_event(UserEvent::AutoPaired);
    }
}

/// Aspetta (al massimo 20 s) che il processo `pid` finisca: dopo un aggiornamento la versione
/// vecchia deve liberare le porte prima che la nuova le apra.
#[cfg(windows)]
pub fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            if WaitForSingleObject(h, 20_000) != 0 {
                crate::log::err("[update] the previous version is still running after 20 s");
            }
            CloseHandle(h);
        }
    }
}

/// Controlla le release; `manual` = chiesto dal menu, quindi si risponde anche quando non c'è
/// niente di nuovo.
fn check_updates(proxy: &EventLoopProxy<UserEvent>, manual: bool) {
    let latest = crate::update::check();
    if let Err(e) = &latest {
        crate::log::err(&format!("[update] {e}"));
    }
    if manual {
        match &latest {
            Ok(None) => toast(&format!(
                "AltKeeper {} is up to date.",
                env!("CARGO_PKG_VERSION")
            )),
            Err(e) => toast(&format!("Update check failed: {e}")),
            Ok(Some(_)) => {}
        }
    }
    let latest = latest.ok().flatten();
    let _ = proxy.send_event(UserEvent::Updates(latest));
}

/// Stacca il processo dal terminale: lanciato con un doppio clic, la finestra nera si chiude.
#[cfg(windows)]
pub fn hide_console() {
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
}

#[cfg(windows)]
pub fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide("AltKeeper"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Esegue uno script AppleScript passandogli `text` come argomento: niente da mettere tra
/// virgolette, qualsiasi carattere arriva così com'è.
#[cfg(target_os = "macos")]
fn osascript(script: &str, text: &str) {
    let _ = std::process::Command::new("osascript")
        .args(["-e", "on run argv", "-e", script, "-e", "end run", text])
        .output();
}

#[cfg(target_os = "macos")]
pub fn message_box(text: &str) {
    osascript(
        r#"display alert "AltKeeper" message (item 1 of argv) as critical"#,
        text,
    );
}

/// Logo di AltKeeper (assets/logo.png) già ridotto in RGBA grezzo: a colori quando il telefono
/// è raggiungibile, in grigio quando non lo è.
const LOGO_32: &[u8] = include_bytes!("../assets/logo-32.rgba");
const LOGO_GREY_32: &[u8] = include_bytes!("../assets/logo-grey-32.rgba");
const LOGO_64: &[u8] = include_bytes!("../assets/logo-64.rgba");

pub fn logo(reachable: bool) -> Icon {
    let rgba = if reachable { LOGO_32 } else { LOGO_GREY_32 };
    Icon::from_rgba(rgba.to_vec(), 32, 32).expect("valid icon")
}

#[cfg(windows)]
fn run_key() -> io::Result<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(RUN_KEY)
        .map(|(k, _)| k)
}

#[cfg(windows)]
pub fn is_autostart_enabled() -> bool {
    autostart_enabled_as(RUN_VALUE)
}

#[cfg(windows)]
pub fn toggle_autostart(on: bool, dir: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    set_autostart_as(RUN_VALUE, on, &exe, dir)
}

#[cfg(target_os = "macos")]
pub fn is_autostart_enabled() -> bool {
    autostart_enabled_as(RUN_VALUE)
}

#[cfg(target_os = "macos")]
pub fn toggle_autostart(on: bool, dir: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    set_autostart_as(RUN_VALUE, on, &exe, dir)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn is_autostart_enabled() -> bool {
    false
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn toggle_autostart(_on: bool, _dir: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Not supported on this platform"))
}

#[cfg(windows)]
pub fn autostart_command(exe: &Path, dir: &Path) -> String {
    format!("\"{}\" tray --dir \"{}\" --minimized", exe.display(), dir.display())
}

#[cfg(windows)]
fn autostart_enabled_as(name: &str) -> bool {
    run_key()
        .and_then(|k| k.get_value::<String, _>(name))
        .is_ok()
}

#[cfg(windows)]
fn set_autostart_as(name: &str, on: bool, exe: &Path, dir: &Path) -> io::Result<()> {
    let key = run_key()?;
    if on {
        key.set_value(name, &autostart_command(exe, dir))
    } else {
        match key.delete_value(name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

/// Cartella dei LaunchAgent dell'utente: macOS avvia al login quelli che trova qui.
#[cfg(target_os = "macos")]
fn agents_dir() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join(format!("altkeeper-agents-{}", std::process::id()));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/LaunchAgents")
}

#[cfg(target_os = "macos")]
fn agent_path(name: &str) -> PathBuf {
    agents_dir().join(format!("{name}.plist"))
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Il LaunchAgent che al login avvia `exe tray --dir dir`.
#[cfg(target_os = "macos")]
pub fn launch_agent(name: &str, exe: &Path, dir: &Path) -> String {
    let arg = |s: &str| format!("    <string>{}</string>\n", xml_escape(s));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key>\n  <string>{}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n{}{}{}{}{}  </array>\n\
         \x20 <key>RunAtLoad</key>\n  <true/>\n\
         \x20 <key>ProcessType</key>\n  <string>Interactive</string>\n\
         </dict>\n</plist>\n",
        xml_escape(name),
        arg(&exe.to_string_lossy()),
        arg("tray"),
        arg("--dir"),
        arg(&dir.to_string_lossy()),
        arg("--minimized"),
    )
}

/// Un'app aperta direttamente da Download (o dall'immagine disco) gira da una copia temporanea
/// in sola lettura ("App Translocation"): un avvio al login che puntasse lì non funzionerebbe.
#[cfg(target_os = "macos")]
fn translocated(exe: &Path) -> bool {
    exe.to_string_lossy().contains("/AppTranslocation/")
}

#[cfg(target_os = "macos")]
fn autostart_enabled_as(name: &str) -> bool {
    agent_path(name).exists()
}

/// Il LaunchAgent vale dal prossimo login: non lo si carica subito, altrimenti partirebbe una
/// seconda copia del programma.
#[cfg(target_os = "macos")]
fn set_autostart_as(name: &str, on: bool, exe: &Path, dir: &Path) -> io::Result<()> {
    let path = agent_path(name);
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        };
    }
    if translocated(exe) {
        return Err(io::Error::other(
            "move AltKeeper.app to the Applications folder first",
        ));
    }
    std::fs::create_dir_all(agents_dir())?;
    std::fs::write(&path, launch_agent(name, exe, dir))
}

/// Alla prima esecuzione attiva l'avvio automatico; dopo rispetta la scelta fatta dal menu.
/// Se è attivo riscrive il comando, così vale anche se il programma è stato spostato.
fn ensure_autostart_default_as(name: &str, state_dir: &Path, exe: &Path, dir: &Path) {
    let marker = state_dir.join(AUTOSTART_MARKER);
    if !marker.exists() {
        // Da una copia temporanea non si decide niente: ci si riprova quando l'app è al suo posto.
        if set_autostart_as(name, true, exe, dir).is_err() {
            return;
        }
        let _ = std::fs::create_dir_all(state_dir);
        let _ = std::fs::write(&marker, "");
    } else if autostart_enabled_as(name) {
        let _ = set_autostart_as(name, true, exe, dir);
    }
}

/// Testo del tooltip: quante app e la scadenza più vicina (`days` = giorni rimasti per app).
pub fn summary(days: &[i64]) -> String {
    let Some(&min) = days.iter().min() else {
        return "AltKeeper: no apps".into();
    };
    let n = days.len();
    let apps = if n == 1 {
        "1 app".to_string()
    } else {
        format!("{n} apps")
    };
    let next = match min {
        ..0 => "one has expired".to_string(),
        0 => "one expires today".to_string(),
        1 => "next expiry in 1 day".to_string(),
        d => format!("next expiry in {d} days"),
    };
    format!("AltKeeper: {apps}, {next}")
}

#[derive(Debug, PartialEq)]
pub enum Note {
    Renewed(Vec<String>),
    Installed,
    Failed(String),
}

/// Dalle righe di log, gli avvisi da mostrare. I profili installati di seguito diventano un
/// avviso solo.
pub fn notes(lines: &[String]) -> Vec<Note> {
    let mut out = Vec::new();
    for line in lines {
        let msg = line.split_once("] ").map_or(line.as_str(), |(_, m)| m);
        // Le righe di AltServer sono nella lingua della CLI (ALTKEEPER_LANG): si riconoscono entrambe.
        if let Some(bundle) = msg
            .strip_prefix("[altserver] profile installed: ")
            .or_else(|| msg.strip_prefix("[altserver] profilo installato: "))
        {
            if let Some(Note::Renewed(v)) = out.last_mut() {
                v.push(bundle.to_string());
            } else {
                out.push(Note::Renewed(vec![bundle.to_string()]));
            }
        } else if matches!(
            msg,
            "[altserver] app installed"
                | "[altserver] app installata"
                | "[web] Installation completed."
        ) {
            out.push(Note::Installed);
        } else if let Some(e) = msg
            .strip_prefix("[altserver] installation failed: ")
            .or_else(|| msg.strip_prefix("[altserver] installazione non riuscita: "))
            .or_else(|| msg.strip_prefix("[web] Error: "))
        {
            out.push(Note::Failed(e.to_string()));
        }
    }
    out
}

/// Al massimo un avviso di errore ogni 10 minuti (AltStore riprova da solo).
#[derive(Default)]
pub struct FailGate {
    last: Option<Instant>,
}

impl FailGate {
    pub fn allow(&mut self, now: Instant) -> bool {
        let ok = self
            .last
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(600));
        if ok {
            self.last = Some(now);
        }
        ok
    }
}

#[cfg(windows)]
fn toast(text: &str) {
    use tauri_winrt_notification::Toast;
    let _ = Toast::new(Toast::POWERSHELL_APP_ID)
        .title("AltKeeper")
        .text1(text)
        .show();
}

#[cfg(target_os = "macos")]
fn toast(text: &str) {
    osascript(
        r#"display notification (item 1 of argv) with title "AltKeeper""#,
        text,
    );
}

/// Apre un indirizzo nel browser predefinito.
#[cfg(target_os = "macos")]
pub fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

#[cfg(windows)]
pub fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn open_in_browser(_url: &str) {}

#[cfg(windows)]
pub fn activate_existing_instance() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let title = wide("AltKeeper");
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
pub fn activate_existing_instance() -> bool {
    false
}

/// Guarda le righe di log nuove e mostra gli avvisi.
fn watch_log() {
    let mut seq = crate::log::since(u64::MAX).0;
    let mut gate = FailGate::default();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let (latest, lines) = crate::log::since(seq);
        seq = latest;
        for note in notes(&lines) {
            match note {
                Note::Renewed(b) => toast(&format!("Profiles installed: {}", b.join(", "))),
                Note::Installed => toast("App installed on the iPhone."),
                Note::Failed(e) if gate.allow(Instant::now()) => toast(&format!("Failed: {e}")),
                Note::Failed(_) => {}
            }
        }
    }
}

/// GET minimale verso la pagina locale; `None` se non risponde.
pub fn http_get(addr: SocketAddr, path: &str) -> Option<String> {
    let query_addr = if addr.ip().is_unspecified() {
        SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), addr.port())
    } else {
        addr
    };
    let mut s = TcpStream::connect_timeout(&query_addr, Duration::from_secs(5)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok()?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: {query_addr}\r\n\r\n").ok()?;
    let mut body = String::new();
    s.read_to_string(&mut body).ok()?;
    body.split_once("\r\n\r\n").map(|(_, b)| b.to_string())
}

/// Ogni 5 minuti legge lo stato dalla pagina (che riguarda il telefono se il dato è vecchio)
/// e aggiorna colore e tooltip dell'icona.
fn watch_state(addr: SocketAddr, proxy: EventLoopProxy<UserEvent>) {
    std::thread::sleep(Duration::from_secs(5));
    for round in 0.. {
        if let Some(v) = http_get(addr, "/api/state")
            .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        {
            let reachable = v["phone"]["reachable"].as_bool() == Some(true);
            let days: Vec<i64> = v["apps"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x["days"].as_i64()).collect())
                .unwrap_or_default();
            let _ = proxy.send_event(UserEvent::Status {
                reachable,
                tip: summary(&days),
            });
        }
        // Il primo controllo del telefono parte con la prima richiesta: si riguarda presto.
        std::thread::sleep(Duration::from_secs(if round == 0 { 60 } else { 300 }));
    }
}

fn show(
    slot: &mut Option<(Window, WebView)>,
    target: &EventLoopWindowTarget<UserEvent>,
    url: &str,
) {
    if let Some((window, webview)) = slot {
        if url.contains('#') {
            let _ = webview.load_url(url);
        }
        window.set_visible(true);
        window.set_minimized(false);
        window.set_focus();
        return;
    }
    let target_width = 1040.0;
    let target_height = 840.0;
    let mut builder = WindowBuilder::new()
        .with_title("AltKeeper")
        .with_inner_size(tao::dpi::LogicalSize::new(target_width, target_height))
        .with_min_inner_size(tao::dpi::LogicalSize::new(420.0, 560.0))
        .with_window_icon(tao::window::Icon::from_rgba(LOGO_64.to_vec(), 64, 64).ok());

    if let Some(monitor) = target.primary_monitor() {
        let screen_size = monitor.size();
        let screen_pos = monitor.position();
        let scale = monitor.scale_factor();
        let win_width_px = (target_width * scale) as i32;
        let win_height_px = (target_height * scale) as i32;

        let center_x = screen_pos.x + ((screen_size.width as i32 - win_width_px) / 2).max(0);
        let center_y = screen_pos.y + ((screen_size.height as i32 - win_height_px) / 2).max(0);
        // Ensure safety margin from the top to prevent appearing under top-docked taskbars
        let min_y = screen_pos.y + (65.0 * scale) as i32;
        let safe_y = center_y.max(min_y);

        builder = builder.with_position(tao::dpi::PhysicalPosition::new(center_x, safe_y));
    }

    let window = match builder.build(target)
    {
        Ok(w) => w,
        Err(e) => return crate::log::err(&format!("[tray] cannot open the window: {e}")),
    };
    match WebViewBuilder::new().with_url(url).with_hotkeys_zoom(false).build(&window) {
        Ok(webview) => {
            // Su macOS l'app non ha un'icona nel Dock: la finestra va portata davanti a mano.
            window.set_focus();
            *slot = Some((window, webview))
        }
        Err(e) => {
            let msg = if cfg!(windows) {
                format!("Cannot show the page (is Microsoft Edge WebView2 installed?): {e}")
            } else {
                format!("Cannot show the page: {e}")
            };
            crate::log::err(&format!("[tray] {msg}"));
            message_box(&format!("{msg}\n\nThe page is also at {url}"));
        }
    }
}

/// Su macOS le scorciatoie (⌘C, ⌘V, ⌘A, ⌘W...) passano dal menu dell'applicazione, anche quando
/// non si vede: senza, nella finestra non si potrebbe incollare la password dell'Apple ID.
#[cfg(target_os = "macos")]
fn app_menu() -> Menu {
    use tray_icon::menu::Submenu;
    let app = Submenu::with_items(
        "AltKeeper",
        true,
        &[
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::close_window(None),
        ],
    );
    let edit = Submenu::with_items(
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ],
    );
    let menu = Menu::new();
    if let (Ok(app), Ok(edit)) = (app, edit) {
        let _ = menu.append_items(&[&app, &edit]);
    }
    menu
}

/// Crea l'icona. All'avvio con Windows l'area di notifica può non essere ancora pronta: si
/// riprova per un minuto prima di arrendersi.
fn build_tray(menu: &Menu) -> TrayIcon {
    let build = || {
        TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            // Su macOS un clic sull'icona della barra dei menu apre il menu, come AltServer.
            .with_menu_on_left_click(cfg!(target_os = "macos"))
            .with_tooltip("AltKeeper")
            .with_icon(logo(false))
            .build()
    };
    (0..30)
        .find_map(|_| {
            build()
                .map_err(|_| std::thread::sleep(Duration::from_secs(2)))
                .ok()
        })
        .unwrap_or_else(|| {
            message_box("AltKeeper cannot create its tray icon. Start it again.");
            std::process::exit(1)
        })
}

/// Avvia il ciclo degli eventi con l'icona; `start` riceve il proxy per segnalare errori del
/// server. Non ritorna: "Quit" chiude il processo. Su macOS va chiamata dal thread principale.
pub fn run(bind: SocketAddr, dir: &Path, minimized: bool, start: impl FnOnce(EventLoopProxy<UserEvent>)) -> ! {
    let client_addr = if bind.ip().is_unspecified() {
        SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), bind.port())
    } else {
        bind
    };
    let url = format!("http://{client_addr}/");
    let mut builder = EventLoopBuilder::<UserEvent>::with_user_event();
    // Il ciclo degli eventi gira sul thread `altkeeper-main` (main() sposta tutto lì per avere uno
    // stack più grande), e tao su Windows rifiuta di crearlo fuori dal main thread se non lo si
    // permette. Finestra, icona e ciclo vivono tutti su questo stesso thread, quindi è sicuro.
    #[cfg(windows)]
    builder.with_any_thread(true);
    #[allow(unused_mut)]
    let mut event_loop = builder.build();
    // Solo l'icona nella barra dei menu, niente Dock né ⌘-Tab: come AltServer.
    #[cfg(target_os = "macos")]
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();
    let _ = TRAY_PROXY.set(proxy.clone());
    let p = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Tray(e));
    }));
    let p = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Menu(e));
    }));

    let exe = std::env::current_exe().unwrap_or_default();
    ensure_autostart_default_as(RUN_VALUE, &dir.join("state"), &exe, dir);
    #[cfg(target_os = "macos")]
    if translocated(&exe) {
        toast("Move AltKeeper.app to the Applications folder to start it at login.");
    }

    let open = MenuItem::new("Open AltKeeper", true, None);
    let logs = MenuItem::new("Show logs", true, None);
    let autostart =
        CheckMenuItem::new(AUTOSTART_LABEL, true, autostart_enabled_as(RUN_VALUE), None);
    let pair_usb = MenuItem::new("Pair iPhone over USB", true, None);
    let anisette_choice = crate::apple::anisette_choice(dir);
    let anisette_locked = crate::env::var("ANISETTE_URL").is_some()
        || anisette_choice.as_ref().is_ok_and(|choice| choice.locked);
    let anisette_url = anisette_choice
        .as_ref()
        .map(|choice| choice.url.as_str())
        .unwrap_or("");
    if let Err(error) = &anisette_choice {
        crate::log::err(&format!("[tray] cannot read anisette server: {error}"));
    }
    let ani_local = CheckMenuItem::new(
        "AltKeeper local server (127.0.0.1:8788)",
        !anisette_locked,
        anisette_url == crate::apple::LOCAL_ANISETTE_URL,
        None,
    );
    let ani_default = CheckMenuItem::new(
        "ani3.stikstore.app (default)",
        !anisette_locked,
        anisette_url == crate::apple::DEFAULT_ANISETTE_URL,
        None,
    );
    let ani_isideload = CheckMenuItem::new(
        "isideload public server",
        !anisette_locked,
        anisette_url == crate::apple::ISIDELOAD_ANISETTE_URL,
        None,
    );
    let ani_other = MenuItem::new("Other…", !anisette_locked, None);
    let anisette_menu = Submenu::new("Anisette server", true);
    let _ = anisette_menu.append_items(&[&ani_local, &ani_default, &ani_isideload, &ani_other]);
    let check = MenuItem::new("Check for updates", true, None);
    let update_now = MenuItem::new("Up to date", false, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &open,
        &logs,
        &PredefinedMenuItem::separator(),
        &pair_usb,
        &anisette_menu,
        &PredefinedMenuItem::separator(),
        &autostart,
        &check,
        &update_now,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);
    #[cfg(target_os = "macos")]
    let app_menu = app_menu();
    // L'icona si crea quando il ciclo degli eventi è già partito: su macOS prima non comparirebbe.
    let mut tray: Option<TrayIcon> = None;

    std::thread::spawn(watch_log);
    crate::phone::start_auto_pair_monitor();
    let p = proxy.clone();
    std::thread::spawn(move || watch_state(bind, p));
    let p = proxy.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        loop {
            check_updates(&p, false);
            std::thread::sleep(Duration::from_secs(24 * 3600));
        }
    });
    let check_proxy = proxy.clone();
    let mut pending: Option<crate::update::Release> = None;
    let mut staging = false;
    let mut announced = String::new();
    start(proxy);

    let dir = dir.to_path_buf();
    let mut slot: Option<(Window, WebView)> = None;
    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                #[cfg(target_os = "macos")]
                app_menu.init_for_nsapp();
                tray = Some(build_tray(&menu));
                if !minimized {
                    show(&mut slot, target, &url);
                }
            }
            #[cfg(windows)]
            Event::UserEvent(UserEvent::Tray(
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                }
                | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                },
            )) => show(&mut slot, target, &url),
            #[cfg(not(windows))]
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            })) => show(&mut slot, target, &url),
            Event::UserEvent(UserEvent::Menu(e)) => {
                if e.id == open.id() {
                    show(&mut slot, target, &url);
                } else if e.id == logs.id() {
                    show(&mut slot, target, &format!("{url}#logs"));
                } else if e.id == pair_usb.id() {
                    if Path::new(crate::PAIRING_FILE).exists() {
                        show(&mut slot, target, &format!("{url}#pairing"));
                    } else {
                        let p = check_proxy.clone();
                        std::thread::spawn(move || {
                            let result = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(|error| error.to_string())
                                .and_then(|runtime| {
                                    runtime.block_on(crate::phone::pair_usb(crate::PAIRING_FILE))
                                });
                            let _ = p.send_event(UserEvent::PairingDone(result));
                        });
                    }
                } else if e.id == ani_local.id() || e.id == ani_default.id() || e.id == ani_isideload.id() {
                    let selected = if e.id == ani_default.id() {
                        None
                    } else if e.id == ani_local.id() {
                        Some(crate::apple::LOCAL_ANISETTE_URL)
                    } else {
                        Some(crate::apple::ISIDELOAD_ANISETTE_URL)
                    };
                    let selected_url = selected.unwrap_or(crate::apple::DEFAULT_ANISETTE_URL);
                    match crate::apple::save_anisette_choice(&dir, selected) {
                        Ok(_) => {
                            let _ = ani_local.set_checked(selected_url == crate::apple::LOCAL_ANISETTE_URL);
                            let _ = ani_default.set_checked(selected_url == crate::apple::DEFAULT_ANISETTE_URL);
                            let _ = ani_isideload.set_checked(selected_url == crate::apple::ISIDELOAD_ANISETTE_URL);
                            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                                handle.spawn(crate::altserver::clear_anisette_cache());
                            }
                        }
                        Err(error) => crate::log::err(&format!("[tray] cannot change anisette server: {error}")),
                    }
                } else if e.id == ani_other.id() {
                    show(&mut slot, target, &format!("{url}#anisette"));
                } else if e.id == autostart.id() {
                    let on = autostart.is_checked();
                    if let Err(err) = set_autostart_as(RUN_VALUE, on, &exe, &dir) {
                        crate::log::err(&format!("[tray] cannot change {AUTOSTART_LABEL}: {err}"));
                        #[cfg(target_os = "macos")]
                        toast(&format!("Cannot change {AUTOSTART_LABEL}: {err}"));
                        autostart.set_checked(!on);
                    }
                } else if e.id == check.id() {
                    let p = check_proxy.clone();
                    std::thread::spawn(move || check_updates(&p, true));
                } else if e.id == update_now.id() && !staging {
                    // Su macOS l'app firmata non si sostituisce da sola: si apre la pagina della
                    // release, da cui si scarica il nuovo AltKeeper.app.
                    #[cfg(target_os = "macos")]
                    if let Some(r) = &pending {
                        open_in_browser(&r.page);
                    }
                    #[cfg(windows)]
                    if let Some(r) = pending.clone() {
                        staging = true;
                        update_now.set_enabled(false);
                        update_now.set_text("Downloading the update...");
                        let (p, exe) = (check_proxy.clone(), exe.clone());
                        std::thread::spawn(move || {
                            let _ = p.send_event(match crate::update::stage(&r, &exe) {
                                Ok(()) => UserEvent::Restart,
                                Err(e) => UserEvent::UpdateFailed(e),
                            });
                        });
                    }
                } else if e.id == quit.id() {
                    crate::altserver::stop();
                    std::process::exit(0);
                }
            }
            Event::UserEvent(UserEvent::Status { reachable, tip }) => {
                if let Some(tray) = &tray {
                    let _ = tray.set_icon(Some(logo(reachable)));
                    let _ = tray.set_tooltip(Some(tip));
                }
            }
            Event::UserEvent(UserEvent::AutoPaired) => toast("iPhone paired over USB."),
            Event::UserEvent(UserEvent::PairingDone(result)) => match result {
                Ok(()) => toast("iPhone paired over USB."),
                Err(error) => {
                    crate::log::err(&format!("[tray] USB pairing failed: {error}"));
                    toast(&format!("USB pairing failed: {error}"));
                }
            },
            Event::UserEvent(UserEvent::Updates(latest)) if !staging => {
                if let Some(r) = &latest {
                    if cfg!(windows) {
                        update_now.set_text(format!("Update now to {}", r.tag));
                    } else {
                        update_now.set_text(format!("Download {}", r.tag));
                    }
                    update_now.set_enabled(true);
                    if announced != r.tag {
                        toast(&if cfg!(windows) {
                            format!(
                                "{} is available. Right-click the tray icon and choose Update now.",
                                r.tag
                            )
                        } else {
                            format!(
                                "{} is available. Click the AltKeeper icon in the menu bar and choose Download.",
                                r.tag
                            )
                        });
                        announced = r.tag.clone();
                    }
                }
                pending = latest.or(pending.take());
            }
            Event::UserEvent(UserEvent::Restart) => {
                crate::log::line("[update] restarting with the new version");
                crate::altserver::stop();
                let spawned = std::process::Command::new(&exe)
                    .arg("tray")
                    .arg("--dir")
                    .arg(&dir)
                    .arg("--replacing")
                    .arg(std::process::id().to_string())
                    .spawn();
                match spawned {
                    Ok(_) => std::process::exit(0),
                    // AltServer è già fermo: meglio chiudere in modo visibile che restare
                    // aperti senza servire AltStore.
                    Err(e) => {
                        crate::update::rollback(&exe);
                        crate::log::err(&format!("[update] cannot start the new version: {e}"));
                        message_box(&format!(
                            "The update could not start ({e}). The previous version was kept.\n\n\
                             Start AltKeeper again."
                        ));
                        std::process::exit(1);
                    }
                }
            }
            Event::UserEvent(UserEvent::UpdateFailed(e)) => {
                staging = false;
                crate::log::err(&format!("[update] {e}"));
                toast(&format!("Update failed: {e}"));
                update_now.set_text("Update now (try again)");
                update_now.set_enabled(true);
            }
            Event::UserEvent(UserEvent::ServerFailed(msg)) => {
                crate::log::err(&msg);
                message_box(&format!("AltKeeper cannot start:\n\n{msg}"));
                std::process::exit(1);
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if let Some((window, _)) = &slot {
                    window.set_visible(false);
                }
            }
            _ => {}
        }
    })
}

/// Cartella dei dati di `tray` su macOS quando non si indica `--dir`: l'interno di
/// AltKeeper.app non va toccato (romperebbe la firma) e può essere in sola lettura.
#[cfg(target_os = "macos")]
pub fn default_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join("Library/Application Support/AltKeeper"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_text() {
        assert_eq!(summary(&[]), "AltKeeper: no apps");
        assert_eq!(summary(&[1]), "AltKeeper: 1 app, next expiry in 1 day");
        assert_eq!(
            summary(&[6, 4, 5]),
            "AltKeeper: 3 apps, next expiry in 4 days"
        );
        assert_eq!(summary(&[0, 3]), "AltKeeper: 2 apps, one expires today");
        assert_eq!(summary(&[-2]), "AltKeeper: 1 app, one has expired");
    }

    #[test]
    fn notes_from_log_lines() {
        let lines: Vec<String> = [
            "[2026-10-25 10:00:00] [altserver] profile installed: com.a.T",
            "[2026-10-25 10:00:01] [altserver] profile installed: com.b.T",
            "[2026-10-25 10:00:02] [altserver] connected",
            "[2026-10-25 10:00:03] [altserver] app installed",
            "[2026-10-25 10:00:04] [altserver] installation failed: Socket(Kind(UnexpectedEof))",
            "[2026-10-25 10:00:05] [web] Installation completed.",
            "[2026-10-25 10:00:06] [web] Error: Apple ID sign-in failed: x",
            "[2026-10-25 10:00:07] [altserver] profilo installato: com.c.T",
            "[2026-10-25 10:00:08] [altserver] app installata",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            notes(&lines),
            vec![
                Note::Renewed(vec!["com.a.T".into(), "com.b.T".into()]),
                Note::Installed,
                Note::Failed("Socket(Kind(UnexpectedEof))".into()),
                Note::Installed,
                Note::Failed("Apple ID sign-in failed: x".into()),
                Note::Renewed(vec!["com.c.T".into()]),
                Note::Installed,
            ]
        );
    }

    #[test]
    fn failed_toast_rate_limited() {
        let t0 = std::time::Instant::now();
        let mut gate = FailGate::default();
        assert!(gate.allow(t0));
        assert!(!gate.allow(t0 + std::time::Duration::from_secs(60)));
        assert!(gate.allow(t0 + std::time::Duration::from_secs(11 * 60)));
    }

    #[cfg(windows)]
    const TEST_VALUE: &str = "AltKeeper-test";
    #[cfg(target_os = "macos")]
    const TEST_VALUE: &str = "io.github.altkeeper.test";

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("altkeeper-tray-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(windows)]
    #[test]
    fn autostart_command_is_quoted() {
        let cmd = autostart_command(
            Path::new(r"C:\Alt Keeper\altkeeper.exe"),
            Path::new(r"C:\Alt Keeper"),
        );
        assert_eq!(
            cmd,
            r#""C:\Alt Keeper\altkeeper.exe" tray --dir "C:\Alt Keeper" --minimized"#
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_runs_tray_with_escaped_paths() {
        let plist = launch_agent(
            "io.github.altkeeper",
            Path::new("/Applications/Alt & Keeper.app/Contents/MacOS/altkeeper"),
            Path::new("/Users/me/Library/Application Support/AltKeeper"),
        );
        assert!(plist.contains("<string>io.github.altkeeper</string>"));
        assert!(plist.contains(
            "<string>/Applications/Alt &amp; Keeper.app/Contents/MacOS/altkeeper</string>\n    \
             <string>tray</string>\n    <string>--dir</string>\n    \
             <string>/Users/me/Library/Application Support/AltKeeper</string>\n    \
             <string>--minimized</string>"
        ));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        let parsed: plist::Value = plist::from_bytes(plist.as_bytes()).unwrap();
        assert_eq!(
            parsed.as_dictionary().unwrap()["ProgramArguments"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn translocated_app_is_not_started_at_login() {
        // Un nome suo: i test girano in parallelo e `autostart_roundtrip...` attiva TEST_VALUE.
        const NAME: &str = "io.github.altkeeper.test-translocated";
        let exe = Path::new(
            "/private/var/folders/x/AppTranslocation/ABC/d/AltKeeper.app/Contents/MacOS/altkeeper",
        );
        assert!(set_autostart_as(NAME, true, exe, Path::new("/tmp")).is_err());
        assert!(!autostart_enabled_as(NAME));
        let d = temp_dir("translocated");
        ensure_autostart_default_as(NAME, &d.join("state"), exe, &d);
        assert!(!d.join("state").join(AUTOSTART_MARKER).exists());
    }

    #[test]
    fn autostart_roundtrip_and_default_only_once() {
        let d = temp_dir("autostart");
        let exe = d.join("altkeeper.exe");
        let _ = set_autostart_as(TEST_VALUE, false, &exe, &d);

        set_autostart_as(TEST_VALUE, true, &exe, &d).unwrap();
        assert!(autostart_enabled_as(TEST_VALUE));
        set_autostart_as(TEST_VALUE, false, &exe, &d).unwrap();
        assert!(!autostart_enabled_as(TEST_VALUE));

        // prima volta: si attiva
        ensure_autostart_default_as(TEST_VALUE, &d.join("state"), &exe, &d);
        assert!(autostart_enabled_as(TEST_VALUE));
        // tolto dal menu: resta tolto
        set_autostart_as(TEST_VALUE, false, &exe, &d).unwrap();
        ensure_autostart_default_as(TEST_VALUE, &d.join("state"), &exe, &d);
        assert!(!autostart_enabled_as(TEST_VALUE));
    }
}
