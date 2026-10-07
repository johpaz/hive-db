use hnsw_rs::api::AnnT;
use hnsw_rs::hnswio::HnswIo;
use hnsw_rs::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Mutex;

pub const MAX_VECTOR_DIMENSION: usize = 65_536;

/// `ef` de búsqueda HNSW por defecto. Con 50 el recall@10 era 0,27 (aleatorio)
/// y 0,73 (agrupado) en 10k docs; con 200 sube a 0,62 y 0,87.
pub const DEFAULT_EF_SEARCH: usize = 200;

const GRAPH_BASENAME: &str = "vectors";
const GRAPH_META_FILE: &str = "vectors.meta";
const GRAPH_DUMP_VERSION: u32 = 1;

/// Metadatos del volcado del grafo. Se escribe al final y por rename: sin
/// este archivo el volcado se considera inexistente.
#[derive(Serialize, Deserialize)]
struct DumpMeta {
    version: u32,
    generation: u64,
    space_id: String,
    dimension: usize,
    ids: Vec<String>,
    deleted: Vec<usize>,
    graph_len: u64,
    data_len: u64,
}

/// Carga el grafo tomando el `HnswIo` por valor, para que el resultado no
/// dependa de un préstamo local.
fn load_static(io: &'static mut HnswIo) -> Option<Hnsw<'static, f32, DistCosine>> {
    io.load_hnsw::<f32, DistCosine>().ok()
}

fn move_dump_files(from: &Path, to: &Path) -> std::io::Result<()> {
    for suffix in ["hnsw.graph", "hnsw.data"] {
        std::fs::rename(dump_file(from, suffix), dump_file(to, suffix))?;
    }
    Ok(())
}

fn dump_file(dir: &Path, suffix: &str) -> std::path::PathBuf {
    dir.join(format!("{GRAPH_BASENAME}.{suffix}"))
}

/// In-memory HNSW state. Durable vectors live in the semantic redb store;
/// this graph is a derived index that can always be rebuilt.
struct Inner {
    hnsw: Hnsw<'static, f32, DistCosine>,
    ids: Vec<String>,
    latest: HashMap<String, usize>,
    deleted: HashSet<usize>,
}

impl Inner {
    fn new() -> Self {
        Self {
            hnsw: Hnsw::new(16, 100_000, 200, 16, DistCosine),
            ids: Vec::new(),
            latest: HashMap::new(),
            deleted: HashSet::new(),
        }
    }

    fn insert(&mut self, id: String, vector: &[f32]) {
        if let Some(old) = self.latest.get(&id) {
            self.deleted.insert(*old);
        }
        let internal_id = self.ids.len();
        self.latest.insert(id.clone(), internal_id);
        self.ids.push(id);
        self.hnsw.insert((vector, internal_id));
    }

    fn delete(&mut self, id: &str) -> bool {
        match self.latest.remove(id) {
            Some(internal_id) => {
                self.deleted.insert(internal_id);
                true
            }
            None => false,
        }
    }
}

/// Índice ANN derivado con upsert/delete mediante tombstones en memoria.
pub struct VectorIndex {
    inner: Mutex<Inner>,
    dimension: usize,
}

impl VectorIndex {
    pub fn new(dimension: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::new()),
            dimension,
        }
    }

    pub fn insert(&self, id: String, vector: Vec<f32>) -> crate::Result<()> {
        validate_vector(&vector, self.dimension)?;
        self.inner.lock().unwrap().insert(id, &vector);
        Ok(())
    }

    pub fn delete(&self, id: &str) -> crate::Result<()> {
        self.inner.lock().unwrap().delete(id);
        Ok(())
    }

    pub fn clear(&self) -> crate::Result<()> {
        *self.inner.lock().unwrap() = Inner::new();
        Ok(())
    }

    /// Reemplaza el grafo por los vectores vivos suministrados.
    pub fn rebuild<'a, I>(&self, vectors: I) -> crate::Result<()>
    where
        I: IntoIterator<Item = (&'a str, &'a [f32])>,
    {
        // Los ids internos se asignan en orden; la inserción en el grafo es
        // paralela (`hnsw_rs` la soporta), que es lo que domina el arranque.
        let mut rebuilt = Inner::new();
        let mut batch: Vec<(&[f32], usize)> = Vec::new();
        for (id, vector) in vectors {
            validate_vector(vector, self.dimension)?;
            if let Some(old) = rebuilt.latest.get(id) {
                rebuilt.deleted.insert(*old);
            }
            let internal_id = rebuilt.ids.len();
            rebuilt.latest.insert(id.to_string(), internal_id);
            rebuilt.ids.push(id.to_string());
            batch.push((vector, internal_id));
        }
        rebuilt.hnsw.parallel_insert_slice(&batch);
        *self.inner.lock().unwrap() = rebuilt;
        Ok(())
    }

    pub fn search(
        &self,
        vector: &[f32],
        k: usize,
        ef_search: Option<usize>,
    ) -> crate::Result<Vec<(String, usize, f32)>> {
        validate_vector(vector, self.dimension)?;
        if k == 0 {
            return Err(crate::IndexError::InvalidVector(
                "k must be greater than zero".into(),
            ));
        }

        let inner = self.inner.lock().unwrap();
        let want = k
            .saturating_mul(4)
            .max(k.saturating_add(inner.deleted.len()))
            .max(1);
        let ef = want.max(ef_search.unwrap_or(DEFAULT_EF_SEARCH));
        let neighbors = inner.hnsw.search(vector, want, ef);

        let mut results = Vec::with_capacity(k);
        for neighbor in neighbors {
            let internal_id = neighbor.d_id;
            if inner.deleted.contains(&internal_id) {
                continue;
            }
            if let Some(id) = inner.ids.get(internal_id) {
                let similarity = (1.0 - neighbor.distance).clamp(-1.0, 1.0);
                results.push((id.clone(), results.len() + 1, similarity));
                if results.len() == k {
                    break;
                }
            }
        }
        Ok(results)
    }

    /// Vuelca el grafo a `dir` para evitar reconstruirlo en la próxima apertura.
    ///
    /// El grafo es un índice derivado: cualquier fallo aquí es recuperable (la
    /// siguiente apertura lo reconstruye), así que el llamador puede ignorarlo.
    pub fn dump(&self, dir: &Path, generation: u64, space_id: &str) -> crate::Result<()> {
        std::fs::create_dir_all(dir)?;
        // Invalida el volcado anterior antes de tocar sus archivos.
        let meta_path = dir.join(GRAPH_META_FILE);
        match std::fs::remove_file(&meta_path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }

        let inner = self.inner.lock().unwrap();
        if inner.ids.is_empty() {
            return Ok(());
        }
        // `file_dump` renombra a un nombre único si ya existen archivos del
        // mismo nombre (p. ej. en un grafo recargado), así que se vuelca en un
        // subdirectorio vacío y luego se mueven los archivos sobre los viejos.
        let staging = dir.join("staging");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let dumped = catch_unwind(AssertUnwindSafe(|| {
            inner.hnsw.file_dump(&staging, GRAPH_BASENAME)
        }));
        let outcome = match dumped {
            Ok(Ok(basename)) if basename == GRAPH_BASENAME => {
                move_dump_files(&staging, dir).map_err(crate::IndexError::from)
            }
            Ok(Ok(other)) => Err(crate::IndexError::Storage(format!(
                "hnsw dump used unexpected basename {other}"
            ))),
            Ok(Err(error)) => Err(crate::IndexError::Storage(error.to_string())),
            Err(_) => Err(crate::IndexError::Storage("hnsw dump panicked".into())),
        };
        let _ = std::fs::remove_dir_all(&staging);
        outcome?;

        let meta = DumpMeta {
            version: GRAPH_DUMP_VERSION,
            generation,
            space_id: space_id.to_string(),
            dimension: self.dimension,
            ids: inner.ids.clone(),
            deleted: inner.deleted.iter().copied().collect(),
            graph_len: std::fs::metadata(dump_file(dir, "hnsw.graph"))?.len(),
            data_len: std::fs::metadata(dump_file(dir, "hnsw.data"))?.len(),
        };
        let temp = meta_path.with_extension("meta.tmp");
        std::fs::write(&temp, bincode::serialize(&meta)?)?;
        std::fs::rename(&temp, &meta_path)?;
        Ok(())
    }

    /// Carga un volcado previo si coincide exactamente con el estado esperado.
    ///
    /// Devuelve `None` ante cualquier duda (archivos ausentes, otra generación,
    /// otro espacio, truncados o ilegibles): el llamador reconstruye desde los
    /// documentos, que son la fuente de verdad.
    pub fn load(
        dir: &Path,
        dimension: usize,
        generation: u64,
        space_id: &str,
        expected_live: usize,
    ) -> Option<Self> {
        let raw = std::fs::read(dir.join(GRAPH_META_FILE)).ok()?;
        let meta: DumpMeta = bincode::deserialize(&raw).ok()?;
        let live = meta.ids.len().checked_sub(meta.deleted.len())?;
        if meta.version != GRAPH_DUMP_VERSION
            || meta.generation != generation
            || meta.space_id != space_id
            || meta.dimension != dimension
            || live != expected_live
            || std::fs::metadata(dump_file(dir, "hnsw.graph")).ok()?.len() != meta.graph_len
            || std::fs::metadata(dump_file(dir, "hnsw.data")).ok()?.len() != meta.data_len
        {
            return None;
        }

        // `load_hnsw` toma prestado el `HnswIo`; se filtra (unos bytes, sin mmap)
        // para obtener un grafo `'static`.
        let io: &'static mut HnswIo = Box::leak(Box::new(HnswIo::new(dir, GRAPH_BASENAME)));
        // El `Option` fuerza a mover (no reborrow) la referencia `'static`.
        let mut slot = Some(io);
        let hnsw = catch_unwind(AssertUnwindSafe(|| load_static(slot.take()?))).ok()??;
        if hnsw.get_nb_point() != meta.ids.len() {
            return None;
        }

        let deleted: HashSet<usize> = meta.deleted.into_iter().collect();
        let latest = meta
            .ids
            .iter()
            .enumerate()
            .filter(|(internal_id, _)| !deleted.contains(internal_id))
            .map(|(internal_id, id)| (id.clone(), internal_id))
            .collect();
        Some(Self {
            inner: Mutex::new(Inner {
                hnsw,
                ids: meta.ids,
                latest,
                deleted,
            }),
            dimension,
        })
    }

    pub fn should_compact(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.deleted.len() >= 1_024 && inner.deleted.len().saturating_mul(4) >= inner.ids.len()
    }

    pub fn stats(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap();
        (inner.latest.len(), inner.deleted.len())
    }
}

pub fn validate_vector(vector: &[f32], expected_dimension: usize) -> crate::Result<()> {
    if vector.len() != expected_dimension {
        return Err(crate::IndexError::DimensionMismatch {
            expected: expected_dimension,
            got: vector.len(),
        });
    }
    let mut norm_squared = 0.0f64;
    for (index, value) in vector.iter().copied().enumerate() {
        if !value.is_finite() {
            return Err(crate::IndexError::InvalidVector(format!(
                "coordinate {index} is not finite"
            )));
        }
        let value = f64::from(value);
        norm_squared += value * value;
    }
    if norm_squared == 0.0 {
        return Err(crate::IndexError::InvalidVector(
            "vector norm must be greater than zero".into(),
        ));
    }
    Ok(())
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    let (mut dot, mut left_norm, mut right_norm) = (0.0f64, 0.0f64, 0.0f64);
    for (&left, &right) in left.iter().zip(right) {
        let left = f64::from(left);
        let right = f64::from(right);
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    (dot / (left_norm * right_norm).sqrt()).clamp(-1.0, 1.0) as f32
}
