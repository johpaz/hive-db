//! Lógica común de los bindings de HiveDB (Node y Python).
//!
//! Aquí vive todo lo que no depende del lenguaje anfitrión: los DTOs planos de entrada y salida,
//! la validación de eventos, patrones y consultas, la apertura de la base, el candado de
//! lectura/escritura que permite consultas concurrentes y los errores con código estable. Cada
//! binding solo convierte sus tipos nativos a estos DTOs y de vuelta, de modo que ambos devuelven
//! los mismos resultados, los mismos mensajes y los mismos códigos.

use hivedb_core::{
    AgentContextRequest, AgentId, ColOp, EventInput, EventKind, EventKindTag, EventPattern,
    HarnessInput, HarnessLoop, HiveDB, OpenOptions, Predicate, PutOptions, ScanOptions, StreamId,
    VectorOptions,
};
use hivedb_index::{FieldBoosts, Fusion, HybridQuery, IndexDoc, ScalarFilter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Display;
use std::sync::{Arc, RwLock};

pub use hivedb_core::{Embedder, Subscription};

// ---------------------------------------------------------------------------------------------
// Errores
// ---------------------------------------------------------------------------------------------

/// Código estable de un error semántico. El resto de errores no llevan código.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    InvalidVector,
    VectorSpaceMismatch,
    IndexDegraded,
    EmbedderUnavailable,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 4] = [
        ErrorCode::InvalidVector,
        ErrorCode::VectorSpaceMismatch,
        ErrorCode::IndexDegraded,
        ErrorCode::EmbedderUnavailable,
    ];

    /// Nombre estable (`"INVALID_VECTOR"`…), el mismo que usa el binding Node como prefijo.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidVector => "INVALID_VECTOR",
            ErrorCode::VectorSpaceMismatch => "VECTOR_SPACE_MISMATCH",
            ErrorCode::IndexDegraded => "INDEX_DEGRADED",
            ErrorCode::EmbedderUnavailable => "EMBEDDER_UNAVAILABLE",
        }
    }
}

/// Error de un binding: el mensaje completo (con el prefijo `CODIGO:` cuando lo hay) y su código.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingError {
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl BindingError {
    /// Construye el error a partir del mensaje; detecta el código por su prefijo `CODIGO:`.
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let code = ErrorCode::ALL
            .into_iter()
            .find(|code| message.contains(&format!("{}:", code.as_str())));
        Self { code, message }
    }
}

impl Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BindingError {}

pub type Result<T> = std::result::Result<T, BindingError>;

/// Convierte cualquier error mostrable en `BindingError`.
pub fn err<E: Display>(error: E) -> BindingError {
    BindingError::new(error.to_string())
}

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(BindingError::new(message))
}

#[cfg(not(feature = "embedder-local"))]
const NO_EMBEDDER_MESSAGE: &str = "EMBEDDER_UNAVAILABLE: this build was compiled without the local embedder \
     (feature `embedder-local`)";

// ---------------------------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInputDto {
    pub agent_id: String,
    pub stream_id: String,
    pub kind: String,
    #[serde(default = "null_value")]
    pub payload: Value,
    #[serde(default)]
    pub causation: Option<u64>,
    /// UUID. Enlaza eventos del mismo objetivo/intención.
    #[serde(default)]
    pub correlation: Option<String>,
}

fn null_value() -> Value {
    Value::Null
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDto {
    pub seq: u64,
    pub agent_id: String,
    pub stream_id: String,
    pub kind_tag: String,
    pub timestamp: u64,
    pub causation: Option<u64>,
    pub correlation: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionDto {
    pub allowed: bool,
    pub intent_log_seq: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalarFilterDto {
    pub field: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionDto {
    /// Solo `"rrf"`.
    pub kind: String,
    #[serde(default)]
    pub k: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldBoostsDto {
    #[serde(default)]
    pub name: Option<f64>,
    #[serde(default)]
    pub body: Option<f64>,
    #[serde(default)]
    pub tags: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatsDto {
    pub invocations: u64,
    pub errors: u64,
    pub total_latency_ms: u64,
    pub total_cost: f64,
    pub last_outcome: Option<String>,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridQueryDto {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
    pub k: u32,
    #[serde(default)]
    pub filters: Option<Vec<ScalarFilterDto>>,
    #[serde(default)]
    pub fusion: Option<FusionDto>,
    #[serde(default)]
    pub boosts: Option<FieldBoostsDto>,
    #[serde(default)]
    pub ef_search: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitDto {
    pub id: String,
    pub score: f64,
    pub text_score: Option<f64>,
    pub vector_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStepDto {
    pub layer: u32,
    pub from: Option<String>,
    pub node: String,
    pub distance: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorTraceDto {
    pub hits: Vec<HitDto>,
    pub steps: Vec<TraceStepDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDocDto {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
    #[serde(default)]
    pub filters: Option<Vec<ScalarFilterDto>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorOptionsDto {
    pub dimension: u32,
    pub space_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenOptionsDto {
    /// Omitir para el modo solo texto.
    #[serde(default)]
    pub vector: Option<VectorOptionsDto>,
    /// `"local"` activa el embedder local.
    #[serde(default)]
    pub embedder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntryDto {
    pub id: String,
    pub version: u64,
    pub doc: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanOptionsDto {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub reverse: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColOpDto {
    /// `"put"` | `"delete"`.
    pub op: String,
    pub collection: String,
    pub id: String,
    #[serde(default)]
    pub doc: Option<Value>,
    #[serde(default)]
    pub expected_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredicateDto {
    /// `"Eq"` | `"Contains"` | `"Always"`.
    pub kind: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventPatternDto {
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub stream_id: Option<String>,
    #[serde(default)]
    pub predicate: Option<PredicateDto>,
}

/// Avance de la descarga del modelo del embedder local.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProgressDto {
    pub file: String,
    pub file_index: u32,
    pub file_count: u32,
    pub downloaded: u64,
    pub total: u64,
}

/// Modelo del embedder local ya presente y verificado en la caché.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedModelDto {
    pub dir: String,
    pub space_id: String,
    pub cached: bool,
}

// ---------------------------------------------------------------------------------------------
// Conversiones de entrada (DTO → tipos del motor)
// ---------------------------------------------------------------------------------------------

fn required_str(payload: &Value, kind: &str, field: &str) -> Result<String> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| BindingError::new(format!("{kind} requires payload.{field}")))
}

fn required_u64(payload: &Value, kind: &str, field: &str) -> Result<u64> {
    payload
        .get(field)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| BindingError::new(format!("{kind} requires payload.{field}")))
}

impl EventInputDto {
    pub fn into_core(self) -> Result<EventInput> {
        let payload = self.payload;
        let kind = match self.kind.as_str() {
            "Fact" => EventKind::Fact,
            "StateTransition" => EventKind::StateTransition,
            "MemoryInvalidate" => EventKind::MemoryInvalidate {
                target_seq: required_u64(&payload, "MemoryInvalidate", "target_seq")?,
            },
            "ToolCall" => EventKind::ToolCall {
                tool: required_str(&payload, "ToolCall", "tool")?,
            },
            "ConsentGranted" => {
                let from = AgentId::from(required_str(&payload, "ConsentGranted", "from")?);
                let to = AgentId::from(required_str(&payload, "ConsentGranted", "to")?);
                let action = required_str(&payload, "ConsentGranted", "action")?;
                let resource = required_str(&payload, "ConsentGranted", "resource")?;
                let expires = payload.get("expires").and_then(|v| v.as_u64());
                EventKind::ConsentGranted {
                    from,
                    to,
                    scope: hivedb_core::Scope::new(action, resource),
                    expires,
                }
            }
            "ConsentRevoked" => EventKind::ConsentRevoked {
                grant_seq: required_u64(&payload, "ConsentRevoked", "grant_seq")?,
            },
            "IntentLogged" => EventKind::IntentLogged {
                actor: AgentId::from(required_str(&payload, "IntentLogged", "actor")?),
                intent: required_str(&payload, "IntentLogged", "intent")?,
                authorized_by: payload.get("authorized_by").and_then(|v| v.as_u64()),
            },
            "LearningProposal" => EventKind::LearningProposal,
            other => return fail(format!("unknown event kind: {other}")),
        };

        let mut input = EventInput::new(self.agent_id, self.stream_id, kind).with_payload(payload);
        if let Some(seq) = self.causation {
            input = input.with_causation(seq);
        }
        if let Some(correlation) = self.correlation {
            let parsed = uuid::Uuid::parse_str(&correlation)
                .map_err(|e| BindingError::new(format!("invalid correlation UUID: {e}")))?;
            input.correlation = Some(parsed);
        }
        Ok(input)
    }
}

impl From<&hivedb_core::Event> for EventDto {
    fn from(event: &hivedb_core::Event) -> Self {
        EventDto {
            seq: event.seq,
            agent_id: event.agent_id.0.clone(),
            stream_id: event.stream_id.0.clone(),
            kind_tag: event.kind_tag().to_string(),
            timestamp: event.timestamp,
            causation: event.causation,
            correlation: event.correlation.map(|u| u.to_string()),
            payload: event.payload.clone(),
        }
    }
}

impl From<ScalarFilterDto> for ScalarFilter {
    fn from(filter: ScalarFilterDto) -> Self {
        ScalarFilter::Eq {
            field: filter.field,
            value: filter.value,
        }
    }
}

fn filters_to_core(filters: Option<Vec<ScalarFilterDto>>) -> Vec<ScalarFilter> {
    filters
        .map(|fs| fs.into_iter().map(Into::into).collect())
        .unwrap_or_default()
}

impl HybridQueryDto {
    pub fn into_core(self) -> Result<HybridQuery> {
        let fusion = match self.fusion {
            Some(fusion) => match fusion.kind.as_str() {
                "rrf" => Fusion::Rrf {
                    k: fusion.k.unwrap_or(60) as usize,
                },
                other => {
                    return fail(format!(
                        "unknown fusion kind: {other} (only \"rrf\" is supported)"
                    ));
                }
            },
            None => Fusion::default(),
        };
        let boosts = self.boosts.map(|b| {
            let defaults = FieldBoosts::default();
            FieldBoosts {
                name: b.name.map(|v| v as f32).unwrap_or(defaults.name),
                body: b.body.map(|v| v as f32).unwrap_or(defaults.body),
                tags: b.tags.map(|v| v as f32).unwrap_or(defaults.tags),
            }
        });
        Ok(HybridQuery {
            text: self.text,
            vector: self.vector,
            k: self.k as usize,
            filters: filters_to_core(self.filters),
            fusion,
            boosts,
            ef_search: self.ef_search.map(|v| v as usize),
        })
    }
}

impl From<IndexDocDto> for IndexDoc {
    fn from(doc: IndexDocDto) -> Self {
        IndexDoc {
            id: doc.id,
            name: doc.name,
            body: doc.body,
            tags: doc.tags,
            vector: doc.vector,
            filters: filters_to_core(doc.filters),
        }
    }
}

impl From<ScanOptionsDto> for ScanOptions {
    fn from(options: ScanOptionsDto) -> Self {
        ScanOptions {
            prefix: options.prefix,
            start: options.start,
            limit: options.limit.unwrap_or(0) as usize,
            offset: options.offset.unwrap_or(0) as usize,
            reverse: options.reverse.unwrap_or(false),
        }
    }
}

impl EventPatternDto {
    pub fn into_core(self) -> Result<EventPattern> {
        let kind = match self.kind.as_deref() {
            Some("Fact") => Some(EventKindTag::Fact),
            Some("StateTransition") => Some(EventKindTag::StateTransition),
            Some("MemoryInvalidate") => Some(EventKindTag::MemoryInvalidate),
            Some("ToolCall") => Some(EventKindTag::ToolCall),
            Some("ConsentGranted") => Some(EventKindTag::ConsentGranted),
            Some("ConsentRevoked") => Some(EventKindTag::ConsentRevoked),
            Some("IntentLogged") => Some(EventKindTag::IntentLogged),
            Some(other) => return fail(format!("unknown kind: {other}")),
            None => None,
        };

        let predicate = match self.predicate {
            Some(p) => match p.kind.as_str() {
                "Eq" => {
                    let path = p
                        .path
                        .ok_or_else(|| BindingError::new("Eq predicate requires path"))?;
                    let value = p
                        .value
                        .ok_or_else(|| BindingError::new("Eq predicate requires value"))?;
                    Some(Predicate::Eq { path, value })
                }
                "Contains" => {
                    let path = p
                        .path
                        .ok_or_else(|| BindingError::new("Contains predicate requires path"))?;
                    let value = p
                        .value
                        .ok_or_else(|| BindingError::new("Contains predicate requires value"))?;
                    Some(Predicate::Contains { path, value })
                }
                "Always" => Some(Predicate::Always),
                other => {
                    return fail(format!(
                        "unknown predicate kind: {other} (expected Eq, Contains or Always)"
                    ));
                }
            },
            None => None,
        };

        Ok(EventPattern {
            agent_id: self.agent_id.map(AgentId::from),
            kind,
            stream_id: self.stream_id.map(StreamId::from),
            predicate,
        })
    }
}

impl ColOpDto {
    fn into_core(self) -> Result<ColOp> {
        match self.op.as_str() {
            "put" => Ok(ColOp::Put {
                collection: self.collection,
                id: self.id,
                doc: self
                    .doc
                    .ok_or_else(|| BindingError::new("batch put requires the json field"))?,
                expected_version: self.expected_version,
            }),
            "delete" => Ok(ColOp::Delete {
                collection: self.collection,
                id: self.id,
            }),
            other => fail(format!(
                "unknown batch op: {other} (expected \"put\" or \"delete\")"
            )),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Conversiones de salida (tipos del motor → DTO)
// ---------------------------------------------------------------------------------------------

fn hit_to_dto(hit: hivedb_index::Hit) -> HitDto {
    HitDto {
        id: hit.id,
        score: hit.score as f64,
        text_score: hit.text_score.map(|s| s as f64),
        vector_score: hit.vector_score.map(|s| s as f64),
    }
}

fn doc_entry_to_dto(entry: hivedb_core::DocEntry) -> DocEntryDto {
    DocEntryDto {
        id: entry.id,
        version: entry.version,
        doc: entry.doc,
    }
}

fn tool_stats_to_dto(stats: hivedb_core::ToolStats) -> ToolStatsDto {
    ToolStatsDto {
        invocations: stats.invocations,
        errors: stats.errors,
        total_latency_ms: stats.total_latency_ms,
        total_cost: stats.total_cost,
        last_outcome: stats.last_outcome,
        last_seq: stats.last_seq,
    }
}

fn agent_ids(ids: &[String]) -> Vec<AgentId> {
    ids.iter().cloned().map(AgentId::from).collect()
}

// ---------------------------------------------------------------------------------------------
// Embedder local
// ---------------------------------------------------------------------------------------------

/// Obtiene el embedder local compartido del proceso. **Bloqueante**: la primera vez descarga el
/// modelo (~470 MB, si falta) y lo carga; después devuelve la misma instancia. Llamar desde un
/// hilo que pueda bloquearse (`spawn_blocking` en Node, GIL liberado en Python).
#[cfg(feature = "embedder-local")]
pub fn local_embedder() -> Result<Arc<dyn Embedder>> {
    // Una sola copia del modelo por proceso: todas las bases comparten la misma instancia.
    hivedb_embed::LocalEmbedder::shared()
        .map(|embedder| embedder as Arc<dyn Embedder>)
        .map_err(err)
}

#[cfg(not(feature = "embedder-local"))]
pub fn local_embedder() -> Result<Arc<dyn Embedder>> {
    fail(NO_EMBEDDER_MESSAGE)
}

/// Descarga (si falta) y verifica el modelo del embedder local avisando del avance. **Bloqueante**.
#[cfg(feature = "embedder-local")]
pub fn prepare_embedder(mut on_progress: impl FnMut(ModelProgressDto)) -> Result<PreparedModelDto> {
    let mut report = |progress: &hivedb_embed::Progress| {
        on_progress(ModelProgressDto {
            file: progress.file.clone(),
            file_index: progress.file_index as u32,
            file_count: progress.file_count as u32,
            downloaded: progress.downloaded,
            total: progress.total,
        });
    };
    hivedb_embed::ensure_multilingual_e5_small_with(&mut report)
        .map(|files| PreparedModelDto {
            dir: files.dir().display().to_string(),
            space_id: files.space_id().to_string(),
            cached: files.was_cached(),
        })
        .map_err(err)
}

#[cfg(not(feature = "embedder-local"))]
pub fn prepare_embedder(_on_progress: impl FnMut(ModelProgressDto)) -> Result<PreparedModelDto> {
    fail(NO_EMBEDDER_MESSAGE)
}

/// Resuelve el nombre de embedder de `OpenOptionsDto.embedder` (bloqueante, ver `local_embedder`).
pub fn resolve_embedder(name: Option<&str>) -> Result<Option<Arc<dyn Embedder>>> {
    match name {
        None => Ok(None),
        Some("local") => local_embedder().map(Some),
        Some(other) => fail(format!(
            "unknown embedder: {other} (only \"local\" is supported)"
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Handle: la base abierta
// ---------------------------------------------------------------------------------------------

/// Base abierta. `RwLock` y no `Mutex`: las operaciones comparten el candado de lectura, así que
/// varias consultas pueden ejecutarse a la vez sobre la misma base. `close()` toma el de escritura
/// y espera a que terminen las operaciones en curso.
pub struct Handle {
    inner: RwLock<Option<Arc<HiveDB>>>,
}

impl Handle {
    /// Abre la base. `":memory:"` abre una base temporal (vive lo que dure el proceso). El
    /// embedder, si lo hay, se resuelve antes con [`resolve_embedder`].
    pub fn open(
        path: &str,
        vector: Option<VectorOptionsDto>,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Result<Self> {
        let options = OpenOptions {
            vector: vector.map(|v| VectorOptions::new(v.dimension as usize, v.space_id)),
            embedder,
        };
        let db = if path == ":memory:" {
            HiveDB::open_temp_with_options(options).map_err(err)?
        } else {
            HiveDB::open_with_options(path, options).map_err(err)?
        };
        Ok(Self {
            inner: RwLock::new(Some(Arc::new(db))),
        })
    }

    pub fn with_db<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&HiveDB) -> Result<T>,
    {
        let lock = self
            .inner
            .read()
            .map_err(|_| BindingError::new("database lock poisoned"))?;
        match lock.as_ref() {
            Some(db) => f(db),
            None => fail("database is closed"),
        }
    }

    /// Clona el `Arc` de la base (para tareas que sobreviven a la llamada, como las suscripciones).
    pub fn db_arc(&self) -> Result<Arc<HiveDB>> {
        let lock = self
            .inner
            .read()
            .map_err(|_| BindingError::new("database lock poisoned"))?;
        lock.clone()
            .ok_or_else(|| BindingError::new("database is closed"))
    }

    /// Cierra la base: espera a las operaciones en curso. Idempotente.
    pub fn close(&self) {
        let mut lock = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *lock = None;
    }

    // -- registro de eventos ------------------------------------------------------------------

    pub fn append(&self, input: EventInputDto) -> Result<u64> {
        let input = input.into_core()?;
        self.with_db(|db| db.append(input).map_err(err))
    }

    pub fn read(&self, seq: u64) -> Result<EventDto> {
        self.with_db(|db| Ok(EventDto::from(&db.read(seq).map_err(err)?)))
    }

    pub fn log_len(&self) -> Result<u64> {
        self.with_db(|db| db.log_len().map_err(err))
    }

    pub fn last_seq(&self) -> Result<u64> {
        self.with_db(|db| db.last_seq().map_err(err))
    }

    // -- proyecciones -------------------------------------------------------------------------

    /// Estadísticas agregadas de una herramienta. Sin `agents`, la proyección mezcla el estado de
    /// todos los agentes de la base (pásalo siempre en un despliegue compartido).
    pub fn tool_stats(
        &self,
        tool: &str,
        agents: Option<&[String]>,
    ) -> Result<Option<ToolStatsDto>> {
        self.with_db(|db| {
            let stats = match agents {
                Some(ids) => db
                    .tool_stats_for_agents(tool, &agent_ids(ids))
                    .map_err(err)?,
                None => db.tool_stats(tool).map_err(err)?,
            };
            Ok(stats.map(tool_stats_to_dto))
        })
    }

    pub fn project_task_state(&self, agent_id: &str, stream_id: &str) -> Result<Option<String>> {
        use hivedb_core::{TaskState, TaskStateState};
        self.with_db(|db| {
            let state: TaskStateState = db.project::<TaskState>().map_err(err)?;
            Ok(state
                .get(&AgentId::from(agent_id), &StreamId::from(stream_id))
                .map(|v| v.to_string()))
        })
    }

    pub fn can(&self, agent: &str, action: &str, resource: &str) -> Result<DecisionDto> {
        self.with_db(|db| {
            let decision = db.can(agent, action, resource).map_err(err)?;
            Ok(DecisionDto {
                allowed: decision.allowed(),
                intent_log_seq: decision.intent_log_seq(),
            })
        })
    }

    // -- memoria de trabajo -------------------------------------------------------------------

    pub fn working_set(
        &self,
        agent_id: &str,
        key: &str,
        value: Value,
        ttl_ms: Option<i64>,
    ) -> Result<()> {
        let ttl = ttl_ms.map(|ms| std::time::Duration::from_millis(ms.max(0) as u64));
        self.with_db(|db| {
            db.working_set(agent_id, key, value, ttl);
            Ok(())
        })
    }

    pub fn working_get(&self, agent_id: &str, key: &str) -> Result<Option<Value>> {
        self.with_db(|db| Ok(db.working_get(agent_id, key)))
    }

    pub fn working_keys(&self, agent_id: &str) -> Result<Vec<String>> {
        self.with_db(|db| Ok(db.working_keys(agent_id)))
    }

    // -- contexto y harness -------------------------------------------------------------------

    /// Hilo causal de un stream. `agents` lo acota a esos agentes (obligatorio en bases compartidas).
    pub fn causal_thread(&self, stream_id: &str, agents: Option<&[String]>) -> Result<Value> {
        self.with_db(|db| {
            let thread = match agents {
                Some(ids) => db
                    .causal_thread_for_agents(stream_id, &agent_ids(ids))
                    .map_err(err)?,
                None => db.causal_thread(stream_id).map_err(err)?,
            };
            serde_json::to_value(&thread)
                .map_err(|e| BindingError::new(format!("serialization error: {e}")))
        })
    }

    pub fn build_agent_context(&self, request: Value) -> Result<Value> {
        self.with_db(|db| {
            let request: AgentContextRequest = serde_json::from_value(request)
                .map_err(|e| BindingError::new(format!("invalid request: {e}")))?;
            let context = db.build_agent_context(request).map_err(err)?;
            serde_json::to_value(&context)
                .map_err(|e| BindingError::new(format!("serialization error: {e}")))
        })
    }

    // -- índice semántico ---------------------------------------------------------------------

    /// Obsoleto: usa `upsert_doc`. `text` se guarda en el campo `body`.
    pub fn index_doc(
        &self,
        id: String,
        text: String,
        vector: Vec<f32>,
        filters: Option<Vec<ScalarFilterDto>>,
    ) -> Result<()> {
        let filters = filters_to_core(filters);
        self.with_db(|db| db.index_doc_with(id, text, vector, &filters).map_err(err))
    }

    pub fn upsert_doc(&self, doc: IndexDocDto) -> Result<()> {
        let doc = IndexDoc::from(doc);
        self.with_db(|db| db.upsert_doc(&doc).map_err(err))
    }

    pub fn upsert_batch(&self, docs: Vec<IndexDocDto>) -> Result<()> {
        let docs: Vec<IndexDoc> = docs.into_iter().map(IndexDoc::from).collect();
        self.with_db(|db| db.upsert_batch(&docs).map_err(err))
    }

    pub fn delete_doc(&self, id: &str) -> Result<()> {
        self.with_db(|db| db.delete_doc(id).map_err(err))
    }

    pub fn delete_by_filter(&self, filter: ScalarFilterDto) -> Result<()> {
        let filter = ScalarFilter::from(filter);
        self.with_db(|db| db.delete_by_filter(&filter).map_err(err))
    }

    pub fn clear_index(&self) -> Result<()> {
        self.with_db(|db| db.clear_index().map_err(err))
    }

    pub fn compact_index(&self) -> Result<()> {
        self.with_db(|db| db.compact_index().map_err(err))
    }

    pub fn query_hybrid(&self, query: HybridQueryDto) -> Result<Vec<HitDto>> {
        let query = query.into_core()?;
        self.with_db(|db| {
            let hits = db.query_hybrid(query).map_err(err)?;
            Ok(hits.into_iter().map(hit_to_dto).collect())
        })
    }

    /// Búsqueda vectorial sin filtros con la ruta del HNSW (capas y nodos visitados).
    pub fn trace_vector(
        &self,
        vector: &[f32],
        k: u32,
        ef_search: Option<u32>,
    ) -> Result<VectorTraceDto> {
        self.with_db(|db| {
            let trace = db
                .trace_vector_search(vector, k as usize, ef_search.map(|e| e as usize))
                .map_err(err)?;
            Ok(VectorTraceDto {
                hits: trace
                    .hits
                    .into_iter()
                    .map(|(id, _, score)| HitDto {
                        id,
                        score: score as f64,
                        text_score: None,
                        vector_score: Some(score as f64),
                    })
                    .collect(),
                steps: trace
                    .steps
                    .into_iter()
                    .map(|s| TraceStepDto {
                        layer: s.layer as u32,
                        from: s.from,
                        node: s.node,
                        distance: s.distance as f64,
                    })
                    .collect(),
            })
        })
    }

    // -- colecciones --------------------------------------------------------------------------

    pub fn col_put(
        &self,
        collection: &str,
        id: &str,
        doc: &Value,
        expected_version: Option<u64>,
    ) -> Result<u64> {
        self.with_db(|db| {
            db.col_put(collection, id, doc, PutOptions { expected_version })
                .map_err(err)
        })
    }

    pub fn col_get(&self, collection: &str, id: &str) -> Result<Option<DocEntryDto>> {
        self.with_db(|db| {
            Ok(db
                .col_get(collection, id)
                .map_err(err)?
                .map(doc_entry_to_dto))
        })
    }

    pub fn col_delete(&self, collection: &str, id: &str) -> Result<bool> {
        self.with_db(|db| db.col_delete(collection, id).map_err(err))
    }

    pub fn col_scan(
        &self,
        collection: &str,
        options: Option<ScanOptionsDto>,
    ) -> Result<Vec<DocEntryDto>> {
        let scan = ScanOptions::from(options.unwrap_or_default());
        self.with_db(|db| {
            Ok(db
                .col_scan(collection, &scan)
                .map_err(err)?
                .into_iter()
                .map(doc_entry_to_dto)
                .collect())
        })
    }

    pub fn col_count(&self, collection: &str) -> Result<u64> {
        self.with_db(|db| db.col_count(collection).map_err(err))
    }

    pub fn col_create_index(&self, collection: &str, field: &str, unique: bool) -> Result<()> {
        self.with_db(|db| db.col_create_index(collection, field, unique).map_err(err))
    }

    pub fn col_find_by(
        &self,
        collection: &str,
        field: &str,
        value: &Value,
        options: Option<ScanOptionsDto>,
    ) -> Result<Vec<DocEntryDto>> {
        let scan = ScanOptions::from(options.unwrap_or_default());
        self.with_db(|db| {
            Ok(db
                .col_find_by(collection, field, value, &scan)
                .map_err(err)?
                .into_iter()
                .map(doc_entry_to_dto)
                .collect())
        })
    }

    /// Aplica varias altas/bajas de forma atómica: o se confirman todas o ninguna.
    pub fn col_batch(&self, ops: Vec<ColOpDto>) -> Result<()> {
        let ops: Vec<ColOp> = ops
            .into_iter()
            .map(ColOpDto::into_core)
            .collect::<Result<_>>()?;
        self.with_db(|db| db.col_batch(&ops).map_err(err))
    }

    // -- suscripciones ------------------------------------------------------------------------

    /// Registra una suscripción. El binding la consume (`Subscription::next` es asíncrono).
    pub fn subscribe(&self, pattern: EventPatternDto) -> Result<Subscription> {
        let pattern = pattern.into_core()?;
        Ok(self.db_arc()?.subscribe(pattern))
    }
}

/// Evalúa una tarea con el bucle del harness (no necesita base abierta).
pub fn evaluate_harness(input: Value) -> Result<Value> {
    let input: HarnessInput = serde_json::from_value(input)
        .map_err(|e| BindingError::new(format!("invalid input: {e}")))?;
    let evaluation = HarnessLoop::evaluate(input);
    serde_json::to_value(&evaluation)
        .map_err(|e| BindingError::new(format!("serialization error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn open() -> Handle {
        Handle::open(":memory:", None, None).unwrap()
    }

    fn event(kind: &str, payload: Value) -> EventInputDto {
        EventInputDto {
            agent_id: "a".into(),
            stream_id: "s".into(),
            kind: kind.into(),
            payload,
            causation: None,
            correlation: None,
        }
    }

    #[test]
    fn el_codigo_se_detecta_por_el_prefijo() {
        assert_eq!(
            BindingError::new("INVALID_VECTOR: dimension mismatch").code,
            Some(ErrorCode::InvalidVector)
        );
        assert_eq!(
            BindingError::new("x EMBEDDER_UNAVAILABLE: sin red").code,
            Some(ErrorCode::EmbedderUnavailable)
        );
        assert_eq!(BindingError::new("database is closed").code, None);
    }

    #[test]
    fn eventos_validan_los_campos_obligatorios() {
        let db = open();
        let error = db.append(event("ToolCall", json!({}))).unwrap_err();
        assert_eq!(error.message, "ToolCall requires payload.tool");
        let error = db.append(event("Inventado", json!({}))).unwrap_err();
        assert_eq!(error.message, "unknown event kind: Inventado");
        let seq = db
            .append(event("ToolCall", json!({ "tool": "grep" })))
            .unwrap();
        assert_eq!(db.read(seq).unwrap().kind_tag, "ToolCall");
        assert_eq!(db.log_len().unwrap(), 1);
    }

    #[test]
    fn la_base_cerrada_falla_y_close_es_idempotente() {
        let db = open();
        db.close();
        db.close();
        assert_eq!(db.log_len().unwrap_err().message, "database is closed");
    }

    #[test]
    fn colecciones_con_version_esperada() {
        let db = open();
        assert_eq!(
            db.col_put("c", "1", &json!({ "n": 1 }), Some(0)).unwrap(),
            1
        );
        assert!(db.col_put("c", "1", &json!({ "n": 2 }), Some(0)).is_err());
        assert_eq!(
            db.col_put("c", "1", &json!({ "n": 2 }), Some(1)).unwrap(),
            2
        );
        assert_eq!(db.col_count("c").unwrap(), 1);
        let entry = db.col_get("c", "1").unwrap().unwrap();
        assert_eq!((entry.version, entry.doc), (2, json!({ "n": 2 })));
    }

    #[test]
    fn el_lote_exige_el_documento_en_los_put() {
        let db = open();
        let op = ColOpDto {
            op: "put".into(),
            collection: "c".into(),
            id: "1".into(),
            doc: None,
            expected_version: None,
        };
        assert_eq!(
            db.col_batch(vec![op]).unwrap_err().message,
            "batch put requires the json field"
        );
    }

    #[test]
    fn la_consulta_hibrida_rechaza_fusiones_desconocidas() {
        let db = open();
        let query = HybridQueryDto {
            text: Some("hola".into()),
            vector: None,
            k: 3,
            filters: None,
            fusion: Some(FusionDto {
                kind: "otra".into(),
                k: None,
            }),
            boosts: None,
            ef_search: None,
        };
        assert!(
            db.query_hybrid(query)
                .unwrap_err()
                .message
                .starts_with("unknown fusion kind: otra")
        );
    }

    #[test]
    fn busqueda_de_texto_sin_vectores() {
        let db = open();
        db.upsert_doc(IndexDocDto {
            id: "d1".into(),
            name: None,
            body: Some("receta de paella valenciana".into()),
            tags: None,
            vector: None,
            filters: None,
        })
        .unwrap();
        let hits = db
            .query_hybrid(HybridQueryDto {
                text: Some("paella".into()),
                vector: None,
                k: 5,
                filters: None,
                fusion: None,
                boosts: None,
                ef_search: None,
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "d1");
    }

    #[test]
    fn un_embedder_desconocido_falla_con_mensaje_claro() {
        let error = resolve_embedder(Some("remoto")).err().unwrap();
        assert_eq!(
            error.message,
            "unknown embedder: remoto (only \"local\" is supported)"
        );
        assert!(resolve_embedder(None).unwrap().is_none());
    }

    #[test]
    fn patrones_validan_el_predicado() {
        let pattern = EventPatternDto {
            predicate: Some(PredicateDto {
                kind: "Eq".into(),
                path: None,
                value: None,
            }),
            ..Default::default()
        };
        assert_eq!(
            pattern.into_core().unwrap_err().message,
            "Eq predicate requires path"
        );
    }
}
