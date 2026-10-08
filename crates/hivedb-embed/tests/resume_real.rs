//! La reanudación de la descarga contra Hugging Face de verdad: el servidor real redirige al
//! CDN, y ahí `Range` tiene que seguir funcionando. Va en su propio fichero (su propio proceso)
//! porque cambia `HIVEDB_MODEL_DIR`, que es global.
//!
//! Necesita red y el modelo ya en la caché de `HIVEDB_MODEL_DIR` (de ahí se toman los bytes
//! de partida), y no se ejecuta con `cargo test` a secas:
//! `cargo test -p hivedb-embed --test resume_real -- --ignored`

use std::path::{Path, PathBuf};

const REVISION_DIR: &str = "multilingual-e5-small-614241f6";

fn hardlink_or_copy(from: &Path, to: &Path) {
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).unwrap();
    }
}

#[test]
#[ignore = "requiere red y el modelo en la caché de HIVEDB_MODEL_DIR"]
fn reanuda_la_descarga_real_a_traves_de_la_redireccion_al_cdn() {
    let cache = PathBuf::from(
        std::env::var_os("HIVEDB_MODEL_DIR").expect("HIVEDB_MODEL_DIR con el modelo ya descargado"),
    )
    .join(REVISION_DIR);
    let full = std::fs::read(cache.join("tokenizer.json")).expect("tokenizer.json en la caché");

    // Caché nueva con todo menos tokenizer.json, y un parcial de sus primeros 5 MB.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(REVISION_DIR);
    std::fs::create_dir_all(&target).unwrap();
    for name in ["config.json", "model.safetensors"] {
        hardlink_or_copy(&cache.join(name), &target.join(name));
    }
    let prefix = 5_000_000;
    std::fs::write(target.join("tokenizer.json.part"), &full[..prefix]).unwrap();

    // SAFETY: este fichero tiene una sola prueba y aún no hay otros hilos que lean el entorno.
    unsafe {
        std::env::set_var("HIVEDB_MODEL_DIR", dir.path());
        std::env::remove_var("HIVEDB_OFFLINE");
    }

    let mut first_seen = None;
    let files = hivedb_embed::ensure_multilingual_e5_small_with(&mut |progress| {
        first_seen.get_or_insert(progress.downloaded);
    })
    .expect("la descarga reanudada debe completarse");

    assert!(!files.was_cached());
    // Se reanudó: el primer aviso ya trae los 5 MB que estaban en disco.
    assert_eq!(first_seen, Some(prefix as u64));
    // Y el archivo (verificado por SHA-256 al terminar) es idéntico al original.
    assert_eq!(std::fs::read(target.join("tokenizer.json")).unwrap(), full);
}
