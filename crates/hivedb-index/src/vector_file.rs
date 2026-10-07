//! Fichero plano de vectores: la copia **autoritativa** de los vectores del
//! índice semántico.
//!
//! Los documentos de `redb` solo guardan el número de ranura (`slot`) de su
//! vector; los vectores viven aquí, contiguos, normalizados (L2) y en `f32`
//! little-endian. El grafo ANN lee de este mismo fichero mapeado en memoria, así
//! que cada vector ocupa disco una sola vez.
//!
//! # Formato
//!
//! Cabecera de 16 bytes (`HIVEVEC1`, `dimension: u32`, reservado) y después una
//! fila de `dimension * 4` bytes por ranura. Las ranuras solo se añaden al final.
//!
//! # Coherencia con `redb`
//!
//! Escribir una tanda es: (1) escribir las filas en la cola del fichero y
//! `fsync`, (2) confirmar la transacción de `redb` que referencia esas ranuras,
//! (3) publicar la nueva vista. Si el proceso muere entre (1) y (2) quedan filas
//! huérfanas al final; al abrir, el almacén trunca el fichero al número de
//! ranuras confirmadas en `redb`. Una vista solo incluye ranuras confirmadas.
//!
//! # Concurrencia
//!
//! Los lectores toman una [`View`] (`Arc`) inmutable: leer un vector es indexar
//! un slice, sin locks. Al publicar se crea otra vista sobre el fichero ya
//! ampliado; las vistas antiguas siguen siendo válidas mientras alguien las use.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

const MAGIC: &[u8; 8] = b"HIVEVEC1";
const HEADER_LEN: usize = 16;

/// Instantánea de solo lectura de las ranuras publicadas.
pub(crate) struct View {
    map: Option<memmap2::Mmap>,
    slots: usize,
    dim: usize,
}

impl View {
    pub(crate) fn empty(dim: usize) -> Self {
        Self {
            map: None,
            slots: 0,
            dim,
        }
    }

    pub(crate) fn slots(&self) -> usize {
        self.slots
    }

    /// Vector de una ranura publicada, o `None` si no existe.
    pub(crate) fn try_get(&self, slot: usize) -> Option<&[f32]> {
        if slot >= self.slots {
            return None;
        }
        let map = self.map.as_ref()?;
        let start = HEADER_LEN + slot * self.dim * 4;
        Some(bytemuck::cast_slice(&map[start..start + self.dim * 4]))
    }

    /// Vector de una ranura. El llamador garantiza `slot < slots()`.
    pub(crate) fn get(&self, slot: u32) -> &[f32] {
        self.try_get(slot as usize)
            .expect("ranura fuera de la vista publicada")
    }
}

/// Fichero de vectores abierto para lectura y ampliación.
pub(crate) struct VectorFile {
    path: PathBuf,
    dim: usize,
    /// Serializa a los escritores y guarda el manejador de escritura.
    writer: Mutex<File>,
    view: RwLock<Arc<View>>,
}

fn header(dim: usize) -> [u8; HEADER_LEN] {
    let mut out = [0u8; HEADER_LEN];
    out[..8].copy_from_slice(MAGIC);
    out[8..12].copy_from_slice(&(dim as u32).to_le_bytes());
    out
}

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

/// Sincroniza el directorio para que un fichero recién creado sobreviva a un
/// corte de luz (mejor esfuerzo; no existe en todas las plataformas).
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

impl VectorFile {
    /// Abre el fichero (creándolo vacío si no existe). La vista inicial incluye
    /// todas las filas completas que haya; el almacén la recorta con
    /// [`VectorFile::truncate_to`] a lo confirmado en `redb`.
    pub(crate) fn open(path: &Path, dim: usize) -> std::io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            file.write_all(&header(dim))?;
            file.sync_all()?;
            sync_dir(path);
        } else {
            let mut found = [0u8; HEADER_LEN];
            file.read_exact(&mut found)
                .map_err(|_| invalid("vector file header truncated"))?;
            if &found[..8] != MAGIC {
                return Err(invalid("vector file has an unknown format"));
            }
            let stored_dim = u32::from_le_bytes([found[8], found[9], found[10], found[11]]);
            if stored_dim as usize != dim {
                return Err(invalid(format!(
                    "vector file dimension {stored_dim} does not match {dim}"
                )));
            }
        }
        let this = Self {
            path: path.to_path_buf(),
            dim,
            writer: Mutex::new(file),
            view: RwLock::new(Arc::new(View::empty(dim))),
        };
        let slots = (this.file_len()?.saturating_sub(HEADER_LEN)) / (dim * 4);
        this.publish(slots)?;
        Ok(this)
    }

    /// Crea un fichero nuevo con las filas dadas (para compactar). Lo deja
    /// sincronizado y publicado.
    pub(crate) fn create_with<'a, I>(path: &Path, dim: usize, rows: I) -> std::io::Result<Self>
    where
        I: IntoIterator<Item = &'a [f32]>,
    {
        let _ = std::fs::remove_file(path);
        let this = Self::open(path, dim)?;
        let rows: Vec<&[f32]> = rows.into_iter().collect();
        this.write_rows(0, &rows)?;
        this.publish(rows.len())?;
        Ok(this)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn file_len(&self) -> std::io::Result<usize> {
        Ok(self.writer.lock().unwrap().metadata()?.len() as usize)
    }

    /// Ranuras publicadas.
    pub(crate) fn slots(&self) -> usize {
        self.view.read().unwrap().slots
    }

    pub(crate) fn view(&self) -> Arc<View> {
        Arc::clone(&self.view.read().unwrap())
    }

    /// Escribe `rows` (ya normalizadas) a partir de la ranura `first` y las
    /// sincroniza a disco. No las publica.
    pub(crate) fn write_rows(&self, first: usize, rows: &[&[f32]]) -> std::io::Result<()> {
        let file = self.writer.lock().unwrap();
        let mut bytes = Vec::with_capacity(rows.len() * self.dim * 4);
        for row in rows {
            debug_assert_eq!(row.len(), self.dim);
            for value in *row {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        write_at(&file, &bytes, (HEADER_LEN + first * self.dim * 4) as u64)?;
        file.sync_data()
    }

    /// Hace visibles las primeras `slots` ranuras.
    pub(crate) fn publish(&self, slots: usize) -> std::io::Result<()> {
        let file = self.writer.lock().unwrap();
        let view = if slots == 0 {
            View::empty(self.dim)
        } else {
            // SAFETY: mapeo de solo lectura. HiveDB es el único escritor del
            // directorio y solo amplía el fichero o lo trunca antes de mapear;
            // si otro proceso lo truncara mientras está mapeado, el acceso
            // fallaría (SIGBUS).
            let map = unsafe { memmap2::Mmap::map(&*file) }?;
            if map.len() < HEADER_LEN + slots * self.dim * 4 {
                return Err(invalid("vector file shorter than the published slots"));
            }
            View {
                map: Some(map),
                slots,
                dim: self.dim,
            }
        };
        drop(file);
        *self.view.write().unwrap() = Arc::new(view);
        Ok(())
    }

    /// Recorta el fichero a `slots` ranuras (descarta las huérfanas) y publica.
    pub(crate) fn truncate_to(&self, slots: usize) -> std::io::Result<()> {
        // Se suelta la vista actual antes de truncar: un mapeo vivo sobre bytes
        // que desaparecen no se puede tocar.
        *self.view.write().unwrap() = Arc::new(View::empty(self.dim));
        {
            let file = self.writer.lock().unwrap();
            file.set_len((HEADER_LEN + slots * self.dim * 4) as u64)?;
            file.sync_all()?;
        }
        self.publish(slots)
    }
}

#[cfg(unix)]
fn write_at(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(bytes, offset)
}

#[cfg(windows)]
fn write_at(file: &File, bytes: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut written = 0;
    while written < bytes.len() {
        let n = file.seek_write(&bytes[written..], offset + written as u64)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        written += n;
    }
    Ok(())
}

/// Normaliza a norma L2 = 1 (los vectores nulos no llegan aquí: se validan antes).
pub(crate) fn normalized(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        vector.iter().map(|x| x / norm).collect()
    } else {
        vector.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escribe_publica_y_reabre() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.dat");
        let file = VectorFile::open(&path, 3).unwrap();
        assert_eq!(file.slots(), 0);
        let (a, b) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        file.write_rows(0, &[&a, &b]).unwrap();
        // Sin publicar no se ve.
        assert_eq!(file.view().slots(), 0);
        file.publish(2).unwrap();
        assert_eq!(file.view().get(1), &b);
        assert!(file.view().try_get(2).is_none());

        drop(file);
        let reopened = VectorFile::open(&path, 3).unwrap();
        assert_eq!(reopened.slots(), 2);
        assert_eq!(reopened.view().get(0), &a);
    }

    #[test]
    fn las_filas_huerfanas_se_descartan_al_truncar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.dat");
        let file = VectorFile::open(&path, 2).unwrap();
        file.write_rows(0, &[&[1.0, 0.0], &[0.0, 1.0], &[0.6, 0.8]])
            .unwrap();
        file.publish(3).unwrap();
        file.truncate_to(2).unwrap();
        assert_eq!(file.slots(), 2);
        drop(file);
        assert_eq!(VectorFile::open(&path, 2).unwrap().slots(), 2);
    }

    #[test]
    fn una_vista_antigua_sigue_siendo_valida_tras_ampliar() {
        let dir = tempfile::tempdir().unwrap();
        let file = VectorFile::open(&dir.path().join("v.dat"), 2).unwrap();
        file.write_rows(0, &[&[1.0, 0.0]]).unwrap();
        file.publish(1).unwrap();
        let old = file.view();
        file.write_rows(1, &[&[0.0, 1.0]]).unwrap();
        file.publish(2).unwrap();
        assert_eq!(old.slots(), 1);
        assert_eq!(old.get(0), &[1.0, 0.0]);
        assert_eq!(file.view().get(1), &[0.0, 1.0]);
    }

    #[test]
    fn rechaza_otra_dimension_y_formatos_desconocidos() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.dat");
        drop(VectorFile::open(&path, 4).unwrap());
        assert!(VectorFile::open(&path, 5).is_err());
        std::fs::write(&path, b"basura de otro formato!").unwrap();
        assert!(VectorFile::open(&path, 4).is_err());
    }

    #[test]
    fn create_with_reemplaza_el_contenido() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.dat");
        let rows: Vec<Vec<f32>> = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let file = VectorFile::create_with(&path, 2, rows.iter().map(Vec::as_slice)).unwrap();
        assert_eq!(file.slots(), 2);
        let again = VectorFile::create_with(&path, 2, std::iter::empty()).unwrap();
        assert_eq!(again.slots(), 0);
    }
}
