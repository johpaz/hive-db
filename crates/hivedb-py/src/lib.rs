//! Binding Python de HiveDB (`hivedb._native`).
//!
//! API síncrona: cada llamada suelta el GIL mientras trabaja el motor, así que varios hilos de
//! Python pueden consultar la misma base a la vez. La capa Python (`hivedb/`) añade la API
//! amigable (`HiveDB`, `Collection`, `AsyncHiveDB`). Las entradas y salidas son `dict`/`list`
//! convertidos con serde a los DTOs de `hivedb-binding-core`, el mismo crate que usa el binding
//! Node, de modo que ambos validan y responden igual.

use hivedb_binding_core as core;
use pyo3::exceptions::{PyException, PyTimeoutError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pythonize::{depythonize, pythonize};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pyo3::create_exception!(
    _native,
    HiveDBError,
    PyException,
    "Error de HiveDB. `code` es uno de INVALID_VECTOR, VECTOR_SPACE_MISMATCH, INDEX_DEGRADED, \
     EMBEDDER_UNAVAILABLE, o `None` si el error no tiene código."
);

/// Cada cuánto se comprueban las señales (Ctrl-C) mientras se espera en Rust sin el GIL.
const SIGNAL_POLL: Duration = Duration::from_millis(100);

fn to_py_err(py: Python<'_>, error: core::BindingError) -> PyErr {
    let code = error.code.map(core::ErrorCode::as_str);
    let err = HiveDBError::new_err(error.message);
    // `code` como atributo de la instancia para que `except HiveDBError as e: e.code` funcione.
    let _ = err.value(py).setattr("code", code);
    err
}

fn de<T: DeserializeOwned>(obj: &Bound<'_, PyAny>) -> PyResult<T> {
    depythonize(obj).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn de_opt<T: DeserializeOwned>(obj: Option<&Bound<'_, PyAny>>) -> PyResult<Option<T>> {
    match obj {
        Some(obj) if !obj.is_none() => de(obj).map(Some),
        _ => Ok(None),
    }
}

fn to_py<'py, T: Serialize + ?Sized>(py: Python<'py>, value: &T) -> PyResult<Bound<'py, PyAny>> {
    pythonize(py, value).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Ejecuta `f` sin el GIL y convierte el error del motor.
fn run<T: Send>(py: Python<'_>, f: impl FnOnce() -> core::Result<T> + Send) -> PyResult<T> {
    py.detach(f).map_err(|e| to_py_err(py, e))
}

// ---------------------------------------------------------------------------------------------
// Base de datos
// ---------------------------------------------------------------------------------------------

#[pyclass(frozen, module = "hivedb._native")]
struct Database {
    inner: core::Handle,
}

#[pymethods]
impl Database {
    /// Abre la base. `options` admite `{"vector": {"dimension", "space_id"}, "embedder": "local"}`.
    /// Con `embedder="local"` la primera vez descarga el modelo (~470 MB) si falta.
    #[staticmethod]
    #[pyo3(signature = (path, options=None))]
    fn open(py: Python<'_>, path: String, options: Option<Bound<'_, PyAny>>) -> PyResult<Self> {
        let options: core::OpenOptionsDto = de_opt(options.as_ref())?.unwrap_or_default();
        let inner = run(py, move || {
            let embedder = core::resolve_embedder(options.embedder.as_deref())?;
            core::Handle::open(&path, options.vector, embedder)
        })?;
        Ok(Self { inner })
    }

    // -- registro de eventos ------------------------------------------------------------------

    fn append(&self, py: Python<'_>, input: &Bound<'_, PyAny>) -> PyResult<u64> {
        let input: core::EventInputDto = de(input)?;
        run(py, || self.inner.append(input))
    }

    fn read<'py>(&self, py: Python<'py>, seq: u64) -> PyResult<Bound<'py, PyAny>> {
        let event = run(py, || self.inner.read(seq))?;
        to_py(py, &event)
    }

    fn log_len(&self, py: Python<'_>) -> PyResult<u64> {
        run(py, || self.inner.log_len())
    }

    fn last_seq(&self, py: Python<'_>) -> PyResult<u64> {
        run(py, || self.inner.last_seq())
    }

    // -- proyecciones -------------------------------------------------------------------------

    #[pyo3(signature = (tool, agents=None))]
    fn tool_stats<'py>(
        &self,
        py: Python<'py>,
        tool: String,
        agents: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let stats = run(py, || self.inner.tool_stats(&tool, agents.as_deref()))?;
        to_py(py, &stats)
    }

    fn project_task_state(
        &self,
        py: Python<'_>,
        agent_id: String,
        stream_id: String,
    ) -> PyResult<Option<String>> {
        run(py, || self.inner.project_task_state(&agent_id, &stream_id))
    }

    fn can<'py>(
        &self,
        py: Python<'py>,
        agent: String,
        action: String,
        resource: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let decision = run(py, || self.inner.can(&agent, &action, &resource))?;
        to_py(py, &decision)
    }

    // -- memoria de trabajo -------------------------------------------------------------------

    #[pyo3(signature = (agent_id, key, value, ttl_ms=None))]
    fn working_set(
        &self,
        py: Python<'_>,
        agent_id: String,
        key: String,
        value: &Bound<'_, PyAny>,
        ttl_ms: Option<i64>,
    ) -> PyResult<()> {
        let value: Value = de(value)?;
        run(py, || {
            self.inner.working_set(&agent_id, &key, value, ttl_ms)
        })
    }

    fn working_get<'py>(
        &self,
        py: Python<'py>,
        agent_id: String,
        key: String,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        match run(py, || self.inner.working_get(&agent_id, &key))? {
            Some(value) => Ok(Some(to_py(py, &value)?)),
            None => Ok(None),
        }
    }

    fn working_keys(&self, py: Python<'_>, agent_id: String) -> PyResult<Vec<String>> {
        run(py, || self.inner.working_keys(&agent_id))
    }

    // -- contexto y harness -------------------------------------------------------------------

    #[pyo3(signature = (stream_id, agents=None))]
    fn causal_thread<'py>(
        &self,
        py: Python<'py>,
        stream_id: String,
        agents: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let thread = run(py, || {
            self.inner.causal_thread(&stream_id, agents.as_deref())
        })?;
        to_py(py, &thread)
    }

    fn build_agent_context<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request: Value = de(request)?;
        let context = run(py, || self.inner.build_agent_context(request))?;
        to_py(py, &context)
    }

    // -- índice semántico ---------------------------------------------------------------------

    fn upsert_doc(&self, py: Python<'_>, doc: &Bound<'_, PyAny>) -> PyResult<()> {
        let doc: core::IndexDocDto = de(doc)?;
        run(py, || self.inner.upsert_doc(doc))
    }

    fn upsert_batch(&self, py: Python<'_>, docs: &Bound<'_, PyAny>) -> PyResult<()> {
        let docs: Vec<core::IndexDocDto> = de(docs)?;
        run(py, || self.inner.upsert_batch(docs))
    }

    fn delete_doc(&self, py: Python<'_>, id: String) -> PyResult<()> {
        run(py, || self.inner.delete_doc(&id))
    }

    fn delete_by_filter(&self, py: Python<'_>, filter: &Bound<'_, PyAny>) -> PyResult<()> {
        let filter: core::ScalarFilterDto = de(filter)?;
        run(py, || self.inner.delete_by_filter(filter))
    }

    fn clear_index(&self, py: Python<'_>) -> PyResult<()> {
        run(py, || self.inner.clear_index())
    }

    fn compact_index(&self, py: Python<'_>) -> PyResult<()> {
        run(py, || self.inner.compact_index())
    }

    fn query_hybrid<'py>(
        &self,
        py: Python<'py>,
        query: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let query: core::HybridQueryDto = de(query)?;
        let hits = run(py, || self.inner.query_hybrid(query))?;
        to_py(py, &hits)
    }

    #[pyo3(signature = (vector, k, ef_search=None))]
    fn trace_vector<'py>(
        &self,
        py: Python<'py>,
        vector: Vec<f32>,
        k: u32,
        ef_search: Option<u32>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let trace = run(py, || self.inner.trace_vector(&vector, k, ef_search))?;
        to_py(py, &trace)
    }

    // -- colecciones --------------------------------------------------------------------------

    #[pyo3(signature = (collection, id, doc, expected_version=None))]
    fn col_put(
        &self,
        py: Python<'_>,
        collection: String,
        id: String,
        doc: &Bound<'_, PyAny>,
        expected_version: Option<u64>,
    ) -> PyResult<u64> {
        let doc: Value = de(doc)?;
        run(py, || {
            self.inner.col_put(&collection, &id, &doc, expected_version)
        })
    }

    fn col_get<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        id: String,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        match run(py, || self.inner.col_get(&collection, &id))? {
            Some(entry) => Ok(Some(to_py(py, &entry)?)),
            None => Ok(None),
        }
    }

    fn col_delete(&self, py: Python<'_>, collection: String, id: String) -> PyResult<bool> {
        run(py, || self.inner.col_delete(&collection, &id))
    }

    #[pyo3(signature = (collection, options=None))]
    fn col_scan<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        options: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let options: Option<core::ScanOptionsDto> = de_opt(options.as_ref())?;
        let entries = run(py, || self.inner.col_scan(&collection, options))?;
        to_py(py, &entries)
    }

    fn col_count(&self, py: Python<'_>, collection: String) -> PyResult<u64> {
        run(py, || self.inner.col_count(&collection))
    }

    #[pyo3(signature = (collection, field, unique=false))]
    fn col_create_index(
        &self,
        py: Python<'_>,
        collection: String,
        field: String,
        unique: bool,
    ) -> PyResult<()> {
        run(py, || {
            self.inner.col_create_index(&collection, &field, unique)
        })
    }

    #[pyo3(signature = (collection, field, value, options=None))]
    fn col_find_by<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        field: String,
        value: &Bound<'py, PyAny>,
        options: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let value: Value = de(value)?;
        let options: Option<core::ScanOptionsDto> = de_opt(options.as_ref())?;
        let entries = run(py, || {
            self.inner.col_find_by(&collection, &field, &value, options)
        })?;
        to_py(py, &entries)
    }

    fn col_batch(&self, py: Python<'_>, ops: &Bound<'_, PyAny>) -> PyResult<()> {
        let ops: Vec<core::ColOpDto> = de(ops)?;
        run(py, || self.inner.col_batch(ops))
    }

    // -- suscripciones ------------------------------------------------------------------------

    fn subscribe(&self, py: Python<'_>, pattern: &Bound<'_, PyAny>) -> PyResult<Subscription> {
        let pattern: core::EventPatternDto = de(pattern)?;
        let subscription = self
            .inner
            .subscribe(pattern)
            .map_err(|e| to_py_err(py, e))?;
        Ok(Subscription::start(subscription))
    }

    fn close(&self, py: Python<'_>) {
        // `close()` espera a las operaciones en curso: sin el GIL, para no bloquear a las que lo necesitan.
        py.detach(|| self.inner.close());
    }
}

// ---------------------------------------------------------------------------------------------
// Suscripción
// ---------------------------------------------------------------------------------------------

/// Eventos que coinciden con un patrón. Iterable (bloquea hasta el siguiente evento) y con
/// `next(timeout)`; `close()` la cancela desde cualquier hilo.
#[pyclass(frozen, module = "hivedb._native")]
struct Subscription {
    events: Mutex<mpsc::Receiver<core::EventDto>>,
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl Subscription {
    /// Un hilo propio consume la suscripción asíncrona del motor y reenvía los eventos por un canal.
    fn start(mut subscription: core::Subscription) -> Self {
        let (events_tx, events) = mpsc::channel();
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("hivedb-subscription".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread().build() else {
                    return;
                };
                runtime.block_on(async move {
                    loop {
                        tokio::select! {
                            _ = &mut stop_rx => break,
                            event = subscription.next() => match event {
                                Some(event) => {
                                    if events_tx.send(core::EventDto::from(&event)).is_err() {
                                        break;
                                    }
                                }
                                None => break,
                            },
                        }
                    }
                });
            })
            .expect("no se pudo crear el hilo de la suscripción");
        Self {
            events: Mutex::new(events),
            stop: Mutex::new(Some(stop_tx)),
        }
    }

    /// Espera el siguiente evento; `Ok(None)` si la suscripción se cerró.
    fn wait(&self, py: Python<'_>, timeout: Option<Duration>) -> PyResult<Option<core::EventDto>> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let slice = match deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(PyTimeoutError::new_err("no llegó ningún evento a tiempo"));
                    }
                    left.min(SIGNAL_POLL)
                }
                None => SIGNAL_POLL,
            };
            let received = py.detach(|| {
                let events = self.events.lock().unwrap_or_else(|p| p.into_inner());
                events.recv_timeout(slice)
            });
            match received {
                Ok(event) => return Ok(Some(event)),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Timeout) => py.check_signals()?,
            }
        }
    }
}

#[pymethods]
impl Subscription {
    /// Siguiente evento como `dict`; `None` si la suscripción se cerró. Con `timeout` (segundos)
    /// lanza `TimeoutError` si no llega ninguno.
    #[pyo3(signature = (timeout=None))]
    fn next<'py>(
        &self,
        py: Python<'py>,
        timeout: Option<f64>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let timeout = timeout.map(|s| Duration::from_secs_f64(s.max(0.0)));
        match self.wait(py, timeout)? {
            Some(event) => Ok(Some(to_py(py, &event)?)),
            None => Ok(None),
        }
    }

    fn close(&self) {
        if let Some(stop) = self.stop.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = stop.send(());
        }
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.next(py, None)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------------------------
// Embedder local
// ---------------------------------------------------------------------------------------------

enum Prepare {
    Progress(core::ModelProgressDto),
    Done(core::Result<core::PreparedModelDto>),
}

/// Descarga (si falta) y verifica el modelo del embedder local, sin abrir ninguna base.
/// `on_progress` (opcional) recibe un `dict` por cada avance, siempre desde el hilo que llama.
#[pyfunction]
#[pyo3(signature = (on_progress=None))]
fn prepare_embedder<'py>(
    py: Python<'py>,
    on_progress: Option<Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let (tx, rx) = mpsc::channel();
    let rx = Mutex::new(rx);
    std::thread::Builder::new()
        .name("hivedb-prepare-embedder".into())
        .spawn(move || {
            let progress_tx = tx.clone();
            let result = core::prepare_embedder(move |progress| {
                let _ = progress_tx.send(Prepare::Progress(progress));
            });
            let _ = tx.send(Prepare::Done(result));
        })
        .map_err(|e| HiveDBError::new_err(e.to_string()))?;

    loop {
        let message = py.detach(|| {
            rx.lock()
                .unwrap_or_else(|p| p.into_inner())
                .recv_timeout(SIGNAL_POLL)
        });
        match message {
            Ok(Prepare::Progress(progress)) => {
                if let Some(callback) = on_progress.as_ref() {
                    // Los errores del callback se descartan, como en el binding Node.
                    let _ = callback.call1((to_py(py, &progress)?,));
                }
            }
            Ok(Prepare::Done(result)) => {
                let model = result.map_err(|e| to_py_err(py, e))?;
                return to_py(py, &model);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => py.check_signals()?,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(HiveDBError::new_err(
                    "model download thread ended unexpectedly",
                ));
            }
        }
    }
}

/// Evalúa una tarea con el bucle del harness (no necesita base abierta).
#[pyfunction]
fn evaluate_harness<'py>(
    py: Python<'py>,
    input: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let input: Value = de(input)?;
    let evaluation = core::evaluate_harness(input).map_err(|e| to_py_err(py, e))?;
    to_py(py, &evaluation)
}

#[pymodule]
fn _native(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Database>()?;
    m.add_class::<Subscription>()?;
    m.add_function(wrap_pyfunction!(prepare_embedder, m)?)?;
    m.add_function(wrap_pyfunction!(evaluate_harness, m)?)?;
    m.add("HiveDBError", py.get_type::<HiveDBError>())?;
    let codes = PyDict::new(py);
    for code in core::ErrorCode::ALL {
        codes.set_item(code.as_str(), code.as_str())?;
    }
    m.add("ERROR_CODES", codes)?;
    Ok(())
}
