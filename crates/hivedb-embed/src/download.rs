//! Descarga y caché de los pesos del modelo.
//!
//! Las URLs apuntan a un commit concreto del repositorio del modelo, y cada
//! archivo se verifica contra un SHA-256 fijado en el código: si el contenido
//! no es exactamente el esperado, no se usa.

use hivedb_index::IndexError;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const REPO: &str = "intfloat/multilingual-e5-small";
const REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";

struct Artifact {
    name: &'static str,
    size: u64,
    sha256: &'static str,
}

const ARTIFACTS: &[Artifact] = &[
    Artifact {
        name: "config.json",
        size: 655,
        sha256: "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
    },
    Artifact {
        name: "tokenizer.json",
        size: 17_082_730,
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    },
    Artifact {
        name: "model.safetensors",
        size: 470_641_600,
        sha256: "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
    },
];

/// Directorio con los archivos del modelo ya verificados.
#[derive(Debug, Clone)]
pub struct ModelFiles {
    dir: PathBuf,
    space_id: String,
}

impl ModelFiles {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Identidad del espacio vectorial: modelo y revisión.
    pub fn space_id(&self) -> &str {
        &self.space_id
    }
}

fn fail(message: String) -> IndexError {
    IndexError::Embedder(message)
}

/// Raíz de la caché: `HIVEDB_MODEL_DIR`, o el directorio de caché del usuario.
pub fn default_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HIVEDB_MODEL_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("hivedb").join("models")
}

fn offline() -> bool {
    std::env::var("HIVEDB_OFFLINE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Garantiza que los archivos de `multilingual-e5-small` están en la caché,
/// descargando los que falten (o tengan un tamaño inesperado).
pub fn ensure_multilingual_e5_small() -> hivedb_index::Result<ModelFiles> {
    let dir = default_cache_dir().join(format!("multilingual-e5-small-{}", &REVISION[..8]));
    std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;

    for artifact in ARTIFACTS {
        let path = dir.join(artifact.name);
        // Un archivo presente se acepta por tamaño: el hash completo se
        // comprobó al descargarlo, y la caché solo se escribe por rename.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == artifact.size) {
            continue;
        }
        if offline() {
            return Err(fail(format!(
                "falta {} en {} y HIVEDB_OFFLINE=1 impide descargarlo",
                artifact.name,
                dir.display()
            )));
        }
        download(artifact, &path)?;
    }

    Ok(ModelFiles {
        dir,
        space_id: format!("{REPO}@{}:mean-l2", &REVISION[..8]),
    })
}

fn download(artifact: &Artifact, destination: &Path) -> hivedb_index::Result<()> {
    let url = format!(
        "https://huggingface.co/{REPO}/resolve/{REVISION}/{}",
        artifact.name
    );
    if artifact.size > 1_000_000 {
        eprintln!(
            "hivedb-embed: descargando {} ({} MB) a {} …",
            artifact.name,
            artifact.size / 1_000_000,
            destination.display()
        );
    }

    let mut response = ureq::get(&url)
        .call()
        .map_err(|e| fail(format!("no se pudo descargar {url}: {e}")))?;
    let mut reader = response.body_mut().as_reader();

    // Archivo temporal propio del proceso: dos procesos descargando a la vez no
    // se pisan, y el rename final es atómico.
    let partial = destination.with_extension(format!("part-{}", std::process::id()));
    let result = write_verified(&mut reader, &partial, artifact);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    std::fs::rename(&partial, destination)
        .map_err(|e| fail(format!("{}: {e}", destination.display())))
}

fn write_verified(
    reader: &mut impl Read,
    partial: &Path,
    artifact: &Artifact,
) -> hivedb_index::Result<()> {
    let mut file =
        std::fs::File::create(partial).map_err(|e| fail(format!("{}: {e}", partial.display())))?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| fail(format!("descargando {}: {e}", artifact.name)))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .map_err(|e| fail(format!("{}: {e}", partial.display())))?;
        written += read as u64;
    }
    file.flush()
        .map_err(|e| fail(format!("{}: {e}", partial.display())))?;

    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if written != artifact.size || digest != artifact.sha256 {
        return Err(fail(format!(
            "{} no coincide con la versión esperada (tamaño {written}/{}, sha256 {digest})",
            artifact.name, artifact.size
        )));
    }
    Ok(())
}
