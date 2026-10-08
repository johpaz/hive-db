use hivedb_binding_core as core;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::*;
use serde_json::Value;
use tokio::task::JoinHandle;

#[napi(object)]
pub struct JsEventInput {
    pub agent_id: String,
    pub stream_id: String,
    pub kind: String,
    pub payload: String,
    pub causation: Option<i64>,
    /// UUID string. Links this event to others sharing the same objective/
    /// intent, so `causalThread()`'s objectiveDrift detector can tell a
    /// decision apart from the stream's original `IntentLogged` correlation.
    pub correlation: Option<String>,
}

#[napi(object)]
pub struct JsEvent {
    pub seq: i64,
    pub agent_id: String,
    pub stream_id: String,
    pub kind_tag: String,
    pub timestamp: i64,
    pub causation: Option<i64>,
    pub correlation: Option<String>,
    pub payload: String,
}

#[napi(object)]
pub struct JsDecision {
    pub allowed: bool,
    pub intent_log_seq: Option<i64>,
}

#[napi(object)]
pub struct JsScalarFilter {
    pub field: String,
    pub value: String,
}

#[napi(object)]
pub struct JsFusion {
    /// Fusion strategy. Only `"rrf"` is supported.
    pub kind: String,
    /// RRF `k` parameter (default 60).
    pub k: Option<u32>,
}

#[napi(object)]
pub struct JsFieldBoosts {
    pub name: Option<f64>,
    pub body: Option<f64>,
    pub tags: Option<f64>,
}

#[napi(object)]
pub struct JsToolStats {
    pub invocations: i64,
    pub errors: i64,
    pub total_latency_ms: i64,
    pub total_cost: f64,
    pub last_outcome: Option<String>,
    pub last_seq: i64,
}

#[napi(object)]
pub struct JsHybridQuery {
    pub text: Option<String>,
    pub vector: Option<Float32Array>,
    pub k: u32,
    pub filters: Option<Vec<JsScalarFilter>>,
    pub fusion: Option<JsFusion>,
    pub boosts: Option<JsFieldBoosts>,
    pub ef_search: Option<u32>,
}

#[napi(object)]
pub struct JsHit {
    pub id: String,
    pub score: f64,
    pub text_score: Option<f64>,
    pub vector_score: Option<f64>,
}

#[napi(object)]
pub struct JsTraceStep {
    pub layer: u32,
    pub from: Option<String>,
    pub node: String,
    pub distance: f64,
}

#[napi(object)]
pub struct JsVectorTrace {
    pub hits: Vec<JsHit>,
    pub steps: Vec<JsTraceStep>,
}

#[napi(object)]
pub struct JsIndexDoc {
    pub id: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub tags: Option<String>,
    pub vector: Option<Float32Array>,
    pub filters: Option<Vec<JsScalarFilter>>,
}

#[napi(object)]
pub struct JsVectorOptions {
    pub dimension: u32,
    pub space_id: String,
}

#[napi(object)]
pub struct JsOpenOptions {
    /// Omitir para usar el modo solo texto.
    pub vector: Option<JsVectorOptions>,
    /// `"local"` activa el embedder local (requiere un binario compilado con la
    /// feature `embedder-local`): los documentos sin vector se embeben solos.
    pub embedder: Option<String>,
}

#[napi(object)]
pub struct JsDocEntry {
    pub id: String,
    /// Monotonic per-document version, starting at 1 on first put.
    pub version: i64,
    /// The stored document as a JSON string.
    pub json: String,
}

#[napi(object)]
pub struct JsPutOptions {
    /// Optimistic concurrency: current version must equal this value
    /// (0 = the document must not exist yet). Omit for unconditional upsert.
    pub expected_version: Option<i64>,
}

#[napi(object)]
pub struct JsScanOptions {
    pub prefix: Option<String>,
    pub start: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub reverse: Option<bool>,
}

#[napi(object)]
pub struct JsColOp {
    /// "put" | "delete"
    pub op: String,
    pub collection: String,
    pub id: String,
    /// JSON document (required for "put").
    pub json: Option<String>,
    pub expected_version: Option<i64>,
}

#[napi(object)]
pub struct JsEventPattern {
    pub agent_id: Option<String>,
    pub kind: Option<String>,
    pub stream_id: Option<String>,
    pub predicate: Option<JsPredicate>,
}

#[napi(object)]
pub struct JsPredicate {
    /// `"Eq"` | `"Contains"` | `"Always"`.
    pub kind: String,
    /// JSON pointer path inside the payload (e.g. `"/temperature"` or `"temperature"`).
    pub path: Option<String>,
    /// JSON-encoded value used by `Eq` and `Contains`.
    pub value: Option<String>,
}

/// Avance de la descarga del modelo del embedder local.
#[napi(object)]
pub struct JsModelProgress {
    /// Archivo en descarga (`model.safetensors`…).
    pub file: String,
    /// Posición de este archivo entre los que faltan (desde 1).
    pub file_index: u32,
    pub file_count: u32,
    /// Bytes ya descargados de este archivo (incluye lo reanudado).
    pub downloaded: f64,
    pub total: f64,
}

/// Modelo del embedder local ya presente y verificado en la caché.
#[napi(object)]
pub struct JsPreparedModel {
    /// Directorio con los archivos del modelo.
    pub dir: String,
    /// Identidad del espacio vectorial (modelo y revisión).
    pub space_id: String,
    /// `true` si ya estaba todo en la caché y no se descargó nada.
    pub cached: bool,
}

/// Descarga (si falta) y verifica el modelo del embedder local, avisando del avance, sin abrir
/// ninguna base. Permite mostrar una barra de progreso antes de `open({ embedder: "local" })`.
/// Sin la feature `embedder-local` falla con `EMBEDDER_UNAVAILABLE`.
#[napi]
pub async fn prepare_embedder(
    on_progress: Option<ThreadsafeFunction<JsModelProgress>>,
) -> Result<JsPreparedModel> {
    tokio::task::spawn_blocking(move || {
        core::prepare_embedder(|progress| {
            if let Some(callback) = on_progress.as_ref() {
                callback.call(
                    Ok(JsModelProgress {
                        file: progress.file,
                        file_index: progress.file_index,
                        file_count: progress.file_count,
                        downloaded: progress.downloaded as f64,
                        total: progress.total as f64,
                    }),
                    ThreadsafeFunctionCallMode::NonBlocking,
                );
            }
        })
    })
    .await
    .map_err(js_err)?
    .map(|model| JsPreparedModel {
        dir: model.dir,
        space_id: model.space_id,
        cached: model.cached,
    })
    .map_err(js_err)
}

fn js_err<E: std::fmt::Display>(e: E) -> Error {
    Error::from_reason(e.to_string())
}

fn parse_payload(payload: &str) -> Result<Value> {
    serde_json::from_str(payload).map_err(|e| Error::from_reason(format!("invalid payload: {e}")))
}

fn parse_json_doc(json: &str) -> Result<Value> {
    serde_json::from_str(json)
        .map_err(|e| Error::from_reason(format!("invalid document JSON: {e}")))
}

fn to_json_string(value: &Value) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|e| Error::from_reason(format!("serialization error: {e}")))
}

fn js_to_event_input(input: JsEventInput) -> Result<core::EventInputDto> {
    Ok(core::EventInputDto {
        payload: parse_payload(&input.payload)?,
        agent_id: input.agent_id,
        stream_id: input.stream_id,
        kind: input.kind,
        causation: input.causation.map(|v| v as u64),
        correlation: input.correlation,
    })
}

fn event_to_js(event: &core::EventDto) -> JsEvent {
    JsEvent {
        seq: event.seq as i64,
        agent_id: event.agent_id.clone(),
        stream_id: event.stream_id.clone(),
        kind_tag: event.kind_tag.clone(),
        timestamp: event.timestamp as i64,
        causation: event.causation.map(|v| v as i64),
        correlation: event.correlation.clone(),
        payload: event.payload.to_string(),
    }
}

fn js_to_scalar_filter(filter: JsScalarFilter) -> core::ScalarFilterDto {
    core::ScalarFilterDto {
        field: filter.field,
        value: filter.value,
    }
}

fn js_to_filters(filters: Option<Vec<JsScalarFilter>>) -> Option<Vec<core::ScalarFilterDto>> {
    filters.map(|fs| fs.into_iter().map(js_to_scalar_filter).collect())
}

fn js_to_hybrid_query(query: JsHybridQuery) -> core::HybridQueryDto {
    core::HybridQueryDto {
        text: query.text,
        vector: query.vector.map(|v| v.to_vec()),
        k: query.k,
        filters: js_to_filters(query.filters),
        fusion: query.fusion.map(|f| core::FusionDto {
            kind: f.kind,
            k: f.k,
        }),
        boosts: query.boosts.map(|b| core::FieldBoostsDto {
            name: b.name,
            body: b.body,
            tags: b.tags,
        }),
        ef_search: query.ef_search,
    }
}

fn js_to_index_doc(doc: JsIndexDoc) -> core::IndexDocDto {
    core::IndexDocDto {
        id: doc.id,
        name: doc.name,
        body: doc.body,
        tags: doc.tags,
        vector: doc.vector.map(|v| v.to_vec()),
        filters: js_to_filters(doc.filters),
    }
}

fn hit_to_js(hit: core::HitDto) -> JsHit {
    JsHit {
        id: hit.id,
        score: hit.score,
        text_score: hit.text_score,
        vector_score: hit.vector_score,
    }
}

fn js_to_event_pattern(pattern: JsEventPattern) -> Result<core::EventPatternDto> {
    let predicate = match pattern.predicate {
        Some(p) => Some(core::PredicateDto {
            kind: p.kind,
            path: p.path,
            value: p.value.as_deref().map(parse_payload).transpose()?,
        }),
        None => None,
    };
    Ok(core::EventPatternDto {
        agent_id: pattern.agent_id,
        kind: pattern.kind,
        stream_id: pattern.stream_id,
        predicate,
    })
}

fn doc_entry_to_js(entry: core::DocEntryDto) -> JsDocEntry {
    JsDocEntry {
        id: entry.id,
        version: entry.version as i64,
        json: entry.doc.to_string(),
    }
}

fn js_to_scan_options(options: Option<JsScanOptions>) -> Option<core::ScanOptionsDto> {
    options.map(|o| core::ScanOptionsDto {
        prefix: o.prefix,
        start: o.start,
        limit: o.limit,
        offset: o.offset,
        reverse: o.reverse,
    })
}

#[napi]
pub struct JsHiveDB {
    /// El candado de lectura/escritura vive en el crate común: las consultas comparten la base y
    /// `close()` espera a las operaciones en curso.
    inner: core::Handle,
    runtime: tokio::runtime::Handle,
}

#[napi]
impl JsHiveDB {
    #[napi(factory)]
    pub async fn open(path: String, options: Option<JsOpenOptions>) -> Result<Self> {
        let (vector, embedder) = match options {
            Some(o) => (o.vector, o.embedder),
            None => (None, None),
        };
        // El modelo se descarga/carga fuera del hilo asíncrono (la primera vez son ~470 MB).
        let embedder = match embedder {
            None => None,
            Some(name) => tokio::task::spawn_blocking(move || core::resolve_embedder(Some(&name)))
                .await
                .map_err(js_err)?
                .map_err(js_err)?,
        };
        let vector = vector.map(|v| core::VectorOptionsDto {
            dimension: v.dimension,
            space_id: v.space_id,
        });
        // ":memory:" abre una base efímera en un directorio temporal del proceso.
        let inner = core::Handle::open(&path, vector, embedder).map_err(js_err)?;
        Ok(Self {
            inner,
            runtime: tokio::runtime::Handle::current(),
        })
    }

    #[napi]
    pub async fn append(&self, input: JsEventInput) -> Result<i64> {
        let input = js_to_event_input(input)?;
        self.inner
            .append(input)
            .map(|seq| seq as i64)
            .map_err(js_err)
    }

    #[napi]
    pub async fn read(&self, seq: i64) -> Result<JsEvent> {
        if seq < 0 {
            return Err(Error::from_reason("seq must be non-negative"));
        }
        let event = self.inner.read(seq as u64).map_err(js_err)?;
        Ok(event_to_js(&event))
    }

    #[napi]
    pub async fn log_len(&self) -> Result<i64> {
        self.inner.log_len().map(|len| len as i64).map_err(js_err)
    }

    #[napi(js_name = "lastSeq")]
    pub async fn last_seq(&self) -> Result<i64> {
        self.inner.last_seq().map(|seq| seq as i64).map_err(js_err)
    }

    /// Estadísticas agregadas de una herramienta.
    ///
    /// `ToolLedger` tiene scope `Agent`: sin `agents`, la proyección mezcla el
    /// estado de todos los shards de la base y suma las llamadas de todos los
    /// inquilinos. Pasar la lista es obligatorio en un despliegue compartido.
    #[napi(js_name = "toolStats")]
    pub async fn tool_stats(
        &self,
        tool: String,
        agents: Option<Vec<String>>,
    ) -> Result<Option<JsToolStats>> {
        let stats = self
            .inner
            .tool_stats(&tool, agents.as_deref())
            .map_err(js_err)?;
        Ok(stats.map(|s| JsToolStats {
            invocations: s.invocations as i64,
            errors: s.errors as i64,
            total_latency_ms: s.total_latency_ms as i64,
            total_cost: s.total_cost,
            last_outcome: s.last_outcome,
            last_seq: s.last_seq as i64,
        }))
    }

    #[napi]
    pub async fn project_task_state(
        &self,
        agent_id: String,
        stream_id: String,
    ) -> Result<Option<String>> {
        self.inner
            .project_task_state(&agent_id, &stream_id)
            .map_err(js_err)
    }

    #[napi]
    pub async fn can(&self, agent: String, action: String, resource: String) -> Result<JsDecision> {
        let decision = self.inner.can(&agent, &action, &resource).map_err(js_err)?;
        Ok(JsDecision {
            allowed: decision.allowed,
            intent_log_seq: decision.intent_log_seq.map(|v| v as i64),
        })
    }

    /// Store a value in working memory with an optional TTL (milliseconds).
    #[napi(js_name = "workingSet")]
    pub async fn working_set(
        &self,
        agent_id: String,
        key: String,
        json: String,
        ttl_ms: Option<i64>,
    ) -> Result<()> {
        let value = parse_payload(&json)?;
        self.inner
            .working_set(&agent_id, &key, value, ttl_ms)
            .map_err(js_err)
    }

    /// Retrieve a value from working memory, returning `None` if expired or missing.
    #[napi(js_name = "workingGet")]
    pub async fn working_get(&self, agent_id: String, key: String) -> Result<Option<String>> {
        let value = self.inner.working_get(&agent_id, &key).map_err(js_err)?;
        Ok(value.map(|v| v.to_string()))
    }

    /// Return all non-expired keys for an agent.
    #[napi(js_name = "workingKeys")]
    pub async fn working_keys(&self, agent_id: String) -> Result<Vec<String>> {
        self.inner.working_keys(&agent_id).map_err(js_err)
    }

    /// Hilo causal de un stream, como JSON.
    ///
    /// `agents` acota la búsqueda a esos agentes. Omitirlo recorre el log de
    /// toda la base, que es lo correcto cuando la base tiene un solo dueño y
    /// una fuga cuando la comparten varios inquilinos.
    #[napi(js_name = "causalThread")]
    pub async fn causal_thread(
        &self,
        stream_id: String,
        agents: Option<Vec<String>>,
    ) -> Result<String> {
        let thread = self
            .inner
            .causal_thread(&stream_id, agents.as_deref())
            .map_err(js_err)?;
        to_json_string(&thread)
    }

    /// Build an agent context window for a task as a JSON string.
    #[napi(js_name = "buildAgentContext")]
    pub async fn build_agent_context(&self, req_json: String) -> Result<String> {
        let request: Value = serde_json::from_str(&req_json)
            .map_err(|e| Error::from_reason(format!("invalid request: {e}")))?;
        let context = self.inner.build_agent_context(request).map_err(js_err)?;
        to_json_string(&context)
    }

    /// Evaluate a task with the harness loop. Input and output are JSON strings.
    #[napi(js_name = "evaluateHarness")]
    pub async fn evaluate_harness(&self, input_json: String) -> Result<String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| Error::from_reason(format!("invalid input: {e}")))?;
        let evaluation = core::evaluate_harness(input).map_err(js_err)?;
        to_json_string(&evaluation)
    }

    /// Deprecated: use `upsertDoc`. Kept for one version; `text` maps to the
    /// `body` field.
    #[napi]
    pub async fn index_doc(
        &self,
        id: String,
        text: String,
        vector: Float32Array,
        filters: Option<Vec<JsScalarFilter>>,
    ) -> Result<()> {
        self.inner
            .index_doc(id, text, vector.to_vec(), js_to_filters(filters))
            .map_err(js_err)
    }

    /// Insert or replace a document in the semantic index.
    #[napi]
    pub async fn upsert_doc(&self, doc: JsIndexDoc) -> Result<()> {
        self.inner.upsert_doc(js_to_index_doc(doc)).map_err(js_err)
    }

    /// Insert or replace a batch of documents under a single text-index
    /// commit. Much faster than repeated `upsertDoc` calls.
    #[napi]
    pub async fn upsert_batch(&self, docs: Vec<JsIndexDoc>) -> Result<()> {
        let docs = docs.into_iter().map(js_to_index_doc).collect();
        self.inner.upsert_batch(docs).map_err(js_err)
    }

    /// Delete a document from the semantic index. Missing ids are a no-op.
    #[napi]
    pub async fn delete_doc(&self, id: String) -> Result<()> {
        self.inner.delete_doc(&id).map_err(js_err)
    }

    /// Delete every indexed document carrying the given scalar filter.
    #[napi]
    pub async fn delete_by_filter(&self, filter: JsScalarFilter) -> Result<()> {
        self.inner
            .delete_by_filter(js_to_scalar_filter(filter))
            .map_err(js_err)
    }

    /// Remove every document from the semantic index.
    #[napi]
    pub async fn clear_index(&self) -> Result<()> {
        self.inner.clear_index().map_err(js_err)
    }

    /// Reconstruye los índices semánticos desde sus documentos autoritativos.
    #[napi]
    pub async fn compact_index(&self) -> Result<()> {
        self.inner.compact_index().map_err(js_err)
    }

    /// Insert or replace a JSON document in a collection. Returns the new
    /// version (starts at 1).
    #[napi]
    pub async fn col_put(
        &self,
        collection: String,
        id: String,
        json: String,
        options: Option<JsPutOptions>,
    ) -> Result<i64> {
        let doc = parse_json_doc(&json)?;
        let expected = options.and_then(|o| o.expected_version).map(|v| v as u64);
        self.inner
            .col_put(&collection, &id, &doc, expected)
            .map(|v| v as i64)
            .map_err(js_err)
    }

    /// Read a document by id.
    #[napi]
    pub async fn col_get(&self, collection: String, id: String) -> Result<Option<JsDocEntry>> {
        let entry = self.inner.col_get(&collection, &id).map_err(js_err)?;
        Ok(entry.map(doc_entry_to_js))
    }

    /// Delete a document. Returns true if it existed.
    #[napi]
    pub async fn col_delete(&self, collection: String, id: String) -> Result<bool> {
        self.inner.col_delete(&collection, &id).map_err(js_err)
    }

    /// Scan a collection in id order.
    #[napi]
    pub async fn col_scan(
        &self,
        collection: String,
        options: Option<JsScanOptions>,
    ) -> Result<Vec<JsDocEntry>> {
        let entries = self
            .inner
            .col_scan(&collection, js_to_scan_options(options))
            .map_err(js_err)?;
        Ok(entries.into_iter().map(doc_entry_to_js).collect())
    }

    /// Number of documents in a collection.
    #[napi]
    pub async fn col_count(&self, collection: String) -> Result<i64> {
        self.inner
            .col_count(&collection)
            .map(|c| c as i64)
            .map_err(js_err)
    }

    /// Create an equality index on a top-level field (optionally unique).
    /// Backfills existing documents; idempotent for an identical definition.
    #[napi]
    pub async fn col_create_index(
        &self,
        collection: String,
        field: String,
        unique: bool,
    ) -> Result<()> {
        self.inner
            .col_create_index(&collection, &field, unique)
            .map_err(js_err)
    }

    /// Look up documents whose indexed field equals the given JSON scalar
    /// (e.g. `"\"abc\""`, `"42"`, `"true"`). Requires colCreateIndex first.
    #[napi]
    pub async fn col_find_by(
        &self,
        collection: String,
        field: String,
        value_json: String,
        options: Option<JsScanOptions>,
    ) -> Result<Vec<JsDocEntry>> {
        let value = parse_json_doc(&value_json)?;
        let entries = self
            .inner
            .col_find_by(&collection, &field, &value, js_to_scan_options(options))
            .map_err(js_err)?;
        Ok(entries.into_iter().map(doc_entry_to_js).collect())
    }

    /// Apply several puts/deletes atomically: either every operation commits
    /// or none does.
    #[napi]
    pub async fn col_batch(&self, ops: Vec<JsColOp>) -> Result<()> {
        let mut parsed = Vec::with_capacity(ops.len());
        for op in ops {
            parsed.push(core::ColOpDto {
                doc: op.json.as_deref().map(parse_json_doc).transpose()?,
                op: op.op,
                collection: op.collection,
                id: op.id,
                expected_version: op.expected_version.map(|v| v as u64),
            });
        }
        self.inner.col_batch(parsed).map_err(js_err)
    }

    #[napi]
    pub async fn query_hybrid(&self, query: JsHybridQuery) -> Result<Vec<JsHit>> {
        let hits = self
            .inner
            .query_hybrid(js_to_hybrid_query(query))
            .map_err(js_err)?;
        Ok(hits.into_iter().map(hit_to_js).collect())
    }

    /// Búsqueda vectorial sin filtros con la ruta del HNSW (capas y nodos visitados).
    #[napi(js_name = "traceVector")]
    pub async fn trace_vector(
        &self,
        vector: Float32Array,
        k: u32,
        ef_search: Option<u32>,
    ) -> Result<JsVectorTrace> {
        let trace = self
            .inner
            .trace_vector(&vector, k, ef_search)
            .map_err(js_err)?;
        Ok(JsVectorTrace {
            hits: trace.hits.into_iter().map(hit_to_js).collect(),
            steps: trace
                .steps
                .into_iter()
                .map(|s| JsTraceStep {
                    layer: s.layer,
                    from: s.from,
                    node: s.node,
                    distance: s.distance,
                })
                .collect(),
        })
    }

    #[napi]
    pub fn subscribe(
        &self,
        pattern: JsEventPattern,
        callback: ThreadsafeFunction<JsEvent>,
    ) -> Result<JsSubscription> {
        let subscription = self
            .inner
            .subscribe(js_to_event_pattern(pattern)?)
            .map_err(js_err)?;

        let handle = self.runtime.spawn(async move {
            let mut subscription = subscription;
            while let Some(event) = subscription.next().await {
                let js_event = event_to_js(&core::EventDto::from(&event));
                if callback.call(Ok(js_event), ThreadsafeFunctionCallMode::NonBlocking)
                    != napi::Status::Ok
                {
                    break;
                }
            }
        });

        Ok(JsSubscription { handle })
    }

    #[napi]
    pub fn close(&mut self) {
        self.inner.close();
    }
}

#[napi]
pub struct JsSubscription {
    handle: JoinHandle<()>,
}

#[napi]
impl JsSubscription {
    #[napi]
    pub fn close(&mut self) {
        self.handle.abort();
    }
}

impl Drop for JsSubscription {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
