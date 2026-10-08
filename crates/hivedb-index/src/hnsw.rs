//! Índice vectorial derivado: ids de documento sobre el grafo plano de
//! [`crate::flat_hnsw`], con borrados por tombstone y volcado a disco.

use crate::flat_hnsw::{Collect, Graph, NoTrace, Recorder};
use crate::vector_file::View;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

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
/// Fichero de vectores de las versiones 1–3 del volcado.
const LEGACY_DATA_FILE: &str = "vectors.hnsw.data";
/// Versión 4: grafo plano propio (`flat_hnsw`) sin vectores (viven en el fichero
/// plano del almacén). Los volcados anteriores no sirven y se reconstruyen.
const GRAPH_DUMP_VERSION: u32 = 4;

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
}

/// Estado en memoria. Los vectores duraderos viven en el fichero plano del
/// almacén (`vector_file`) y los documentos en redb; este grafo es un índice
/// derivado que siempre puede reconstruirse.
struct Inner {
    graph: Graph,
    /// Ranura → id de documento (vacío si la ranura está muerta).
    ids: Vec<String>,
    latest: HashMap<String, usize>,
    deleted: HashSet<usize>,
}

impl Inner {
    fn register(&mut self, id: &str) {
        if let Some(old) = self.latest.get(id) {
            self.deleted.insert(*old);
        }
        let slot = self.ids.len();
        self.latest.insert(id.to_string(), slot);
        self.ids.push(id.to_string());
    }

    fn delete(&mut self, id: &str) -> bool {
        match self.latest.remove(id) {
            Some(slot) => {
                self.deleted.insert(slot);
                true
            }
            None => false,
        }
    }
}

/// Un paso de la búsqueda HNSW: `node` entró al frente de búsqueda de `layer`
/// alcanzado desde `from` (`None` para el punto de entrada).
#[derive(Clone, Debug, PartialEq)]
pub struct TraceStep {
    pub layer: u8,
    pub from: Option<String>,
    pub node: String,
    /// Distancia coseno (1 − similitud) de `node` a la consulta.
    pub distance: f32,
}

/// Resultado de [`VectorIndex::search_traced`].
#[derive(Clone, Debug, PartialEq)]
pub struct VectorTrace {
    /// `(id, posición, similitud)`, igual que `search`.
    pub hits: Vec<(String, usize, f32)>,
    pub steps: Vec<TraceStep>,
}

/// Índice ANN derivado con upsert/delete mediante tombstones en memoria.
pub struct VectorIndex {
    inner: RwLock<Inner>,
    dimension: usize,
}

impl VectorIndex {
    /// Índice vacío sobre una vista sin ranuras.
    pub(crate) fn new(dimension: usize, view: Arc<View>) -> Self {
        Self {
            inner: RwLock::new(Inner {
                graph: Graph::new(dimension, HNSW_M, HNSW_EF_CONSTRUCTION, view),
                ids: Vec::new(),
                latest: HashMap::new(),
                deleted: HashSet::new(),
            }),
            dimension,
        }
    }

    /// Construye el grafo sobre todas las ranuras de `view`. `slot_ids[s]` es
    /// el documento vivo que usa la ranura `s`, o `None` si está muerta.
    pub(crate) fn build(
        dimension: usize,
        view: Arc<View>,
        slot_ids: Vec<Option<String>>,
    ) -> crate::Result<Self> {
        if slot_ids.len() != view.slots() {
            return Err(crate::IndexError::Storage(format!(
                "vector file has {} slots but {} were expected",
                view.slots(),
                slot_ids.len()
            )));
        }
        let mut inner = Inner {
            graph: Graph::new(
                dimension,
                HNSW_M,
                HNSW_EF_CONSTRUCTION,
                Arc::new(View::empty(dimension)),
            ),
            ids: Vec::with_capacity(slot_ids.len()),
            latest: HashMap::new(),
            deleted: HashSet::new(),
        };
        for (slot, id) in slot_ids.into_iter().enumerate() {
            match id {
                Some(id) => inner.register(&id),
                None => {
                    inner.ids.push(String::new());
                    inner.deleted.insert(slot);
                }
            }
        }
        inner.graph.extend(view);
        Ok(Self {
            inner: RwLock::new(inner),
            dimension,
        })
    }

    /// Enlaza las ranuras nuevas de `view` (una por id, en orden). Si un id ya
    /// tenía vector, el anterior queda como tombstone.
    pub(crate) fn add_slots(&self, view: Arc<View>, ids: &[&str]) -> crate::Result<()> {
        let mut inner = self.inner.write().unwrap();
        if inner.ids.len() + ids.len() != view.slots() {
            return Err(crate::IndexError::Storage(format!(
                "vector file has {} slots but {} were expected",
                view.slots(),
                inner.ids.len() + ids.len()
            )));
        }
        for id in ids {
            inner.register(id);
        }
        inner.graph.extend(view);
        Ok(())
    }

    pub fn delete(&self, id: &str) -> crate::Result<()> {
        self.inner.write().unwrap().delete(id);
        Ok(())
    }

    pub fn search(
        &self,
        vector: &[f32],
        k: usize,
        ef_search: Option<usize>,
    ) -> crate::Result<Vec<(String, usize, f32)>> {
        self.search_with(vector, k, ef_search, &mut NoTrace)
    }

    /// Igual que [`search`](Self::search) pero devuelve además la ruta que
    /// recorrió el grafo (de la capa superior a la 0). Es una herramienta de
    /// diagnóstico: la ruta normal de búsqueda no registra nada.
    pub fn search_traced(
        &self,
        vector: &[f32],
        k: usize,
        ef_search: Option<usize>,
    ) -> crate::Result<VectorTrace> {
        let mut collect = Collect::default();
        let hits = self.search_with(vector, k, ef_search, &mut collect)?;
        let inner = self.inner.read().unwrap();
        let name = |node: u32| inner.ids.get(node as usize).cloned().unwrap_or_default();
        let steps = collect
            .0
            .into_iter()
            .map(|(layer, from, node, distance)| TraceStep {
                layer: layer as u8,
                from: (from != u32::MAX).then(|| name(from)),
                node: name(node),
                distance,
            })
            .collect();
        Ok(VectorTrace { hits, steps })
    }

    fn search_with<R: Recorder>(
        &self,
        vector: &[f32],
        k: usize,
        ef_search: Option<usize>,
        rec: &mut R,
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
        let neighbors = inner.graph.search_rec(vector, ef, want, rec);

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
    /// Los vectores no se vuelcan: viven en el fichero plano del almacén.
    pub fn dump(&self, dir: &Path, generation: u64, space_id: &str) -> crate::Result<()> {
        std::fs::create_dir_all(dir)?;
        // Invalida el volcado anterior antes de tocar sus archivos.
        let meta_path = dir.join(GRAPH_META_FILE);
        match std::fs::remove_file(&meta_path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }

        let inner = self.inner.read().unwrap();
        if inner.ids.is_empty() {
            return Ok(());
        }
        // Se escribe aparte y se mueve por rename: el meta, que es lo que valida
        // el volcado, va el último.
        let staging = dir.join("staging");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let outcome = (|| -> std::io::Result<()> {
            inner.graph.write_graph(&staging.join(GRAPH_FILE))?;
            std::fs::rename(staging.join(GRAPH_FILE), dir.join(GRAPH_FILE))
        })();
        let _ = std::fs::remove_dir_all(&staging);
        outcome?;
        // Restos de versiones anteriores, que guardaban los vectores aquí.
        let _ = std::fs::remove_file(dir.join(LEGACY_DATA_FILE));

        let meta = DumpMeta {
            version: GRAPH_DUMP_VERSION,
            generation,
            space_id: space_id.to_string(),
            dimension: self.dimension,
            ids: inner.ids.clone(),
            deleted: inner.deleted.iter().copied().collect(),
            graph_len: std::fs::metadata(dir.join(GRAPH_FILE))?.len(),
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
    pub(crate) fn load(
        dir: &Path,
        dimension: usize,
        generation: u64,
        space_id: &str,
        view: Arc<View>,
    ) -> Option<Self> {
        let raw = std::fs::read(dir.join(GRAPH_META_FILE)).ok()?;
        let meta: DumpMeta = bincode::deserialize(&raw).ok()?;
        let graph_path: PathBuf = dir.join(GRAPH_FILE);
        if meta.version != GRAPH_DUMP_VERSION
            || meta.generation != generation
            || meta.space_id != space_id
            || meta.dimension != dimension
            || std::fs::metadata(&graph_path).ok()?.len() != meta.graph_len
        {
            return None;
        }
        let graph = Graph::read(&graph_path, view, dimension)?;
        if graph.len() != meta.ids.len() || meta.deleted.iter().any(|&d| d >= meta.ids.len()) {
            return None;
        }

        let deleted: HashSet<usize> = meta.deleted.into_iter().collect();
        let latest = meta
            .ids
            .iter()
            .enumerate()
            .filter(|(slot, _)| !deleted.contains(slot))
            .map(|(slot, id)| (id.clone(), slot))
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
