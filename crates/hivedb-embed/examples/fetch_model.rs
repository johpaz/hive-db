//! Descarga y verifica el modelo del embedder local en la caché, sin cargarlo.
//!
//! Sirve para preparar una instalación sin red (air-gapped): se ejecuta una vez en una
//! máquina con acceso, se copia el directorio resultante a la máquina aislada y allí se
//! arranca con `HIVEDB_MODEL_DIR=<ruta> HIVEDB_OFFLINE=1`.
//!
//! `HIVEDB_MODEL_DIR=./modelo cargo run --release -p hivedb-embed --example fetch_model`

use hivedb_embed::ensure_multilingual_e5_small;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = ensure_multilingual_e5_small()?;
    println!("modelo listo en {}", files.dir().display());
    println!("space_id: {}", files.space_id());
    for entry in std::fs::read_dir(files.dir())? {
        let entry = entry?;
        let bytes = std::fs::metadata(entry.path())?.len();
        println!("  {} ({bytes} bytes)", entry.file_name().to_string_lossy());
    }
    Ok(())
}
