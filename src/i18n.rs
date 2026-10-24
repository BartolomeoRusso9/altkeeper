//! Selezione della lingua per i messaggi rivolti all'utente.
//!
//! ALTKEEPER_LANG (o l'alias legacy ALTREFRESH_LANG): "it" o "en", con
//! eventuali suffissi regionali. Per compatibilità il default resta italiano.
//! Il protocollo AltStore, i JSON, gli ID e i nomi dei comandi non si traducono.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Italian,
    English,
}

pub fn language_from(value: Option<&str>) -> Language {
    let value = value.unwrap_or("").trim();
    let base = value.split(['-', '_']).next().unwrap_or("");
    if base.eq_ignore_ascii_case("en") {
        Language::English
    } else {
        Language::Italian
    }
}

pub fn language() -> Language {
    language_from(crate::env::var("LANG").as_deref())
}

pub fn is_english() -> bool {
    language() == Language::English
}

/// Pair di messaggi, valutato solo nella lingua selezionata. La selezione è
/// centralizzata; gli argomenti di format! restano controllati dal compilatore.
///
/// Per le frasi con segnaposto usare:
/// crate::tr!(format!("Profilo {name}"), format!("Profile {name}"))
#[macro_export]
macro_rules! tr {
    ($it:expr, $en:expr $(,)?) => {{
        if $crate::i18n::is_english() {
            $en
        } else {
            $it
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::{language_from, Language};

    #[test]
    fn default_e_valori_italiani() {
        for raw in [None, Some(""), Some("it"), Some("IT"), Some("it-IT"), Some("it_it")] {
            assert_eq!(language_from(raw), Language::Italian);
        }
    }

    #[test]
    fn opzioni_inglesi() {
        for raw in [Some("en"), Some("EN"), Some("en-US"), Some("en_GB"), Some(" en-us ")] {
            assert_eq!(language_from(raw), Language::English);
        }
    }

    #[test]
    fn valori_sconosciuti_non_attivano_inglese() {
        for raw in [Some("english"), Some("enough"), Some("ru"), Some("de-DE"), Some("xx_en")] {
            assert_eq!(language_from(raw), Language::Italian);
        }
    }
}
