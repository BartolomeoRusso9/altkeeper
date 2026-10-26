// Righe di log del server (AltServer, pagina web, DNS-SD): ognuna con data e ora, scritta sul
// terminale, in coda a `altkeeper.log` e in memoria per la scheda Logs della pagina.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const FILE_NAME: &str = "altkeeper.log";
const MAX_FILE: u64 = 5 * 1024 * 1024;
const MAX_LINES: usize = 2000;

struct Sink {
    path: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    max_file: u64,
    ring: VecDeque<(u64, String)>,
    seq: u64,
}

impl Sink {
    const fn new(max_file: u64) -> Self {
        Sink { path: None, file: None, size: 0, max_file, ring: VecDeque::new(), seq: 0 }
    }

    fn open(&mut self, path: PathBuf) {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        self.file = options.open(&path).ok();
        // Il log può contenere dati sensibili: se non lo si riesce a proteggere, non lo si scrive.
        if self.file.is_some() && crate::fsperm::restrict_file(&path).is_err() {
            self.file = None;
        }
        self.size = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        self.path = Some(path);
    }

    fn record(&mut self, line: &str) {
        self.seq += 1;
        self.ring.push_back((self.seq, line.to_string()));
        if self.ring.len() > MAX_LINES {
            self.ring.pop_front();
        }
        let len = line.len() as u64 + 1;
        if self.file.is_some() && self.size + len > self.max_file {
            self.rotate();
        }
        if let Some(f) = self.file.as_mut() {
            if writeln!(f, "{line}").is_ok() {
                self.size += len;
            }
        }
    }

    // Il file pieno diventa `altkeeper.log.1` (quello vecchio si perde) e se ne apre uno nuovo.
    fn rotate(&mut self) {
        let Some(path) = self.path.clone() else { return };
        self.file = None;
        let old = path.with_extension("log.1");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, &old);
        self.open(path);
    }

    fn since(&self, seq: u64) -> (u64, Vec<String>) {
        let lines = self.ring.iter().filter(|(s, _)| *s > seq).map(|(_, l)| l.clone()).collect();
        (self.seq, lines)
    }
}

static SINK: Mutex<Sink> = Mutex::new(Sink::new(MAX_FILE));

fn sink() -> std::sync::MutexGuard<'static, Sink> {
    SINK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Da qui in poi le righe vanno anche in `dir/altkeeper.log`.
pub fn init(dir: &Path) {
    sink().open(dir.join(FILE_NAME));
}

pub fn stamp(now: chrono::DateTime<chrono::Local>, msg: &str) -> String {
    format!("[{}] {msg}", now.format("%Y-%m-%d %H:%M:%S"))
}

pub fn line(msg: &str) {
    let l = stamp(chrono::Local::now(), msg);
    println!("{l}");
    sink().record(&l);
}

// La usano solo il tray e gli aggiornamenti (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn err(msg: &str) {
    let l = stamp(chrono::Local::now(), msg);
    eprintln!("{l}");
    sink().record(&l);
}

/// Le righe dopo `seq`, e il numero dell'ultima.
pub fn since(seq: u64) -> (u64, Vec<String>) {
    sink().since(seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("altkeeper-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stamp_format() {
        let t = chrono::Local.with_ymd_and_hms(2026, 10, 25, 14, 3, 22).unwrap();
        assert_eq!(stamp(t, "x"), "[2026-10-25 14:03:22] x");
    }

    #[test]
    fn ring_keeps_last_2000() {
        let mut s = Sink::new(MAX_FILE);
        for i in 1..=2005 {
            s.record(&format!("line {i}"));
        }
        let (seq, lines) = s.since(0);
        assert_eq!(seq, 2005);
        assert_eq!(lines.len(), 2000);
        assert_eq!(lines[0], "line 6");
    }

    #[test]
    fn since_returns_only_new() {
        let mut s = Sink::new(MAX_FILE);
        s.record("a");
        let (seq, _) = s.since(0);
        s.record("b");
        assert_eq!(s.since(seq), (2, vec!["b".to_string()]));
        assert_eq!(s.since(2), (2, vec![]));
    }

    #[test]
    fn rotates_when_full() {
        let d = temp_dir("rotate");
        let mut s = Sink::new(100);
        s.open(d.join(FILE_NAME));
        for _ in 0..10 {
            s.record("0123456789012345678901234567890123456789");
        }
        assert!(d.join("altkeeper.log.1").exists());
        assert!(std::fs::metadata(d.join(FILE_NAME)).unwrap().len() <= 100);
    }

    #[test]
    fn log_survives_unwritable_file() {
        let d = temp_dir("unwritable");
        let mut s = Sink::new(MAX_FILE);
        s.open(d.clone()); // una cartella non si apre come file
        s.record("still fine");
        assert_eq!(s.since(0).1, vec!["still fine".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn log_files_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = temp_dir("private");
        let mut s = Sink::new(100);
        s.open(d.join(FILE_NAME));
        for _ in 0..10 {
            s.record("0123456789012345678901234567890123456789");
        }
        for path in [d.join(FILE_NAME), d.join("altkeeper.log.1")] {
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
