use crate::hnsw::{MAX_VECTOR_DIMENSION, VectorIndex, cosine_similarity, validate_vector};
use crate::rrf::rrf;
use crate::text::TextIndex;
use crate::types::{Fusion, Hit, HybridQuery, IndexDoc, ScalarFilter, VectorConfig};
use crate::vector_file::{VectorFile, View, normalized};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// Documentos con el vector dentro del registro (formato anterior a 0.6); solo se
/// lee para migrarlos.
const DOCS_V1: TableDefinition<&str, Vec<u8>> = TableDefinition::new("semantic_docs");
/// Tras migrar, `semantic_docs` se recrea con claves `u64` (vacía): una versión
/// anterior, que la abre con claves `&str`, falla con un error de tipo en vez de
/// ver un índice vacío y escribir en él. `meta.json` no se toca (las bases de
/// versiones anteriores no deben verse alteradas).
const DOCS_V1_TRIPWIRE: TableDefinition<u64, u64> = TableDefinition::new("semantic_docs");
/// Documentos sin vector, con la ranura del fichero plano.
const DOCS: TableDefinition<&str, Vec<u8>> = TableDefinition::new("semantic_docs_v2");
const META: TableDefinition<&str, u64> = TableDefinition::new("semantic_meta");
const GENERATION_KEY: &str = "generation";
/// Ranuras del fichero de vectores confirmadas en redb.
const VECTOR_SLOTS_KEY: &str = "vector_slots";
/// Cuál de los dos ficheros alternos de vectores (0 o 1) está activo.
const VECTOR_FILE_KEY: &str = "vector_file";
const STORE_FILE: &str = "semantic.redb";
const DATABASE_META_FILE: &str = "meta.json";
const GRAPH_DIR: &str = "hnsw";
/// Generación con la que se cerró limpiamente el índice de texto (`fts/`).
const FTS_MARKER_FILE: &str = "fts.generation";
/// Versión del análisis de texto con el que se construyó `fts/`. Súbela cada vez que cambie
/// cómo se tokeniza (filtros, listas de palabras vacías, stemmer): un índice de otra versión
/// no es válido y se reconstruye al abrir. (El marcador de versiones sin este campo mide 8
/// bytes y tampoco coincide.)
const FTS_ANALYSIS_VERSION: u32 = 3;
const SCHEMA_VERSION: u32 = 2;
/// Candidatos por fuente en una consulta híbrida: `k × FUSION_DEPTH_FACTOR`, con un mínimo.
const FUSION_DEPTH_FACTOR: usize = 5;
const FUSION_MIN_DEPTH: usize = 50;

#[derive(Debug, Serialize, Deserialize)]
struct DatabaseMeta {
    schema_version: u32,
    metric: String,
    vector: Option<VectorConfig>,
    /// Único campo del `meta.json` de 0.3.x. Se conserva al migrar una base de
    /// esa versión para que 0.3.x pueda volver a abrirla si hay que dar marcha
    /// atrás; no forma parte de la identidad del espacio vectorial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vector_dimension: Option<usize>,
}

impl DatabaseMeta {
    /// Lo que debe coincidir entre aperturas: esquema, métrica y espacio.
    fn same_space(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.metric == other.metric
            && self.vector == other.vector
    }
}

/// Documento tal como se guarda en redb: sin el vector, que vive en el fichero
/// plano (`vector_file`) y se referencia por su ranura.
#[derive(Serialize, Deserialize)]
struct StoredDoc {
    /// Siempre con `vector: None`.
    doc: IndexDoc,
    slot: Option<u64>,
}

/// Tanda máxima que se migra del formato antiguo por transacción.
const MIGRATION_BATCH: usize = 2_000;

struct SemanticStore {
    db: Database,
    base: PathBuf,
    dim: Option<usize>,
    /// Fichero de vectores activo; `None` si la base no tiene espacio vectorial.
    vectors: RwLock<Option<Arc<VectorFile>>>,
}

fn vector_file_path(base: &Path, index: u64) -> PathBuf {
    base.join(format!("vectors.{index}.dat"))
}

fn io_error(error: std::io::Error) -> crate::IndexError {
    crate::IndexError::Storage(format!("vector file: {error}"))
}

impl SemanticStore {
    fn open(base: &Path, dim: Option<usize>) -> crate::Result<Self> {
        migrate_legacy_store(base, dim)?;
        let db = Database::create(base.join(STORE_FILE)).map_err(storage_error)?;
        let txn = db.begin_write().map_err(storage_error)?;
        {
            txn.open_table(DOCS).map_err(storage_error)?;
            txn.open_table(META).map_err(storage_error)?;
            // Señal para versiones anteriores (ver `DOCS_V1_TRIPWIRE`). En una base que
            // venía del formato anterior ya la dejó la migración.
            txn.open_table(DOCS_V1_TRIPWIRE).map_err(storage_error)?;
        }
        txn.commit().map_err(storage_error)?;

        let store = Self {
            db,
            base: base.to_path_buf(),
            dim,
            vectors: RwLock::new(None),
        };
        let mut store = store;
        store.open_vector_file()?;
        Ok(store)
    }

    fn meta_u64(&self, key: &str) -> crate::Result<u64> {
        let txn = self.db.begin_read().map_err(storage_error)?;
        let meta = txn.open_table(META).map_err(storage_error)?;
        Ok(meta
            .get(key)
            .map_err(storage_error)?
            .map(|value| value.value())
            .unwrap_or(0))
    }

    /// Abre el fichero de vectores activo y lo concilia con lo confirmado en
    /// redb: las filas huérfanas de una tanda que no llegó a confirmarse se
    /// descartan; que falten filas confirmadas es corrupción.
    fn open_vector_file(&mut self) -> crate::Result<()> {
        let Some(dim) = self.dim else {
            return Ok(());
        };
        let active = self.meta_u64(VECTOR_FILE_KEY)?;
        let committed = self.meta_u64(VECTOR_SLOTS_KEY)? as usize;
        let file =
            VectorFile::open(&vector_file_path(&self.base, active), dim).map_err(io_error)?;
        match file.slots().cmp(&committed) {
            std::cmp::Ordering::Greater => file.truncate_to(committed).map_err(io_error)?,
            std::cmp::Ordering::Less => {
                return Err(crate::IndexError::Storage(format!(
                    "vector file has {} slots but the store references {committed}: \
                     the file is damaged or incomplete",
                    file.slots()
                )));
            }
            std::cmp::Ordering::Equal => {}
        }
        // El otro fichero es un resto de una compactación que no se confirmó
        // (o el de antes de una que sí).
        let _ = std::fs::remove_file(vector_file_path(&self.base, 1 - active.min(1)));
        *self.vectors.write().unwrap() = Some(Arc::new(file));
        Ok(())
    }

    fn vector_file(&self) -> Option<Arc<VectorFile>> {
        self.vectors.read().unwrap().clone()
    }

    /// Vista de los vectores publicados; `None` si la base no tiene vectores.
    fn view(&self) -> Option<Arc<View>> {
        self.vector_file().map(|file| file.view())
    }

    fn generation(&self) -> crate::Result<u64> {
        self.meta_u64(GENERATION_KEY)
    }

    fn doc_count(&self) -> crate::Result<u64> {
        let txn = self.db.begin_read().map_err(storage_error)?;
        let docs = txn.open_table(DOCS).map_err(storage_error)?;
        docs.len().map_err(storage_error)
    }

    fn load_all(&self) -> crate::Result<Vec<StoredDoc>> {
        let txn = self.db.begin_read().map_err(storage_error)?;
        let docs = txn.open_table(DOCS).map_err(storage_error)?;
        let mut loaded = Vec::new();
        for entry in docs.iter().map_err(storage_error)? {
            let (_id, value) = entry.map_err(storage_error)?;
            loaded.push(bincode::deserialize(&value.value())?);
        }
        Ok(loaded)
    }

    /// Ranura → documento vivo que la usa (o `None` si está muerta), para
    /// reconstruir el grafo.
    fn slot_ids(&self, docs: &[StoredDoc]) -> crate::Result<Vec<Option<String>>> {
        let slots = self.view().map_or(0, |view| view.slots());
        let mut ids = vec![None; slots];
        for stored in docs {
            if let Some(slot) = stored.slot {
                let entry = ids.get_mut(slot as usize).ok_or_else(|| {
                    crate::IndexError::Storage(format!(
                        "document {} references slot {slot} beyond the vector file",
                        stored.doc.id
                    ))
                })?;
                *entry = Some(stored.doc.id.clone());
            }
        }
        Ok(ids)
    }

    /// Vectores (normalizados) de los documentos dados que tengan uno.
    fn load_vectors(&self, ids: &[String]) -> crate::Result<Vec<(String, Vec<f32>)>> {
        let Some(view) = self.view() else {
            return Ok(Vec::new());
        };
        let txn = self.db.begin_read().map_err(storage_error)?;
        let docs = txn.open_table(DOCS).map_err(storage_error)?;
        let mut loaded = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(value) = docs.get(id.as_str()).map_err(storage_error)? else {
                continue;
            };
            let stored: StoredDoc = bincode::deserialize(&value.value())?;
            if let Some(slot) = stored.slot {
                let vector = view.try_get(slot as usize).ok_or_else(|| {
                    crate::IndexError::Storage(format!(
                        "document {id} references slot {slot} beyond the vector file"
                    ))
                })?;
                loaded.push((stored.doc.id, vector.to_vec()));
            }
        }
        Ok(loaded)
    }

    /// Guarda la tanda. Devuelve la generación y la vista de vectores tras
    /// publicar las ranuras nuevas.
    fn upsert_batch(&self, documents: &[IndexDoc]) -> crate::Result<(u64, Option<Arc<View>>)> {
        let file = self.vector_file();
        let rows: Vec<Vec<f32>> = documents
            .iter()
            .filter_map(|doc| doc.vector.as_deref().map(normalized))
            .collect();
        let first = file.as_ref().map_or(0, |f| f.slots());
        if !rows.is_empty() {
            let file = file
                .as_ref()
                .ok_or(crate::IndexError::VectorIndexDisabled)?;
            let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();
            // Primero los vectores, sincronizados; después la transacción que
            // los referencia (ver `vector_file`).
            file.write_rows(first, &refs).map_err(io_error)?;
        }

        let txn = self.db.begin_write().map_err(storage_error)?;
        let generation;
        {
            let mut docs = txn.open_table(DOCS).map_err(storage_error)?;
            let mut next = first as u64;
            for doc in documents {
                let mut doc = doc.clone();
                let slot = doc.vector.take().map(|_| {
                    next += 1;
                    next - 1
                });
                let id = doc.id.clone();
                let encoded = bincode::serialize(&StoredDoc { doc, slot })?;
                docs.insert(id.as_str(), encoded).map_err(storage_error)?;
            }
            let mut meta = txn.open_table(META).map_err(storage_error)?;
            if !rows.is_empty() {
                meta.insert(VECTOR_SLOTS_KEY, next).map_err(storage_error)?;
            }
            generation = next_generation(&mut meta)?;
        }
        txn.commit().map_err(storage_error)?;
        if let Some(file) = file.as_ref() {
            file.publish(first + rows.len()).map_err(io_error)?;
        }
        Ok((generation, file.map(|f| f.view())))
    }

    fn delete_ids(&self, ids: &[String]) -> crate::Result<u64> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        let generation;
        {
            let mut docs = txn.open_table(DOCS).map_err(storage_error)?;
            for id in ids {
                docs.remove(id.as_str()).map_err(storage_error)?;
            }
            let mut meta = txn.open_table(META).map_err(storage_error)?;
            generation = next_generation(&mut meta)?;
        }
        txn.commit().map_err(storage_error)?;
        Ok(generation)
    }

    /// Cambia el fichero de vectores activo por `replacement` (ya sincronizado)
    /// en la misma transacción que `apply` modifica los documentos. Si la
    /// transacción no se confirma, el fichero nuevo se borra.
    fn switch_vector_file<F>(
        &self,
        replacement: VectorFile,
        new_index: u64,
        apply: F,
    ) -> crate::Result<u64>
    where
        F: FnOnce(&mut redb::Table<'_, &str, Vec<u8>>) -> crate::Result<()>,
    {
        let committed = (|| -> crate::Result<u64> {
            let txn = self.db.begin_write().map_err(storage_error)?;
            let generation;
            {
                let mut docs = txn.open_table(DOCS).map_err(storage_error)?;
                apply(&mut docs)?;
                let mut meta = txn.open_table(META).map_err(storage_error)?;
                meta.insert(VECTOR_SLOTS_KEY, replacement.slots() as u64)
                    .map_err(storage_error)?;
                meta.insert(VECTOR_FILE_KEY, new_index)
                    .map_err(storage_error)?;
                generation = next_generation(&mut meta)?;
            }
            txn.commit().map_err(storage_error)?;
            Ok(generation)
        })();
        let generation = match committed {
            Ok(generation) => generation,
            Err(error) => {
                let _ = std::fs::remove_file(replacement.path());
                return Err(error);
            }
        };
        let old = self.vectors.write().unwrap().replace(Arc::new(replacement));
        if let Some(old) = old {
            // Mejor esfuerzo: si no se puede borrar (p. ej. aún mapeado en
            // Windows), la próxima apertura lo elimina.
            let _ = std::fs::remove_file(old.path());
        }
        Ok(generation)
    }

    fn clear(&self) -> crate::Result<u64> {
        let Some(dim) = self.dim else {
            let txn = self.db.begin_write().map_err(storage_error)?;
            let generation;
            {
                let mut docs = txn.open_table(DOCS).map_err(storage_error)?;
                docs.retain(|_, _| false).map_err(storage_error)?;
                let mut meta = txn.open_table(META).map_err(storage_error)?;
                generation = next_generation(&mut meta)?;
            }
            txn.commit().map_err(storage_error)?;
            return Ok(generation);
        };
        let new_index = 1 - self.meta_u64(VECTOR_FILE_KEY)?.min(1);
        let replacement = VectorFile::create_with(
            &vector_file_path(&self.base, new_index),
            dim,
            std::iter::empty(),
        )
        .map_err(io_error)?;
        self.switch_vector_file(replacement, new_index, |docs| {
            docs.retain(|_, _| false).map_err(storage_error)?;
            Ok(())
        })
    }

    /// Reescribe el fichero de vectores sin las ranuras muertas (documentos
    /// actualizados o borrados). Devuelve la nueva generación, o `None` si no
    /// había nada que compactar. Los números de ranura cambian, así que el grafo
    /// debe reconstruirse después.
    fn compact_vectors(&self) -> crate::Result<Option<u64>> {
        let (Some(dim), Some(file)) = (self.dim, self.vector_file()) else {
            return Ok(None);
        };
        let docs = self.load_all()?;
        let live = docs.iter().filter(|d| d.slot.is_some()).count();
        if live == file.slots() {
            return Ok(None);
        }
        let view = file.view();
        let new_index = 1 - self.meta_u64(VECTOR_FILE_KEY)?.min(1);
        let mut rows: Vec<&[f32]> = Vec::with_capacity(live);
        for stored in &docs {
            if let Some(slot) = stored.slot {
                rows.push(view.try_get(slot as usize).ok_or_else(|| {
                    crate::IndexError::Storage(format!(
                        "document {} references slot {slot} beyond the vector file",
                        stored.doc.id
                    ))
                })?);
            }
        }
        let replacement =
            VectorFile::create_with(&vector_file_path(&self.base, new_index), dim, rows)
                .map_err(io_error)?;
        let generation = self.switch_vector_file(replacement, new_index, |table| {
            let mut next = 0u64;
            for stored in docs {
                let slot = stored.slot.map(|_| {
                    next += 1;
                    next - 1
                });
                let id = stored.doc.id.clone();
                let encoded = bincode::serialize(&StoredDoc {
                    doc: stored.doc,
                    slot,
                })?;
                table.insert(id.as_str(), encoded).map_err(storage_error)?;
            }
            Ok(())
        })?;
        Ok(Some(generation))
    }
}

/// Pasa una base del formato anterior (el vector dentro del registro de cada documento, en la
/// tabla `semantic_docs`) al actual: documentos sin vector en `semantic_docs_v2` y los vectores
/// en el fichero plano.
///
/// Se escribe un fichero `redb` **nuevo** (`semantic.redb.migrating`) y, al terminar, sustituye
/// al antiguo con un `rename`. El antiguo no se toca hasta ese momento, así que una interrupción
/// en cualquier punto deja la base original intacta y la siguiente apertura repite la migración.
/// (Copiar a otra tabla del mismo fichero y compactar dejaba el fichero más grande que antes: la
/// compactación de `redb` no reclama bien los valores grandes.) Un fichero de vectores de un
/// intento anterior se descarta: no hay ranuras confirmadas hasta que termina la migración.
fn migrate_legacy_store(base: &Path, dim: Option<usize>) -> crate::Result<()> {
    let path = base.join(STORE_FILE);
    if !path.exists() {
        return Ok(());
    }
    // ¿Es de un formato anterior? Se abre la base para ver si existe `semantic_docs` con
    // claves de texto (la señal que deja la migración tiene otro tipo).
    let legacy = Database::open(&path).map_err(storage_error)?;
    let total = {
        let txn = legacy.begin_read().map_err(storage_error)?;
        match txn.open_table(DOCS_V1) {
            Ok(table) => table.len().map_err(storage_error)?,
            Err(
                redb::TableError::TableDoesNotExist(_) | redb::TableError::TableTypeMismatch { .. },
            ) => {
                return Ok(());
            }
            Err(error) => return Err(storage_error(error)),
        }
    };

    let migrating = base.join(format!("{STORE_FILE}.migrating"));
    let _ = std::fs::remove_file(&migrating);
    let new_db = Database::create(&migrating).map_err(storage_error)?;
    let file = match dim {
        Some(dim) => Some(
            VectorFile::create_with(&vector_file_path(base, 0), dim, std::iter::empty())
                .map_err(io_error)?,
        ),
        None => None,
    };

    let generation = {
        let txn = legacy.begin_read().map_err(storage_error)?;
        match txn.open_table(META) {
            Ok(meta) => meta
                .get(GENERATION_KEY)
                .map_err(storage_error)?
                .map_or(0, |v| v.value()),
            Err(_) => 0,
        }
    };

    let mut next_slot = 0usize;
    let mut cursor: Option<String> = None;
    let mut migrated = 0u64;
    while migrated < total {
        let batch: Vec<(String, IndexDoc)> = {
            let txn = legacy.begin_read().map_err(storage_error)?;
            let table = txn.open_table(DOCS_V1).map_err(storage_error)?;
            let lower = match cursor.as_deref() {
                Some(last) => std::ops::Bound::Excluded(last),
                None => std::ops::Bound::Unbounded,
            };
            let mut batch = Vec::new();
            for entry in table
                .range::<&str>((lower, std::ops::Bound::Unbounded))
                .map_err(storage_error)?
                .take(MIGRATION_BATCH)
            {
                let (key, value) = entry.map_err(storage_error)?;
                batch.push((
                    key.value().to_string(),
                    bincode::deserialize(&value.value())?,
                ));
            }
            batch
        };
        let Some((last, _)) = batch.last() else { break };
        cursor = Some(last.clone());

        let rows: Vec<Vec<f32>> = batch
            .iter()
            .filter_map(|(_, doc)| doc.vector.as_deref().map(normalized))
            .collect();
        if !rows.is_empty() {
            let file = file
                .as_ref()
                .ok_or(crate::IndexError::VectorIndexDisabled)?;
            if rows.iter().any(|row| Some(row.len()) != dim) {
                return Err(crate::IndexError::Storage(
                    "legacy document vector has a different dimension".into(),
                ));
            }
            let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();
            file.write_rows(next_slot, &refs).map_err(io_error)?;
        }

        let txn = new_db.begin_write().map_err(storage_error)?;
        {
            let mut docs = txn.open_table(DOCS).map_err(storage_error)?;
            let mut slot = next_slot as u64;
            for (key, mut doc) in batch {
                let assigned = doc.vector.take().map(|_| {
                    slot += 1;
                    slot - 1
                });
                docs.insert(
                    key.as_str(),
                    bincode::serialize(&StoredDoc {
                        doc,
                        slot: assigned,
                    })?,
                )
                .map_err(storage_error)?;
                migrated += 1;
            }
        }
        txn.commit().map_err(storage_error)?;
        next_slot += rows.len();
    }

    // Metadatos y señal para versiones anteriores, y todo sincronizado antes de sustituir.
    let txn = new_db.begin_write().map_err(storage_error)?;
    {
        txn.open_table(DOCS).map_err(storage_error)?;
        let mut meta = txn.open_table(META).map_err(storage_error)?;
        meta.insert(GENERATION_KEY, generation)
            .map_err(storage_error)?;
        meta.insert(VECTOR_SLOTS_KEY, next_slot as u64)
            .map_err(storage_error)?;
        meta.insert(VECTOR_FILE_KEY, 0u64).map_err(storage_error)?;
        txn.open_table(DOCS_V1_TRIPWIRE).map_err(storage_error)?;
    }
    txn.commit().map_err(storage_error)?;
    drop(file);
    drop(new_db);
    drop(legacy);
    std::fs::rename(&migrating, &path)?;
    Ok(())
}

fn next_generation(meta: &mut redb::Table<'_, &str, u64>) -> crate::Result<u64> {
    let generation = meta
        .get(GENERATION_KEY)
        .map_err(storage_error)?
        .map(|value| value.value())
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| crate::IndexError::Storage("semantic generation overflow".into()))?;
    meta.insert(GENERATION_KEY, generation)
        .map_err(storage_error)?;
    Ok(generation)
}

fn storage_error(error: impl std::fmt::Display) -> crate::IndexError {
    crate::IndexError::Storage(error.to_string())
}

struct DerivedState {
    text: TextIndex,
    vector: Option<VectorIndex>,
    generation: u64,
}

/// Índice semántico híbrido respaldado por documentos autoritativos en redb.
pub struct SemanticIndex {
    store: SemanticStore,
    vector_config: Option<VectorConfig>,
    state: RwLock<DerivedState>,
    /// Directorio del volcado del grafo ANN; `None` en índices en RAM.
    graph_dir: Option<PathBuf>,
    /// `true` si el volcado en disco coincide con el grafo en memoria.
    graph_saved: AtomicBool,
    _temp_dir: Option<tempfile::TempDir>,
}

impl SemanticIndex {
    pub fn open<P: AsRef<Path>>(
        base_dir: P,
        vector_config: Option<VectorConfig>,
    ) -> crate::Result<Self> {
        Self::open_inner(base_dir.as_ref(), vector_config, None)
    }

    pub fn open_in_ram(vector_config: Option<VectorConfig>) -> crate::Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let base = temp_dir.path().to_path_buf();
        Self::open_inner(&base, vector_config, Some(temp_dir))
    }

    fn open_inner(
        base: &Path,
        vector_config: Option<VectorConfig>,
        temp_dir: Option<tempfile::TempDir>,
    ) -> crate::Result<Self> {
        std::fs::create_dir_all(base)?;
        validate_config(vector_config.as_ref())?;
        resolve_database_meta(base, vector_config.as_ref())?;

        let fts_dir = base.join("fts");
        std::fs::create_dir_all(&fts_dir)?;
        let store = SemanticStore::open(base, vector_config.as_ref().map(|c| c.dimension))?;
        let generation = store.generation()?;
        let graph_dir = base.join(GRAPH_DIR);
        // Restos de versiones anteriores, que guardaban aquí los vectores.
        let _ = std::fs::remove_file(graph_dir.join("vectors.hnsw.data"));

        // Camino rápido: si el cierre anterior dejó el índice de texto y el
        // grafo ANN al día con esta generación, no hace falta leer ni un
        // documento. El marcador se borra al abrir y se reescribe al cerrar:
        // un fallo a mitad de sesión lo deja ausente y se reconstruye todo.
        let fts_marker = base.join(FTS_MARKER_FILE);
        let marked = read_fts_marker(&fts_marker);
        let _ = std::fs::remove_file(&fts_marker);
        let text = TextIndex::open(fts_dir)?;
        let fts_ready = marked == Some(generation) && text.num_docs()? == store.doc_count()?;
        let mut loaded = match (vector_config.as_ref(), store.view()) {
            (Some(config), Some(view)) => VectorIndex::load(
                &graph_dir,
                config.dimension,
                generation,
                &config.space_id,
                view,
            ),
            _ => None,
        };

        let documents = if fts_ready && (loaded.is_some() || vector_config.is_none()) {
            None
        } else {
            let documents = store.load_all()?;
            if !fts_ready {
                text.clear()?;
                text.upsert_batch(&documents.iter().map(|d| d.doc.clone()).collect::<Vec<_>>())?;
            }
            // Defensa extra: el grafo restaurado debe tener tantos vectores
            // vivos como documentos con vector.
            let expected_live = documents.iter().filter(|d| d.slot.is_some()).count();
            if loaded
                .as_ref()
                .is_some_and(|index| index.stats().0 != expected_live)
            {
                loaded = None;
            }
            Some(documents)
        };
        let graph_saved = loaded.is_some();
        let vector = match (loaded, documents) {
            (Some(index), _) => Some(index),
            (None, Some(documents)) => {
                build_vector_index(vector_config.as_ref(), &store, &documents)?
            }
            (None, None) => None,
        };

        Ok(Self {
            store,
            vector_config,
            state: RwLock::new(DerivedState {
                text,
                vector,
                generation,
            }),
            graph_dir: temp_dir.is_none().then_some(graph_dir),
            graph_saved: AtomicBool::new(graph_saved),
            _temp_dir: temp_dir,
        })
    }

    /// Vuelca el grafo ANN a disco si cambió desde la última carga o volcado,
    /// para que la próxima apertura no tenga que reconstruirlo. Se llama sola
    /// al cerrar; es seguro llamarla antes. Los índices en RAM no se vuelcan.
    pub fn persist_vector_graph(&self) -> crate::Result<()> {
        let (Some(graph_dir), Some(config)) =
            (self.graph_dir.as_ref(), self.vector_config.as_ref())
        else {
            return Ok(());
        };
        if self.graph_saved.load(Ordering::Acquire) {
            return Ok(());
        }
        let state = self.state.read().unwrap();
        let Some(vector) = state.vector.as_ref() else {
            return Ok(());
        };
        if state.generation != self.store.generation()? {
            return Ok(());
        }
        vector.dump(graph_dir, state.generation, &config.space_id)?;
        self.graph_saved.store(true, Ordering::Release);
        Ok(())
    }

    pub fn upsert(&self, doc: &IndexDoc) -> crate::Result<()> {
        self.upsert_batch(std::slice::from_ref(doc))
    }

    pub fn upsert_batch(&self, docs: &[IndexDoc]) -> crate::Result<()> {
        validate_documents(docs, self.vector_config.as_ref())?;
        if docs.is_empty() {
            return Ok(());
        }
        let mut state = self.state.write().unwrap();
        let (generation, view) = self.store.upsert_batch(docs)?;
        let update = state
            .text
            .upsert_batch(docs)
            .and_then(|()| sync_vectors(state.vector.as_ref(), view, docs));
        self.finish_update(&mut state, generation, update)
    }

    pub fn delete(&self, id: &str) -> crate::Result<()> {
        let mut state = self.state.write().unwrap();
        let generation = self.store.delete_ids(&[id.to_string()])?;
        let update = state
            .text
            .delete_doc(id)
            .and_then(|()| match state.vector.as_ref() {
                Some(vector) => vector.delete(id),
                None => Ok(()),
            });
        self.finish_update(&mut state, generation, update)
    }

    pub fn delete_by_filter(&self, filter: &ScalarFilter) -> crate::Result<()> {
        let mut state = self.state.write().unwrap();
        let ids = state.text.ids_by_filter(filter)?;
        if ids.is_empty() {
            return Ok(());
        }
        let generation = self.store.delete_ids(&ids)?;
        let update = (|| {
            if let Some(vector) = state.vector.as_ref() {
                for id in &ids {
                    vector.delete(id)?;
                }
            }
            state.text.delete_by_filter(filter)
        })();
        self.finish_update(&mut state, generation, update)
    }

    pub fn clear(&self) -> crate::Result<()> {
        let mut state = self.state.write().unwrap();
        let generation = self.store.clear()?;
        let update = state.text.clear().map(|()| {
            // El almacén ya cambió a un fichero de vectores vacío.
            if let (Some(config), Some(view)) = (self.vector_config.as_ref(), self.store.view()) {
                state.vector = Some(VectorIndex::new(config.dimension, view));
            }
        });
        self.finish_update(&mut state, generation, update)
    }

    pub fn compact(&self) -> crate::Result<()> {
        let mut state = self.state.write().unwrap();
        self.store.compact_vectors()?;
        self.rebuild_state(&mut state)
    }

    pub fn query_hybrid(&self, query: HybridQuery) -> crate::Result<Vec<Hit>> {
        if query.k == 0 {
            return Err(crate::IndexError::InvalidVector(
                "k must be greater than zero".into(),
            ));
        }
        if let Some(vector) = query.vector.as_ref() {
            let config = self
                .vector_config
                .as_ref()
                .ok_or(crate::IndexError::VectorIndexDisabled)?;
            validate_vector(vector, config.dimension)?;
        }
        self.ensure_synced()?;
        let state = self.state.read().unwrap();
        let boosts = query.boosts.unwrap_or_default();

        // En una consulta híbrida cada fuente aporta más candidatos que `k` antes de fusionar:
        // con RRF (que solo mira posiciones), un documento que es 2.º en una lista y 1.º en
        // la otra debe poder ganar a los que solo son 1.º en una; recortando cada lista a
        // `k` ese documento se perdería (con `k = 1`, cada fuente aporta solo su mejor).
        let depth = if query.text.is_some() && query.vector.is_some() {
            query
                .k
                .saturating_mul(FUSION_DEPTH_FACTOR)
                .max(FUSION_MIN_DEPTH)
        } else {
            query.k
        };

        let text_ranking = match &query.text {
            Some(text) => Some(state.text.search(text, &query.filters, boosts, depth)?),
            None => None,
        };
        let vector_ranking = match &query.vector {
            Some(vector) if query.filters.is_empty() => Some(
                state
                    .vector
                    .as_ref()
                    .ok_or(crate::IndexError::VectorIndexDisabled)?
                    .search(vector, depth, query.ef_search)?,
            ),
            Some(vector) => Some(self.search_vector_filtered_exact(
                &state.text,
                vector,
                &query.filters,
                depth,
            )?),
            None => None,
        };

        Ok(merge_rankings(
            text_ranking,
            vector_ranking,
            query.fusion,
            query.k,
        ))
    }

    /// Búsqueda vectorial sin filtros que devuelve también la ruta del HNSW.
    pub fn trace_vector(
        &self,
        vector: &[f32],
        k: usize,
        ef_search: Option<usize>,
    ) -> crate::Result<crate::hnsw::VectorTrace> {
        let config = self
            .vector_config
            .as_ref()
            .ok_or(crate::IndexError::VectorIndexDisabled)?;
        validate_vector(vector, config.dimension)?;
        self.ensure_synced()?;
        let state = self.state.read().unwrap();
        state
            .vector
            .as_ref()
            .ok_or(crate::IndexError::VectorIndexDisabled)?
            .search_traced(vector, k, ef_search)
    }

    fn search_vector_filtered_exact(
        &self,
        text: &TextIndex,
        query: &[f32],
        filters: &[ScalarFilter],
        k: usize,
    ) -> crate::Result<Vec<(String, usize, f32)>> {
        let ids = text.ids_by_filters(filters)?;
        let mut scores: Vec<(String, f32)> = self
            .store
            .load_vectors(&ids)?
            .into_iter()
            .map(|(id, vector)| {
                let score = cosine_similarity(query, &vector);
                (id, score)
            })
            .collect();
        scores.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        Ok(scores
            .into_iter()
            .take(k)
            .enumerate()
            .map(|(rank, (id, score))| (id, rank + 1, score))
            .collect())
    }

    fn finish_update(
        &self,
        state: &mut DerivedState,
        generation: u64,
        update: crate::Result<()>,
    ) -> crate::Result<()> {
        self.graph_saved.store(false, Ordering::Release);
        if let Err(update_error) = update {
            if let Err(rebuild_error) = self.rebuild_state(state) {
                return Err(crate::IndexError::IndexUnavailableAfterCommit {
                    generation,
                    cause: format!(
                        "update failed: {update_error}; rebuild failed: {rebuild_error}"
                    ),
                });
            }
            return Ok(());
        }
        state.generation = generation;
        if state
            .vector
            .as_ref()
            .is_some_and(VectorIndex::should_compact)
        {
            // Compactar es un mantenimiento: si falla, los vectores muertos
            // simplemente siguen ocupando sitio hasta el próximo intento.
            if let Ok(Some(compacted)) = self.store.compact_vectors() {
                state.generation = compacted;
                self.rebuild_vector(state)?;
            }
        }
        Ok(())
    }

    fn ensure_synced(&self) -> crate::Result<()> {
        let generation = self.store.generation()?;
        if self.state.read().unwrap().generation == generation {
            return Ok(());
        }
        let mut state = self.state.write().unwrap();
        if state.generation != generation {
            self.rebuild_state(&mut state)?;
        }
        Ok(())
    }

    fn rebuild_state(&self, state: &mut DerivedState) -> crate::Result<()> {
        self.graph_saved.store(false, Ordering::Release);
        let documents = self.store.load_all()?;
        state.text.clear()?;
        state
            .text
            .upsert_batch(&documents.iter().map(|d| d.doc.clone()).collect::<Vec<_>>())?;
        state.vector = build_vector_index(self.vector_config.as_ref(), &self.store, &documents)?;
        state.generation = self.store.generation()?;
        Ok(())
    }

    fn rebuild_vector(&self, state: &mut DerivedState) -> crate::Result<()> {
        self.graph_saved.store(false, Ordering::Release);
        let documents = self.store.load_all()?;
        state.vector = build_vector_index(self.vector_config.as_ref(), &self.store, &documents)?;
        Ok(())
    }

    /// Marca el índice de texto como al día con la generación actual para que
    /// la próxima apertura no lo reconstruya. Solo se llama al cerrar.
    fn persist_text_marker(&self) -> crate::Result<()> {
        let Some(graph_dir) = self.graph_dir.as_ref() else {
            return Ok(());
        };
        let state = self.state.read().unwrap();
        if state.generation != self.store.generation()? {
            return Ok(());
        }
        let base = graph_dir.parent().unwrap_or(graph_dir);
        let marker = base.join(FTS_MARKER_FILE);
        let temp = marker.with_extension("tmp");
        let mut content = state.generation.to_le_bytes().to_vec();
        content.extend_from_slice(&FTS_ANALYSIS_VERSION.to_le_bytes());
        std::fs::write(&temp, content)?;
        std::fs::rename(&temp, &marker)?;
        Ok(())
    }

    /// `true` si el volcado del grafo ANN en disco coincide con el de memoria
    /// (se restauró al abrir o se volcó desde entonces).
    pub fn vector_graph_persisted(&self) -> bool {
        self.graph_saved.load(Ordering::Acquire)
    }

    pub fn vector_dimension(&self) -> Option<usize> {
        self.vector_config.as_ref().map(|config| config.dimension)
    }

    pub fn vector_stats(&self) -> Option<(usize, usize)> {
        self.state
            .read()
            .unwrap()
            .vector
            .as_ref()
            .map(VectorIndex::stats)
    }
}

impl Drop for SemanticIndex {
    fn drop(&mut self) {
        // Mejor esfuerzo: si falla, la próxima apertura reconstruye.
        let _ = self.persist_vector_graph();
        let _ = self.persist_text_marker();
    }
}

/// Generación con la que se cerró el índice de texto, si el marcador es válido y se escribió
/// con la versión de análisis actual.
fn read_fts_marker(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let bytes: [u8; 12] = bytes.try_into().ok()?;
    let version = u32::from_le_bytes(bytes[8..].try_into().ok()?);
    (version == FTS_ANALYSIS_VERSION).then(|| u64::from_le_bytes(bytes[..8].try_into().unwrap()))
}

fn validate_config(config: Option<&VectorConfig>) -> crate::Result<()> {
    let Some(config) = config else {
        return Ok(());
    };
    if config.dimension == 0 || config.dimension > MAX_VECTOR_DIMENSION {
        return Err(crate::IndexError::InvalidVector(format!(
            "dimension must be in 1..={MAX_VECTOR_DIMENSION}, got {}",
            config.dimension
        )));
    }
    if config.space_id.trim().is_empty() {
        return Err(crate::IndexError::VectorSpaceMismatch(
            "space_id must not be empty".into(),
        ));
    }
    Ok(())
}

fn validate_documents(docs: &[IndexDoc], config: Option<&VectorConfig>) -> crate::Result<()> {
    for doc in docs {
        if let Some(vector) = doc.vector.as_ref() {
            let config = config.ok_or(crate::IndexError::VectorIndexDisabled)?;
            validate_vector(vector, config.dimension)?;
        }
    }
    Ok(())
}

fn resolve_database_meta(base: &Path, vector: Option<&VectorConfig>) -> crate::Result<()> {
    let path = base.join(DATABASE_META_FILE);
    let mut requested = DatabaseMeta {
        schema_version: SCHEMA_VERSION,
        metric: "cosine".into(),
        vector: vector.cloned(),
        vector_dimension: None,
    };
    if path.exists() {
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(corrupt_meta)?;
        if raw.get("schema_version").is_some() {
            let stored: DatabaseMeta = serde_json::from_value(raw).map_err(corrupt_meta)?;
            if !stored.same_space(&requested) {
                return Err(crate::IndexError::VectorSpaceMismatch(format!(
                    "stored configuration {stored:?} does not match requested {requested:?}"
                )));
            }
            return Ok(());
        }
        // `meta.json` de 0.3.x: sólo `{"vector_dimension": N}`. Esa versión no
        // guardaba los documentos del índice en ningún almacén autoritativo
        // —Tantivy sólo conserva `id` y `filters`—, así que no hay nada que
        // traer: el índice semántico arranca vacío y el consumidor reindexa.
        // Colecciones y log no dependen de este archivo.
        let legacy_dimension = raw
            .get("vector_dimension")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| corrupt_meta("missing schema_version"))?;
        requested.vector_dimension = Some(legacy_dimension as usize);
    }
    write_database_meta(&path, &requested)
}

/// Escribe `meta.json` vía archivo temporal + rename: migrar una base existente
/// reescribe el archivo, y uno a medio escribir la dejaría sin poder abrirse.
fn write_database_meta(path: &Path, meta: &DatabaseMeta) -> crate::Result<()> {
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(meta)?)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn corrupt_meta(cause: impl std::fmt::Display) -> crate::IndexError {
    crate::IndexError::Storage(format!("corrupt {DATABASE_META_FILE}: {cause}"))
}

fn build_vector_index(
    config: Option<&VectorConfig>,
    store: &SemanticStore,
    documents: &[StoredDoc],
) -> crate::Result<Option<VectorIndex>> {
    let (Some(config), Some(view)) = (config, store.view()) else {
        return Ok(None);
    };
    let slot_ids = store.slot_ids(documents)?;
    Ok(Some(VectorIndex::build(config.dimension, view, slot_ids)?))
}

/// Refleja la tanda en el grafo. Las ranuras nuevas (una por documento con
/// vector, en orden) se enlazan de golpe; un documento cuya última aparición en
/// la tanda no trae vector pierde el suyo.
fn sync_vectors(
    vector: Option<&VectorIndex>,
    view: Option<Arc<View>>,
    docs: &[IndexDoc],
) -> crate::Result<()> {
    let (Some(index), Some(view)) = (vector, view) else {
        return if docs.iter().any(|doc| doc.vector.is_some()) {
            Err(crate::IndexError::VectorIndexDisabled)
        } else {
            Ok(())
        };
    };
    let with_vector: Vec<&str> = docs
        .iter()
        .filter(|doc| doc.vector.is_some())
        .map(|doc| doc.id.as_str())
        .collect();
    if !with_vector.is_empty() {
        index.add_slots(view, &with_vector)?;
    }
    let mut last: HashMap<&str, bool> = HashMap::new();
    for doc in docs {
        last.insert(doc.id.as_str(), doc.vector.is_some());
    }
    for (id, has_vector) in last {
        if !has_vector {
            index.delete(id)?;
        }
    }
    Ok(())
}

fn merge_rankings(
    text: Option<Vec<(String, usize, f32)>>,
    vector: Option<Vec<(String, usize, f32)>>,
    fusion: Fusion,
    k: usize,
) -> Vec<Hit> {
    match (text, vector) {
        (Some(text), None) => text
            .into_iter()
            .map(|(id, _, score)| Hit {
                id,
                score,
                text_score: Some(score),
                vector_score: None,
            })
            .collect(),
        (None, Some(vector)) => vector
            .into_iter()
            .map(|(id, _, score)| Hit {
                id,
                score,
                text_score: None,
                vector_score: Some(score),
            })
            .collect(),
        (Some(text), Some(vector)) => {
            let text_scores: HashMap<&str, f32> = text
                .iter()
                .map(|(id, _, score)| (id.as_str(), *score))
                .collect();
            let vector_scores: HashMap<&str, f32> = vector
                .iter()
                .map(|(id, _, score)| (id.as_str(), *score))
                .collect();
            let rankings = vec![
                text.iter()
                    .map(|(id, rank, _)| (id.clone(), *rank))
                    .collect(),
                vector
                    .iter()
                    .map(|(id, rank, _)| (id.clone(), *rank))
                    .collect(),
            ];
            let Fusion::Rrf { k: fusion_k } = fusion;
            rrf(&rankings, fusion_k)
                .into_iter()
                .take(k)
                .map(|(id, score)| Hit {
                    text_score: text_scores.get(id.as_str()).copied(),
                    vector_score: vector_scores.get(id.as_str()).copied(),
                    id,
                    score,
                })
                .collect()
        }
        (None, None) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_gap_is_rebuilt_before_query() {
        let index = SemanticIndex::open_in_ram(Some(VectorConfig::new(8, "test:8"))).unwrap();
        let mut vector = vec![0.0; 8];
        vector[2] = 1.0;
        let doc = IndexDoc::new("committed")
            .with_body("confirmado antes del fallo")
            .with_vector(vector.clone());

        // Simula un proceso que confirmó redb y se detuvo antes de actualizar
        // los índices derivados.
        index.store.upsert_batch(&[doc]).unwrap();
        let hits = index
            .query_hybrid(
                HybridQuery::default()
                    .with_text("confirmado")
                    .with_vector(vector)
                    .with_k(1),
            )
            .unwrap();
        assert_eq!(hits[0].id, "committed");
    }
}
