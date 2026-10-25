//! Su Windows incorpora il logo (assets/altkeeper.ico) nell'exe: è l'icona che si vede in
//! Esplora file, sul desktop e nella barra delle applicazioni. Sugli altri sistemi non fa niente.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/altkeeper.ico");

    #[cfg(windows)]
    {
        // Compilare da Windows verso un altro sistema non richiede nessuna risorsa.
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/altkeeper.ico");
        res.set("ProductName", "AltKeeper");
        res.set("FileDescription", "AltKeeper");
        if let Err(e) = res.compile() {
            // L'icona è solo estetica: se manca il compilatore di risorse (rc.exe) si va avanti.
            println!("cargo:warning=icon not embedded in altkeeper.exe: {e}");
        }
    }
}
