//! Índice vectorial derivado: ids de documento sobre el grafo plano de
//! [`crate::flat_hnsw`], con borrados por tombstone y volcado a disco.

use crate::flat_hnsw::Graph;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const MAX_VECTOR_DIMENSION: usize = 65_536;

/// `ef` de búsqueda HNSW por defecto: con 200 el recall@10 a 100k vectores
/// agrupados es ~0,96 (ver `docs/BENCHMARKS.md`).
pub const DEFAULT_EF_SEARCH: usize = 200;
/// Vecinos por nodo y capa (la capa 0 admite el doble).
const HNSW_M: usize = 24;
/// Candidatos explorados al insertar.
const HNSW_EF_CONSTRUCTION: usize = 100;

const GRAPH_META_FILE: &str = "vectors.meta";
const GRAPH_FILE: &str = "vectors.hnsw.graph";
const DATA_FILE: &str = "vectors.hnsw.data";
/// Versión 3: grafo plano propio (`flat_hnsw`). Los volcados de `hnsw_rs`
/// (versiones 1 y 2) no sirven y se reconstruyen.
const GRAPH_DUMP_VERSION: u32 = 3;

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

/// Estado en memoria. Los vectores duraderos viven en el almacén semántico
/// (redb); este grafo es un índice derivado que siempre puede reconstruirse.
struct Inner {
    graph: Graph,
    ids: Vec<String>,
    latest: HashMap<String, usize>,
    deleted: HashSet<usize>,
}

impl Inner {
    fn new(dimension: usize) -> Self {
        Self {
            graph: Graph::new(dimension, HNSW_M, HNSW_EF_CONSTRUCTION),
            ids: Vec::new(),
            latest: HashMap::new(),
            deleted: HashSet::new(),
        }
    }

    /// Registra el id interno de un vector nuevo (el grafo lo recibe aparte).
    fn register(&mut self, id: &str) {
        if let Some(old) = self.latest.get(id) {
            self.deleted.insert(*old);
        }
        let internal_id = self.ids.len();
        self.latest.insert(id.to_string(), internal_id);
        self.ids.push(id.to_string());
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
    inner: RwLock<Inner>,
    dimension: usize,
}

impl VectorIndex {
    pub fn new(dimension: usize) -> Self {
        Self {
            inner: RwLock::new(Inner::new(dimension)),
            dimension,
        }
    }

    pub fn insert(&self, id: String, vector: Vec<f32>) -> crate::Result<()> {
        self.insert_many(&[(id.as_str(), vector.as_slice())])
    }

    /// Inserta varios vectores a la vez. Con lotes grandes el grafo se enlaza
    /// en paralelo; qué versión de cada id es la viva es lo mismo que
    /// insertando uno a uno.
    pub fn insert_many(&self, items: &[(&str, &[f32])]) -> crate::Result<()> {
        for (_, vector) in items {
            validate_vector(vector, self.dimension)?;
        }
        if items.is_empty() {
            return Ok(());
        }
        let mut inner = self.inner.write().unwrap();
        for (id, _) in items {
            inner.register(id);
        }
        let vectors: Vec<&[f32]> = items.iter().map(|(_, vector)| *vector).collect();
        inner.graph.insert_batch(&vectors);
        Ok(())
    }

    pub fn delete(&self, id: &str) -> crate::Result<()> {
        self.inner.write().unwrap().delete(id);
        Ok(())
    }

    pub fn clear(&self) -> crate::Result<()> {
        *self.inner.write().unwrap() = Inner::new(self.dimension);
        Ok(())
    }

    /// Reemplaza el grafo por los vectores vivos suministrados.
    pub fn rebuild<'a, I>(&self, vectors: I) -> crate::Result<()>
    where
        I: IntoIterator<Item = (&'a str, &'a [f32])>,
    {
        let mut rebuilt = Inner::new(self.dimension);
        let mut refs: Vec<&[f32]> = Vec::new();
        for (id, vector) in vectors {
            validate_vector(vector, self.dimension)?;
            rebuilt.register(id);
            refs.push(vector);
        }
        rebuilt.graph.insert_batch(&refs);
        *self.inner.write().unwrap() = rebuilt;
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

        let inner = self.inner.read().unwrap();
        let want = k
            .saturating_mul(4)
            .max(k.saturating_add(inner.deleted.len()))
            .max(1);
        let ef = want.max(ef_search.unwrap_or(DEFAULT_EF_SEARCH));
        let neighbors = inner.graph.search(vector, ef, want);

        let mut results = Vec::with_capacity(k);
        for (internal_id, distance) in neighbors {
            let internal_id = internal_id as usize;
            if inner.deleted.contains(&internal_id) {
                continue;
            }
            if let Some(id) = inner.ids.get(internal_id) {
                let similarity = (1.0 - distance).clamp(-1.0, 1.0);
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
    /// Si los vectores en disco ya están al día solo se reescribe el grafo.
    pub fn dump(&self, dir: &Path, generation: u64, space_id: &str) -> crate::Result<()> {
        std::fs::create_dir_all(dir)?;
        // Invalida el volcado anterior antes de tocar sus archivos.
        let meta_path = dir.join(GRAPH_META_FILE);
        match std::fs::remove_file(&meta_path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }

        let mut inner = self.inner.write().unwrap();
        if inner.ids.is_empty() {
            return Ok(());
        }
        // Se escribe en un subdirectorio y se mueve sobre los archivos viejos
        // por rename: así un fallo a medias no deja un volcado a medio escribir
        // con la apariencia de válido (el meta, que es lo que lo valida, va el último).
        let staging = dir.join("staging");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let outcome = (|| -> std::io::Result<()> {
            inner.graph.write_graph(&staging.join(GRAPH_FILE))?;
            let write_vectors = !inner.graph.vectors_clean() || !dir.join(DATA_FILE).exists();
            if write_vectors {
                inner.graph.write_vectors(&staging.join(DATA_FILE))?;
                std::fs::rename(staging.join(DATA_FILE), dir.join(DATA_FILE))?;
            }
            std::fs::rename(staging.join(GRAPH_FILE), dir.join(GRAPH_FILE))?;
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&staging);
        outcome?;
        inner.graph.mark_vectors_persisted();

        let meta = DumpMeta {
            version: GRAPH_DUMP_VERSION,
            generation,
            space_id: space_id.to_string(),
            dimension: self.dimension,
            ids: inner.ids.clone(),
            deleted: inner.deleted.iter().copied().collect(),
            graph_len: std::fs::metadata(dir.join(GRAPH_FILE))?.len(),
            data_len: std::fs::metadata(dir.join(DATA_FILE))?.len(),
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
    pub fn load(dir: &Path, dimension: usize, generation: u64, space_id: &str) -> Option<Self> {
        let raw = std::fs::read(dir.join(GRAPH_META_FILE)).ok()?;
        let meta: DumpMeta = bincode::deserialize(&raw).ok()?;
        let graph_path: PathBuf = dir.join(GRAPH_FILE);
        let data_path: PathBuf = dir.join(DATA_FILE);
        if meta.version != GRAPH_DUMP_VERSION
            || meta.generation != generation
            || meta.space_id != space_id
            || meta.dimension != dimension
            || std::fs::metadata(&graph_path).ok()?.len() != meta.graph_len
            || std::fs::metadata(&data_path).ok()?.len() != meta.data_len
        {
            return None;
        }
        let graph = Graph::read(&graph_path, &data_path, dimension)?;
        if graph.len() != meta.ids.len() || meta.deleted.iter().any(|&d| d >= meta.ids.len()) {
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
            inner: RwLock::new(Inner {
                graph,
                ids: meta.ids,
                latest,
                deleted,
            }),
            dimension,
        })
    }

    pub fn should_compact(&self) -> bool {
        let inner = self.inner.read().unwrap();
        inner.deleted.len() >= 1_024 && inner.deleted.len().saturating_mul(4) >= inner.ids.len()
    }

    pub fn stats(&self) -> (usize, usize) {
        let inner = self.inner.read().unwrap();
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
