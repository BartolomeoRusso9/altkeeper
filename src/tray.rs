//! Icona nella barra di sistema e finestra con la pagina web (solo Windows). Comando
//! `altkeeper tray`: AltServer e la pagina girano come con `serve --altserver`, senza terminale.
//! Chiudere la finestra la nasconde soltanto; il programma si ferma solo con "Quit".

use std::io;
use std::path::Path;

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

pub fn dot(rgb: [u8; 3]) -> Icon {
    Icon::from_rgba(dot_rgba(rgb), 32, 32).expect("valid icon")
}

pub const GREEN: [u8; 3] = [52, 168, 83];

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
        .with_window_icon(tao::window::Icon::from_rgba(dot_rgba(GREEN), 32, 32).ok())
        .build(target)
    {
        Ok(w) => w,
        Err(e) => return crate::log::err(&format!("[tray] cannot open the window: {e}")),
    };
    match WebViewBuilder::new().with_url(url).build(&window) {
        Ok(webview) => *slot = Some((window, webview)),
        Err(e) => crate::log::err(&format!(
            "[tray] cannot show the page (is Microsoft Edge WebView2 installed?): {e}"
        )),
    }
}

/// Pixel RGBA di un pallino pieno 32x32 del colore dato.
fn dot_rgba(rgb: [u8; 3]) -> Vec<u8> {
    const N: i32 = 32;
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x * 2 - N + 1, y * 2 - N + 1);
            let inside = dx * dx + dy * dy <= (N - 2) * (N - 2);
            px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], if inside { 255 } else { 0 }]);
        }
    }
    px
}

/// Avvia il ciclo degli eventi con l'icona; `start` riceve il proxy per segnalare errori del
/// server. Non ritorna: "Quit" chiude il processo.
pub fn run(url: String, dir: &Path, start: impl FnOnce(EventLoopProxy<UserEvent>)) -> ! {
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
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &open,
        &logs,
        &PredefinedMenuItem::separator(),
        &autostart,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("AltKeeperPlus")
        .with_icon(dot(GREEN))
        .build()
        .expect("tray icon");

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
                } else if e.id == quit.id() {
                    crate::altserver::stop();
                    std::process::exit(0);
                }
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
