use crate::clock::{Clock, SystemClock, into_clock};
use crate::collections::{ColOp, Collections, DocEntry, PutOptions, ScanOptions};
use crate::error::HiveResult;
use crate::event::{AgentId, Event, EventInput, EventKind, StreamId};
use crate::log::EventLog;
use crate::memory::WorkingMemory;
#[cfg(any(test, loom))]
use crate::memory_log::MemoryEventLog;
use crate::projection::{Projection, ProjectionRegistry};
use crate::reactive::{EventPattern, ReactiveEngine, Subscription};
use crate::state::{
    consent_graph::ConsentGraph,
    current_facts::CurrentFacts,
    task_state::TaskState,
    tool_ledger::{ToolLedger, ToolStats},
};
use hivedb_index::{
    EmbedKind, Embedder, Hit, HybridQuery, IndexDoc, ScalarFilter, SemanticIndex, VectorConfig,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use hivedb_index::VectorConfig as VectorOptions;

/// Options for opening a database.
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    /// Identidad explícita del espacio vectorial. `None` activa modo solo texto,
    /// salvo que haya un `embedder`, que fija el espacio por sí mismo.
    pub vector: Option<VectorOptions>,
    /// Generador de embeddings. Con él, los documentos sin vector se embeben a
    /// partir de su texto y las consultas de texto buscan también por vector.
    /// Los vectores aportados explícitamente tienen siempre prioridad.
    pub embedder: Option<Arc<dyn Embedder>>,
}

/// Public handle to a HiveDB database.
///
/// All writes happen through [`HiveDB::append`]; the log is immutable once an
/// event receives a `seq`.
#[derive(Clone)]
pub struct HiveDB {
    log: LogHandle,
    working: Arc<WorkingMemory>,
    semantic: Option<Arc<SemanticIndex>>,
    embedder: Option<Arc<dyn Embedder>>,
    collections: Option<Arc<Collections>>,
    reactive: Arc<ReactiveEngine>,
    clock: Arc<dyn Clock>,
    base_path: PathBuf,
    /// Keeps ephemeral databases (`open_temp` / `":memory:"`) alive: the
    /// directory is removed from disk when the last clone drops. `None` for
    /// persistent databases.
    _temp_dir: Option<Arc<tempfile::TempDir>>,
}

/// Internal handle that hides whether the log is backed by sharded `redb`
/// files or by an in-memory structure used for concurrency model checking.
#[derive(Clone)]
enum LogHandle {
    Redb(Arc<EventLog>),
    #[cfg(any(test, loom))]
    Memory(Arc<MemoryEventLog>),
}

impl LogHandle {
    fn append(&self, input: EventInput) -> HiveResult<Event> {
        match self {
            LogHandle::Redb(log) => log.append(input),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.append(input),
        }
    }

    fn read(&self, seq: u64) -> HiveResult<Event> {
        match self {
            LogHandle::Redb(log) => log.read(seq),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.read(seq),
        }
    }

    fn len(&self) -> HiveResult<u64> {
        match self {
            LogHandle::Redb(log) => log.len(),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.len(),
        }
    }

    fn last_seq(&self) -> HiveResult<u64> {
        match self {
            LogHandle::Redb(log) => log.last_seq(),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.len(),
        }
    }

    fn read_stream(&self, agent_id: &AgentId, stream_id: &StreamId) -> HiveResult<Vec<Event>> {
        match self {
            LogHandle::Redb(log) => log.read_stream(agent_id, stream_id),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.read_stream(agent_id, stream_id),
        }
    }

    fn read_stream_all_agents(&self, stream_id: &StreamId) -> HiveResult<Vec<Event>> {
        match self {
            LogHandle::Redb(log) => log.read_stream_all_agents(stream_id),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.read_stream_all_agents(stream_id),
        }
    }

    fn read_stream_for_agents(
        &self,
        agents: &[AgentId],
        stream_id: &StreamId,
    ) -> HiveResult<Vec<Event>> {
        match self {
            LogHandle::Redb(log) => log.read_stream_for_agents(agents, stream_id),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.read_stream_for_agents(agents, stream_id),
        }
    }

    fn project<P: Projection>(&self) -> HiveResult<P::State> {
        match self {
            LogHandle::Redb(log) => log.project::<P>(),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.project::<P>(),
        }
    }

    fn project_for_agents<P: Projection>(&self, agents: &[AgentId]) -> HiveResult<P::State> {
        match self {
            LogHandle::Redb(log) => log.project_for_agents::<P>(agents),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.project::<P>(),
        }
    }

    fn projection_checkpoint<P: Projection>(&self) -> HiveResult<u64> {
        match self {
            LogHandle::Redb(log) => log.projection_checkpoint::<P>(),
            #[cfg(any(test, loom))]
            LogHandle::Memory(_) => Ok(0),
        }
    }

    fn wipe_projections_and_rebuild(&self) -> HiveResult<()> {
        match self {
            LogHandle::Redb(log) => log.wipe_projections_and_rebuild(),
            #[cfg(any(test, loom))]
            LogHandle::Memory(log) => log.wipe_projections_and_rebuild(),
        }
    }

    fn flush_next_seq(&self) -> HiveResult<()> {
        match self {
            LogHandle::Redb(log) => log.flush_next_seq(),
            #[cfg(any(test, loom))]
            LogHandle::Memory(_) => Ok(()),
        }
    }
}

impl HiveDB {
    /// Open a database at the given path, creating it if necessary.
    pub fn open<P: AsRef<Path>>(path: P) -> HiveResult<Self> {
        Self::open_with_options(path, OpenOptions::default())
    }

    /// Open a database with explicit options.
    pub fn open_with_options<P: AsRef<Path>>(path: P, options: OpenOptions) -> HiveResult<Self> {
        Self::open_with_clock_and_options(path, into_clock(SystemClock), options)
    }

    /// Open a database with an explicit clock source.
    #[doc(hidden)]
    pub fn open_with_clock<P: AsRef<Path>>(path: P, clock: Arc<dyn Clock>) -> HiveResult<Self> {
        Self::open_with_clock_and_options(path, clock, OpenOptions::default())
    }

    /// Open a database with an explicit clock source and options.
    #[doc(hidden)]
    pub fn open_with_clock_and_options<P: AsRef<Path>>(
        path: P,
        clock: Arc<dyn Clock>,
        options: OpenOptions,
    ) -> HiveResult<Self> {
        let base = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&base)?;

        let registry = default_registry();
        let log = LogHandle::Redb(Arc::new(EventLog::open(&base, registry, clock.clone())?));
        let working = Arc::new(WorkingMemory::new());
        let vector = resolve_vector_options(&options)?;
        let semantic = Some(Arc::new(SemanticIndex::open(&base, vector)?));
        let collections = Some(Arc::new(Collections::open(&base)?));
        let reactive = Arc::new(ReactiveEngine::new());

        Ok(Self {
            log,
            working,
            semantic,
            embedder: options.embedder,
            collections,
            reactive,
            clock,
            base_path: base,
            _temp_dir: None,
        })
    }

    /// Open a temporary database. The data directory is removed from disk
    /// when the last clone of the handle drops.
    pub fn open_temp() -> HiveResult<Self> {
        Self::open_temp_with_clock(into_clock(SystemClock))
    }

    /// Open an in-memory database for concurrency model checking.
    ///
    /// The semantic index still uses a temporary directory because the index
    /// layer is not loom-aware.
    #[cfg(any(test, loom))]
    #[doc(hidden)]
    pub fn open_in_memory() -> HiveResult<Self> {
        Self::open_in_memory_with_clock(into_clock(SystemClock))
    }

    /// Open an in-memory database with an explicit clock source.
    #[cfg(any(test, loom))]
    #[doc(hidden)]
    pub fn open_in_memory_with_clock(clock: Arc<dyn Clock>) -> HiveResult<Self> {
        let dir = tempfile::tempdir()?;
        let base = dir.path().to_path_buf();

        let log = LogHandle::Memory(Arc::new(MemoryEventLog::new(clock.clone())));
        let working = Arc::new(WorkingMemory::new());
        let semantic = None;
        let collections = None;
        let reactive = Arc::new(ReactiveEngine::new());

        Ok(Self {
            log,
            working,
            semantic,
            embedder: None,
            collections,
            reactive,
            clock,
            base_path: base,
            _temp_dir: Some(Arc::new(dir)),
        })
    }

    /// Open a temporary database with an explicit clock source.
    #[doc(hidden)]
    pub fn open_temp_with_clock(clock: Arc<dyn Clock>) -> HiveResult<Self> {
        Self::open_temp_with_clock_and_options(clock, OpenOptions::default())
    }

    /// Open a temporary database with an explicit clock source and options.
    /// The backing directory is removed when the last clone drops.
    #[doc(hidden)]
    pub fn open_temp_with_clock_and_options(
        clock: Arc<dyn Clock>,
        options: OpenOptions,
    ) -> HiveResult<Self> {
        let dir = tempfile::tempdir()?;
        let mut db = Self::open_with_clock_and_options(dir.path(), clock, options)?;
        db._temp_dir = Some(Arc::new(dir));
        Ok(db)
    }

    /// Open a temporary database with explicit options.
    pub fn open_temp_with_options(options: OpenOptions) -> HiveResult<Self> {
        Self::open_temp_with_clock_and_options(into_clock(SystemClock), options)
    }

    /// Advance the clock to `timestamp_ms`.
    ///
    /// For test clocks (e.g. [`MockClock`]) this moves time forward. For the
    /// system clock it is a no-op.
    #[doc(hidden)]
    pub fn advance_clock_to(&self, timestamp_ms: u64) {
        self.clock.advance_clock_to(timestamp_ms);
    }

    /// Append a new event to the log.
    ///
    /// Returns the engine-assigned global sequence number.
    pub fn append(&self, input: EventInput) -> HiveResult<u64> {
        let event = self.log.append(input)?;
        self.reactive.dispatch(&event);
        Ok(event.seq)
    }

    /// Read a single event by sequence number.
    pub fn read(&self, seq: u64) -> HiveResult<Event> {
        self.log.read(seq)
    }

    /// Query the current state of a projection.
    pub fn project<P: Projection>(&self) -> HiveResult<P::State> {
        self.log.project::<P>()
    }

    /// Returns the last sequence number applied to a projection.
    pub fn projection_checkpoint<P: Projection>(&self) -> HiveResult<u64> {
        self.log.projection_checkpoint::<P>()
    }

    /// Returns aggregated statistics for a tool, if any `ToolCall` events have
    /// been recorded for it.
    /// Recorre los shards de TODA la base. `ToolLedger` tiene scope `Agent`, así
    /// que sobre una base compartida por varios inquilinos esto suma las
    /// llamadas de todos: usar [`HiveDB::tool_stats_for_agents`] en ese caso.
    pub fn tool_stats(&self, tool: &str) -> HiveResult<Option<ToolStats>> {
        let state = self.log.project::<ToolLedger>()?;
        Ok(state.get(tool).cloned())
    }

    /// Estadísticas de una herramienta, restringidas a un conjunto de agentes.
    ///
    /// La variante que necesita un consumidor multi-inquilino: sin la lista, la
    /// proyección mezcla el estado de todos los shards de la base y devuelve
    /// agregados que cruzan enjambres.
    pub fn tool_stats_for_agents(
        &self,
        tool: &str,
        agents: &[AgentId],
    ) -> HiveResult<Option<ToolStats>> {
        let state = self.log.project_for_agents::<ToolLedger>(agents)?;
        Ok(state.get(tool).cloned())
    }

    /// Store a value in working memory with an optional TTL.
    pub fn working_set(
        &self,
        agent_id: impl Into<AgentId>,
        key: impl Into<String>,
        value: Value,
        ttl: Option<Duration>,
    ) {
        self.working.set(agent_id.into(), key.into(), value, ttl);
    }

    /// Retrieve a value from working memory, returning `None` if expired.
    pub fn working_get(&self, agent_id: impl Into<AgentId>, key: &str) -> Option<Value> {
        self.working.get(&agent_id.into(), key)
    }

    /// Return all non-expired keys for an agent.
    pub fn working_keys(&self, agent_id: impl Into<AgentId>) -> Vec<String> {
        self.working.keys(&agent_id.into())
    }

    fn semantic(&self) -> HiveResult<&SemanticIndex> {
        self.semantic.as_deref().ok_or_else(|| {
            crate::error::HiveError::InvalidInput(
                "semantic index not available in in-memory mode".into(),
            )
        })
    }

    /// Insert or replace a document in the semantic index.
    pub fn upsert_doc(&self, doc: &IndexDoc) -> HiveResult<()> {
        self.upsert_batch(std::slice::from_ref(doc))
    }

    /// Insert or replace a batch of documents under a single text-index
    /// commit.
    ///
    /// Con un `embedder` configurado, los documentos sin vector pero con texto
    /// se embeben aquí, en un único lote.
    pub fn upsert_batch(&self, docs: &[IndexDoc]) -> HiveResult<()> {
        let semantic = self.semantic()?;
        match self.embed_missing_vectors(docs)? {
            Some(embedded) => semantic.upsert_batch(&embedded),
            None => semantic.upsert_batch(docs),
        }
        .map_err(Into::into)
    }

    /// Genera el vector de los documentos que tienen texto y no traen vector.
    /// Devuelve `None` si no hay nada que embeber (sin embedder, o todos los
    /// documentos ya traen vector o no tienen texto).
    fn embed_missing_vectors(&self, docs: &[IndexDoc]) -> HiveResult<Option<Vec<IndexDoc>>> {
        let Some(embedder) = self.embedder.as_ref() else {
            return Ok(None);
        };
        let pending: Vec<(usize, String)> = docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.vector.is_none())
            .filter_map(|(i, doc)| document_text(doc).map(|text| (i, text)))
            .collect();
        if pending.is_empty() {
            return Ok(None);
        }
        let texts: Vec<&str> = pending.iter().map(|(_, text)| text.as_str()).collect();
        let vectors = embedder.embed(&texts, EmbedKind::Document)?;
        if vectors.len() != pending.len() {
            return Err(hivedb_index::IndexError::Embedder(format!(
                "el embedder devolvió {} vectores para {} textos",
                vectors.len(),
                pending.len()
            ))
            .into());
        }
        let mut embedded = docs.to_vec();
        for ((i, _), vector) in pending.into_iter().zip(vectors) {
            embedded[i].vector = Some(vector);
        }
        Ok(Some(embedded))
    }

    /// Delete a document from the semantic index. Missing ids are a no-op.
    pub fn delete_doc(&self, id: &str) -> HiveResult<()> {
        self.semantic()?.delete(id).map_err(Into::into)
    }

    /// Delete every indexed document carrying the given scalar filter.
    pub fn delete_by_filter(&self, filter: &ScalarFilter) -> HiveResult<()> {
        self.semantic()?
            .delete_by_filter(filter)
            .map_err(Into::into)
    }

    /// Remove every document from the semantic index.
    pub fn clear_index(&self) -> HiveResult<()> {
        self.semantic()?.clear().map_err(Into::into)
    }

    /// Reconstruye ambos índices semánticos desde los documentos autoritativos.
    pub fn compact_index(&self) -> HiveResult<()> {
        self.semantic()?.compact().map_err(Into::into)
    }

    fn collections(&self) -> HiveResult<&Collections> {
        self.collections.as_deref().ok_or_else(|| {
            crate::error::HiveError::InvalidInput(
                "collections not available in in-memory mode".into(),
            )
        })
    }

    /// Insert or replace a JSON document in a collection. Returns the new
    /// version (starts at 1).
    pub fn col_put(
        &self,
        collection: &str,
        id: &str,
        doc: &Value,
        options: PutOptions,
    ) -> HiveResult<u64> {
        self.collections()?.put(collection, id, doc, options)
    }

    /// Read a document by id.
    pub fn col_get(&self, collection: &str, id: &str) -> HiveResult<Option<DocEntry>> {
        self.collections()?.get(collection, id)
    }

    /// Delete a document. Returns `true` if it existed.
    pub fn col_delete(&self, collection: &str, id: &str) -> HiveResult<bool> {
        self.collections()?.delete(collection, id)
    }

    /// Scan a collection in id order.
    pub fn col_scan(&self, collection: &str, options: &ScanOptions) -> HiveResult<Vec<DocEntry>> {
        self.collections()?.scan(collection, options)
    }

    /// Number of documents in a collection.
    pub fn col_count(&self, collection: &str) -> HiveResult<u64> {
        self.collections()?.count(collection)
    }

    /// Create an equality index on a top-level field (optionally unique).
    pub fn col_create_index(&self, collection: &str, field: &str, unique: bool) -> HiveResult<()> {
        self.collections()?.create_index(collection, field, unique)
    }

    /// Look up documents whose indexed field equals `value`.
    pub fn col_find_by(
        &self,
        collection: &str,
        field: &str,
        value: &Value,
        options: &ScanOptions,
    ) -> HiveResult<Vec<DocEntry>> {
        self.collections()?
            .find_by(collection, field, value, options)
    }

    /// Apply several puts/deletes atomically across collections.
    pub fn col_batch(&self, ops: &[ColOp]) -> HiveResult<()> {
        self.collections()?.batch(ops)
    }

    /// Index a document for hybrid search.
    ///
    /// Deprecated shim over [`HiveDB::upsert_doc`]: `text` maps to the `body`
    /// field.
    pub fn index_doc(
        &self,
        id: impl Into<String>,
        text: impl Into<String>,
        vector: Vec<f32>,
    ) -> HiveResult<()> {
        self.index_doc_with(id, text, vector, &[])
    }

    /// Index a document with scalar filters for hybrid search.
    ///
    /// Deprecated shim over [`HiveDB::upsert_doc`]: `text` maps to the `body`
    /// field.
    pub fn index_doc_with(
        &self,
        id: impl Into<String>,
        text: impl Into<String>,
        vector: Vec<f32>,
        filters: &[ScalarFilter],
    ) -> HiveResult<()> {
        let doc = IndexDoc::new(id)
            .with_body(text)
            .with_vector(vector)
            .with_filters(filters.to_vec());
        self.upsert_doc(&doc)
    }

    /// Execute a hybrid search query.
    ///
    /// Con un `embedder` configurado, una consulta de texto sin vector también
    /// busca por vector, por lo que el resultado pasa a ser una fusión RRF.
    pub fn query_hybrid(&self, mut query: HybridQuery) -> HiveResult<Vec<Hit>> {
        let semantic = self.semantic()?;
        let to_embed = match (self.embedder.as_ref(), query.vector.as_ref()) {
            (Some(embedder), None) => query
                .text
                .as_deref()
                .filter(|text| !text.trim().is_empty())
                .map(|text| (embedder, text.to_string())),
            _ => None,
        };
        if let Some((embedder, text)) = to_embed {
            query.vector = embedder.embed(&[text.as_str()], EmbedKind::Query)?.pop();
        }
        semantic.query_hybrid(query).map_err(Into::into)
    }

    /// Búsqueda vectorial (sin filtros) que devuelve además la ruta que
    /// recorrió el HNSW. Herramienta de diagnóstico y visualización; la ruta
    /// normal de `query_hybrid` no registra nada.
    pub fn trace_vector_search(
        &self,
        vector: &[f32],
        k: usize,
        ef_search: Option<usize>,
    ) -> HiveResult<hivedb_index::VectorTrace> {
        self.semantic()?
            .trace_vector(vector, k, ef_search)
            .map_err(Into::into)
    }

    /// Subscribe to a pattern of events.
    pub fn subscribe(&self, pattern: EventPattern) -> Subscription {
        self.reactive.subscribe(pattern)
    }

    /// Wipe all materialized projection state and rebuild it from the log.
    #[doc(hidden)]
    pub fn wipe_projections_and_rebuild(&self) -> HiveResult<()> {
        self.log.wipe_projections_and_rebuild()
    }

    /// Number of events currently stored in the log.
    pub fn log_len(&self) -> HiveResult<u64> {
        self.log.len()
    }

    /// Returns the highest assigned sequence number, or 0 if the log is empty.
    pub fn last_seq(&self) -> HiveResult<u64> {
        self.log.last_seq()
    }

    /// Read all events for a given agent/stream in ascending order.
    pub fn read_stream(&self, agent_id: &AgentId, stream_id: &StreamId) -> HiveResult<Vec<Event>> {
        self.log.read_stream(agent_id, stream_id)
    }

    /// Returns the base path of the database.
    pub fn path(&self) -> &Path {
        &self.base_path
    }

    /// Returns true if `agent` is authorized to perform `action` on `resource`
    /// according to the current consent graph.
    ///
    /// This method appends an `IntentLogged` event to the log for audit
    /// purposes. The returned `Decision` contains the sequence number of that
    /// event and, if allowed, the grant that authorized it.
    pub fn can(
        &self,
        agent: impl Into<AgentId>,
        action: impl Into<String>,
        resource: impl Into<String>,
    ) -> HiveResult<Decision> {
        let agent = agent.into();
        let action = action.into();
        let resource = resource.into();

        let state = self.log.project::<ConsentGraph>()?;
        let now = self.clock.now_ms();
        let authorized_by = state.find_active_grant(&agent, &action, &resource, now);

        let intent_seq = self.append(EventInput::new(
            agent.clone(),
            StreamId::from("consent"),
            EventKind::IntentLogged {
                actor: agent,
                intent: format!("{}:{}", action, resource),
                authorized_by,
            },
        ))?;

        Ok(Decision {
            allowed: authorized_by.is_some(),
            intent_log_seq: Some(intent_seq),
        })
    }

    /// Build the causal thread for a given stream.
    ///
    /// Recorre el log de TODA la base. Ver
    /// [`HiveDB::causal_thread_for_agents`] para el caso multi-inquilino.
    pub fn causal_thread(
        &self,
        stream_id: impl Into<StreamId>,
    ) -> HiveResult<crate::causal::CausalThread> {
        let stream_id = stream_id.into();
        let events = self.log.read_stream_all_agents(&stream_id)?;
        Ok(crate::causal::CausalThread::from_events(&events))
    }

    /// Hilo causal de un stream, restringido a un conjunto de agentes.
    ///
    /// Además de no cruzar inquilinos, el coste pasa de O(eventos de la base) a
    /// O(eventos de esos agentes) — que es lo que vuelve viable llamar a esto
    /// en cada turno cuando la base la comparten muchos enjambres.
    pub fn causal_thread_for_agents(
        &self,
        stream_id: impl Into<StreamId>,
        agents: &[AgentId],
    ) -> HiveResult<crate::causal::CausalThread> {
        let stream_id = stream_id.into();
        let events = self.log.read_stream_for_agents(agents, &stream_id)?;
        Ok(crate::causal::CausalThread::from_events(&events))
    }
}

impl Drop for HiveDB {
    fn drop(&mut self) {
        // Best-effort flush of the next sequence number so graceful shutdowns
        // avoid a full shard scan on the next open.
        let _ = self.log.flush_next_seq();
    }
}

/// Result of an authorization check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    allowed: bool,
    intent_log_seq: Option<u64>,
}

impl Decision {
    /// True if the action is authorized.
    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// Sequence number of the `IntentLogged` event recording this decision.
    pub fn intent_log_seq(&self) -> Option<u64> {
        self.intent_log_seq
    }
}

/// Espacio vectorial efectivo: el del embedder si lo hay (y debe coincidir con
/// `vector` si ambos se indican), o el explícito en otro caso.
fn resolve_vector_options(options: &OpenOptions) -> HiveResult<Option<VectorOptions>> {
    let Some(embedder) = options.embedder.as_ref() else {
        return Ok(options.vector.clone());
    };
    let from_embedder = VectorConfig::new(embedder.dimension(), embedder.space_id());
    match options.vector.as_ref() {
        Some(explicit) if explicit != &from_embedder => {
            Err(hivedb_index::IndexError::VectorSpaceMismatch(format!(
                "el espacio vectorial indicado {explicit:?} no coincide con el del embedder {from_embedder:?}"
            ))
            .into())
        }
        _ => Ok(Some(from_embedder)),
    }
}

/// Texto que representa al documento ante el embedder: nombre, etiquetas y
/// cuerpo, en ese orden. `None` si no tiene texto.
fn document_text(doc: &IndexDoc) -> Option<String> {
    let parts: Vec<&str> = [&doc.name, &doc.tags, &doc.body]
        .into_iter()
        .filter_map(|part| part.as_deref())
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn default_registry() -> ProjectionRegistry {
    let mut registry = ProjectionRegistry::empty();
    registry.register::<CurrentFacts>();
    registry.register::<TaskState>();
    registry.register::<ConsentGraph>();
    registry.register::<ToolLedger>();
    registry
}

impl From<hivedb_index::IndexError> for crate::error::HiveError {
    fn from(e: hivedb_index::IndexError) -> Self {
        crate::error::HiveError::InvalidInput(e.to_string())
    }
}
