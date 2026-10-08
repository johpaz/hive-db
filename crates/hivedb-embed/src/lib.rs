//! Embedder local de HiveDB: `intfloat/multilingual-e5-small` sobre `candle`.
//!
//! Rust puro (sin ONNX Runtime ni librerías C++), así que compila igual para
//! todos los targets de HiveDB. El modelo (MIT, 384 dimensiones, español e
//! inglés entre otros) pesa unos 470 MB y se descarga la primera vez a una
//! caché local; las siguientes aperturas lo leen de disco.
//!
//! ```no_run
//! use hivedb_embed::LocalEmbedder;
//! let embedder = LocalEmbedder::multilingual_e5_small().unwrap();
//! ```

mod download;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use hivedb_index::{EmbedKind, Embedder, IndexError};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

pub use download::{
    ModelFiles, Progress, default_cache_dir, ensure_multilingual_e5_small,
    ensure_multilingual_e5_small_with,
};

/// Instancia compartida del proceso (ver [`LocalEmbedder::shared`]).
static SHARED: Mutex<Option<Arc<LocalEmbedder>>> = Mutex::new(None);

/// Devuelve la instancia guardada en `slot` o la carga con `load`. El candado se mantiene
/// durante la carga a propósito: dos peticiones simultáneas no cargan dos copias.
fn shared_or_load<T>(
    slot: &Mutex<Option<Arc<T>>>,
    load: impl FnOnce() -> hivedb_index::Result<T>,
) -> hivedb_index::Result<Arc<T>> {
    let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = guard.as_ref() {
        return Ok(Arc::clone(existing));
    }
    let loaded = Arc::new(load()?);
    *guard = Some(Arc::clone(&loaded));
    Ok(loaded)
}

/// Textos por pasada del modelo. Acota la memoria de activaciones.
const BATCH_SIZE: usize = 32;
/// Longitud máxima del modelo; lo que exceda se trunca.
const MAX_TOKENS: usize = 512;

/// Embedder local con `multilingual-e5-small` (o un modelo BERT compatible con
/// el mismo esquema de prefijos `query:` / `passage:` y pooling de media).
pub struct LocalEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    space_id: String,
    dimension: usize,
}

impl std::fmt::Debug for LocalEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEmbedder")
            .field("space_id", &self.space_id)
            .field("dimension", &self.dimension)
            .finish_non_exhaustive()
    }
}

fn fail(context: &str, cause: impl std::fmt::Display) -> IndexError {
    IndexError::Embedder(format!("{context}: {cause}"))
}

impl LocalEmbedder {
    /// `multilingual-e5-small`, descargándolo a la caché local si hace falta.
    ///
    /// Necesita red solo la primera vez. Con `HIVEDB_OFFLINE=1` nunca descarga.
    /// La caché se puede mover con `HIVEDB_MODEL_DIR`.
    pub fn multilingual_e5_small() -> hivedb_index::Result<Self> {
        let files = ensure_multilingual_e5_small()?;
        Self::from_dir(files.dir(), files.space_id())
    }

    /// El embedder compartido por todo el proceso: **una sola copia del modelo en memoria**
    /// (~735 MiB) para todas las bases que lo usen, en lugar de una por base abierta.
    ///
    /// La primera llamada carga el modelo (y lo descarga si falta); las siguientes devuelven la
    /// misma instancia al instante. Si varios hilos lo piden a la vez, solo uno lo carga y los
    /// demás esperan. Un fallo no se guarda: la siguiente llamada vuelve a intentarlo. La copia
    /// vive hasta que termina el proceso (`LocalEmbedder` es `Sync`: varias consultas la usan a
    /// la vez).
    pub fn shared() -> hivedb_index::Result<Arc<Self>> {
        shared_or_load(&SHARED, Self::multilingual_e5_small)
    }

    /// Como [`LocalEmbedder::multilingual_e5_small`], avisando del avance de la descarga
    /// (solo hay avance la primera vez, cuando el modelo no está en la caché).
    pub fn multilingual_e5_small_with(
        on_progress: &mut dyn FnMut(&Progress),
    ) -> hivedb_index::Result<Self> {
        let files = ensure_multilingual_e5_small_with(on_progress)?;
        Self::from_dir(files.dir(), files.space_id())
    }

    /// Carga un modelo desde un directorio con `config.json`, `tokenizer.json`
    /// y `model.safetensors`. `space_id` identifica el espacio vectorial: debe
    /// cambiar si cambia el modelo.
    pub fn from_dir(dir: &Path, space_id: impl Into<String>) -> hivedb_index::Result<Self> {
        let config: Config = serde_json::from_slice(
            &std::fs::read(dir.join("config.json")).map_err(|e| fail("config.json", e))?,
        )
        .map_err(|e| fail("config.json", e))?;
        let dimension = config.hidden_size;

        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| fail("tokenizer.json", e))?;
        let pad_id = tokenizer
            .token_to_id("<pad>")
            .or_else(|| tokenizer.token_to_id("[PAD]"))
            .unwrap_or(0);
        let pad_token = tokenizer
            .id_to_token(pad_id)
            .unwrap_or_else(|| "<pad>".into());
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            pad_id,
            pad_token,
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| fail("truncation", e))?;

        let device = Device::Cpu;
        let weights = dir.join("model.safetensors");
        // SAFETY: el archivo se mapea en solo lectura. Si otro proceso lo
        // modificara mientras está mapeado, el comportamiento sería indefinido;
        // la caché solo se escribe por rename atómico y nunca in situ.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights], DType::F32, &device) }
            .map_err(|e| fail("model.safetensors", e))?;
        let model = BertModel::load(vb, &config).map_err(|e| fail("cargando el modelo", e))?;

        Ok(Self {
            model,
            tokenizer,
            device,
            space_id: space_id.into(),
            dimension,
        })
    }

    fn embed_batch(&self, texts: &[String]) -> candle_core::Result<Vec<Vec<f32>>> {
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| candle_core::Error::Msg(e.to_string()))?;
        let rows = encodings.len();
        let width = encodings.first().map_or(0, |e| e.get_ids().len());

        let ids: Vec<u32> = encodings
            .iter()
            .flat_map(|e| e.get_ids().to_vec())
            .collect();
        let mask: Vec<u32> = encodings
            .iter()
            .flat_map(|e| e.get_attention_mask().to_vec())
            .collect();
        let input_ids = Tensor::from_vec(ids, (rows, width), &self.device)?;
        let attention = Tensor::from_vec(mask, (rows, width), &self.device)?;
        let token_types = input_ids.zeros_like()?;

        let hidden = self
            .model
            .forward(&input_ids, &token_types, Some(&attention))?;

        // Media de los tokens reales (la máscara excluye el relleno) y
        // normalización L2, que es lo que espera el espacio coseno del índice.
        let mask = attention.to_dtype(DType::F32)?.unsqueeze(2)?;
        let summed = hidden.broadcast_mul(&mask)?.sum(1)?;
        let counts = mask.sum(1)?;
        let mean = summed.broadcast_div(&counts)?;
        let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?;
        mean.broadcast_div(&norm)?.to_vec2::<f32>()
    }
}

impl Embedder for LocalEmbedder {
    fn space_id(&self) -> &str {
        &self.space_id
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn embed(&self, texts: &[&str], kind: EmbedKind) -> hivedb_index::Result<Vec<Vec<f32>>> {
        let prefix = match kind {
            EmbedKind::Document => "passage: ",
            EmbedKind::Query => "query: ",
        };
        // Se procesa por longitud creciente: un lote se rellena hasta su texto más
        // largo, y mezclar frases cortas con largas desperdicia la mayor parte
        // del cálculo. El resultado se devuelve en el orden de entrada.
        let mut order: Vec<usize> = (0..texts.len()).collect();
        order.sort_by_key(|&i| texts[i].len());
        let mut out: Vec<Vec<f32>> = vec![Vec::new(); texts.len()];
        for chunk in order.chunks(BATCH_SIZE) {
            let batch: Vec<String> = chunk
                .iter()
                .map(|&i| format!("{prefix}{}", texts[i]))
                .collect();
            let vectors = self
                .embed_batch(&batch)
                .map_err(|e| fail("generando embeddings", e))?;
            for (&i, vector) in chunk.iter().zip(vectors) {
                out[i] = vector;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn la_instancia_compartida_se_carga_una_sola_vez() {
        let slot: Mutex<Option<Arc<u32>>> = Mutex::new(None);
        let loads = AtomicUsize::new(0);
        let results: Vec<Arc<u32>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        shared_or_load(&slot, || {
                            loads.fetch_add(1, Ordering::SeqCst);
                            // La carga real tarda: los demás hilos deben esperar, no cargar otra.
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            Ok(7)
                        })
                        .unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(results.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
    }

    #[test]
    fn un_fallo_de_carga_no_se_guarda() {
        let slot: Mutex<Option<Arc<u32>>> = Mutex::new(None);
        let error = shared_or_load(&slot, || Err(IndexError::Embedder("sin red".into())));
        assert!(error.is_err());
        // La siguiente petición lo intenta de nuevo y funciona.
        assert_eq!(*shared_or_load(&slot, || Ok(3)).unwrap(), 3);
    }
}
