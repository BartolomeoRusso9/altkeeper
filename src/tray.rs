//! Icona nella barra di sistema e finestra con la pagina web (solo Windows). Comando
//! `altkeeper tray`: AltServer e la pagina girano come con `serve --altserver`, senza terminale.
//! Chiudere la finestra la nasconde soltanto; il programma si ferma solo con "Quit".

use std::io::{self, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIconBuilder, TrayIconEvent};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;
use wry::{WebView, WebViewBuilder};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "AltKeeperPlus";
const AUTOSTART_MARKER: &str = "autostart-initialised";

pub enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    ServerFailed(String),
    Status { reachable: bool, tip: String },
    Updates(Option<crate::update::Release>),
    Restart,
    UpdateFailed(String),
}

/// Aspetta (al massimo 20 s) che il processo `pid` finisca: dopo un aggiornamento la versione
/// vecchia deve liberare le porte prima che la nuova le apra.
pub fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
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
            Ok(None) => toast(&format!("AltKeeperPlus {} is up to date.", env!("CARGO_PKG_VERSION"))),
            Err(e) => toast(&format!("Update check failed: {e}")),
            Ok(Some(_)) => {}
        }
    }
    let latest = latest.ok().flatten();
    let _ = proxy.send_event(UserEvent::Updates(latest));
}

/// Stacca il processo dal terminale: lanciato con un doppio clic, la finestra nera si chiude.
pub fn hide_console() {
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
}

pub fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide("AltKeeperPlus"));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
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

fn run_key() -> io::Result<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY).map(|(k, _)| k)
}

pub fn autostart_command(exe: &Path, dir: &Path) -> String {
    format!("\"{}\" tray --dir \"{}\"", exe.display(), dir.display())
}

fn autostart_enabled_as(name: &str) -> bool {
    run_key().and_then(|k| k.get_value::<String, _>(name)).is_ok()
}

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

/// Alla prima esecuzione attiva l'avvio con Windows; dopo rispetta la scelta fatta dal menu.
/// Se è attivo riscrive il comando, così vale anche se l'exe è stato spostato.
fn ensure_autostart_default_as(name: &str, state_dir: &Path, exe: &Path, dir: &Path) {
    let marker = state_dir.join(AUTOSTART_MARKER);
    if !marker.exists() {
        let _ = set_autostart_as(name, true, exe, dir);
        let _ = std::fs::create_dir_all(state_dir);
        let _ = std::fs::write(&marker, "");
    } else if autostart_enabled_as(name) {
        let _ = set_autostart_as(name, true, exe, dir);
    }
}

/// Testo del tooltip: quante app e la scadenza più vicina (`days` = giorni rimasti per app).
pub fn summary(days: &[i64]) -> String {
    let Some(&min) = days.iter().min() else {
        return "AltKeeperPlus: no apps".into();
    };
    let n = days.len();
    let apps = if n == 1 { "1 app".to_string() } else { format!("{n} apps") };
    let next = match min {
        ..0 => "one has expired".to_string(),
        0 => "one expires today".to_string(),
        1 => "next expiry in 1 day".to_string(),
        d => format!("next expiry in {d} days"),
    };
    format!("AltKeeperPlus: {apps}, {next}")
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
            "[altserver] app installed" | "[altserver] app installata" | "[web] Installation completed."
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
        let ok = self.last.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(600));
        if ok {
            self.last = Some(now);
        }
        ok
    }
}

fn toast(text: &str) {
    use tauri_winrt_notification::Toast;
    let _ = Toast::new(Toast::POWERSHELL_APP_ID).title("AltKeeperPlus").text1(text).show();
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
fn http_get(addr: SocketAddr, path: &str) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(60))).ok()?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").ok()?;
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
            let _ = proxy.send_event(UserEvent::Status { reachable, tip: summary(&days) });
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
    let window = match WindowBuilder::new()
        .with_title("AltKeeperPlus")
        .with_inner_size(tao::dpi::LogicalSize::new(560.0, 860.0))
        .with_window_icon(tao::window::Icon::from_rgba(LOGO_64.to_vec(), 64, 64).ok())
        .build(target)
    {
        Ok(w) => w,
        Err(e) => return crate::log::err(&format!("[tray] cannot open the window: {e}")),
    };
    match WebViewBuilder::new().with_url(url).build(&window) {
        Ok(webview) => *slot = Some((window, webview)),
        Err(e) => {
            let msg = format!("Cannot show the page (is Microsoft Edge WebView2 installed?): {e}");
            crate::log::err(&format!("[tray] {msg}"));
            message_box(&format!("{msg}\n\nThe page is also at {url}"));
        }
    }
}

/// Avvia il ciclo degli eventi con l'icona; `start` riceve il proxy per segnalare errori del
/// server. Non ritorna: "Quit" chiude il processo.
pub fn run(bind: SocketAddr, dir: &Path, start: impl FnOnce(EventLoopProxy<UserEvent>)) -> ! {
    let url = format!("http://{bind}/");
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
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

    let open = MenuItem::new("Open AltKeeper", true, None);
    let logs = MenuItem::new("Show logs", true, None);
    let autostart =
        CheckMenuItem::new("Start with Windows", true, autostart_enabled_as(RUN_VALUE), None);
    let check = MenuItem::new("Check for updates", true, None);
    let update_now = MenuItem::new("Up to date", false, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &open,
        &logs,
        &PredefinedMenuItem::separator(),
        &autostart,
        &check,
        &update_now,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);
    // All'avvio con Windows l'area di notifica può non essere ancora pronta: si riprova per un
    // minuto prima di arrendersi.
    let build = || {
        TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_menu_on_left_click(false)
            .with_tooltip("AltKeeperPlus")
            .with_icon(logo(false))
            .build()
    };
    let tray = (0..30)
        .find_map(|_| {
            build().map_err(|_| std::thread::sleep(Duration::from_secs(2))).ok()
        })
        .unwrap_or_else(|| {
            message_box("AltKeeperPlus cannot create its tray icon. Start it again.");
            std::process::exit(1)
        });

    std::thread::spawn(watch_log);
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
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            })) => show(&mut slot, target, &url),
            Event::UserEvent(UserEvent::Menu(e)) => {
                if e.id == open.id() {
                    show(&mut slot, target, &url);
                } else if e.id == logs.id() {
                    show(&mut slot, target, &format!("{url}#logs"));
                } else if e.id == autostart.id() {
                    let on = autostart.is_checked();
                    if let Err(err) = set_autostart_as(RUN_VALUE, on, &exe, &dir) {
                        crate::log::err(&format!("[tray] cannot change Start with Windows: {err}"));
                        autostart.set_checked(!on);
                    }
                } else if e.id == check.id() {
                    let p = check_proxy.clone();
                    std::thread::spawn(move || check_updates(&p, true));
                } else if e.id == update_now.id() && !staging {
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
                let _ = tray.set_icon(Some(logo(reachable)));
                let _ = tray.set_tooltip(Some(tip));
            }
            Event::UserEvent(UserEvent::Updates(latest)) if !staging => {
                if let Some(r) = &latest {
                    update_now.set_text(format!("Update now to {}", r.tag));
                    update_now.set_enabled(true);
                    if announced != r.tag {
                        toast(&format!(
                            "{} is available. Right-click the tray icon and choose Update now.",
                            r.tag
                        ));
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
                             Start AltKeeperPlus again."
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
                message_box(&format!("AltKeeperPlus cannot start:\n\n{msg}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_text() {
        assert_eq!(summary(&[]), "AltKeeperPlus: no apps");
        assert_eq!(summary(&[1]), "AltKeeperPlus: 1 app, next expiry in 1 day");
        assert_eq!(summary(&[6, 4, 5]), "AltKeeperPlus: 3 apps, next expiry in 4 days");
        assert_eq!(summary(&[0, 3]), "AltKeeperPlus: 2 apps, one expires today");
        assert_eq!(summary(&[-2]), "AltKeeperPlus: 1 app, one has expired");
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

    const TEST_VALUE: &str = "AltKeeperPlus-test";

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("altkeeper-tray-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn autostart_command_is_quoted() {
        let cmd = autostart_command(Path::new(r"C:\Alt Keeper\altkeeper.exe"), Path::new(r"C:\Alt Keeper"));
        assert_eq!(cmd, r#""C:\Alt Keeper\altkeeper.exe" tray --dir "C:\Alt Keeper""#);
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
