//! Lettura delle risposte interattive da una console reale.
//!
//! Su Windows (soprattutto PowerShell) lo stdin di un processo nativo può essere
//! una pipe chiusa, mentre la console collegata è ancora interattiva. CONIN$
//! permette di leggere direttamente dalla console, come fa rpassword per la
//! password dell'Apple ID. In assenza di una console (stdin reindirizzato),
//! ricadiamo su stdin senza entrare in un ciclo infinito su EOF.

use std::io::{self, BufRead, Write};

/// Mostra una domanda e legge una singola risposta (senza newline).
pub fn prompt_line(prompt: &str) -> io::Result<String> {
    {
        // Release stdout before blocking for input. Other tasks may need to log.
        let mut out = io::stdout().lock();
        out.write_all(prompt.as_bytes())?;
        out.flush()?;
    }
    read_console_line()
}

fn read_console_line() -> io::Result<String> {
    #[cfg(windows)]
    {
        // Evita lo stdin ereditato da PowerShell; la console resta disponibile
        // anche quando il processo figlio vede stdin come pipe o EOF.
        if let Ok(console) = std::fs::File::open("CONIN$") {
            return read_line_from(io::BufReader::new(console));
        }
    }
    read_line_from(io::stdin().lock())
}

fn read_line_from(mut reader: impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            crate::tr!("nessun input disponibile dal terminale", "no terminal input available"),
        ));
    }
    Ok(line.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::read_line_from;
    use std::io::{Cursor, ErrorKind};

    #[test]
    fn legge_codice_numerico_con_crlf_windows() {
        assert_eq!(read_line_from(Cursor::new(" 012345\r\n")).unwrap(), "012345");
    }

    #[test]
    fn legge_comandi_e_scelte_senza_newline() {
        assert_eq!(read_line_from(Cursor::new("p1\n")).unwrap(), "p1");
        assert_eq!(read_line_from(Cursor::new(" N \r\n")).unwrap(), "N");
    }

    #[test]
    fn eof_e_errore_e_non_una_risposta_vuota() {
        assert_eq!(
            read_line_from(Cursor::new("")).unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn riga_vuota_non_e_eof() {
        assert_eq!(read_line_from(Cursor::new("\r\n")).unwrap(), "");
    }
}
