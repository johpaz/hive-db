//! Contrato para generar embeddings dentro de HiveDB.
//!
//! El motor no depende de ningún modelo: un `Embedder` es cualquier cosa que
//! convierta texto en vectores de dimensión fija. La implementación local con
//! `candle` vive en el crate `hivedb-embed`, tras una feature de Cargo.

/// Para qué se va a usar el vector. Algunos modelos (la familia E5, por
/// ejemplo) usan prefijos distintos para documentos y consultas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedKind {
    /// Texto que se indexa.
    Document,
    /// Texto con el que se busca.
    Query,
}

/// Convierte texto en vectores de dimensión fija.
///
/// Los vectores devueltos deben tener exactamente [`Embedder::dimension`]
/// componentes y ser finitos y de norma no nula; el índice los valida.
pub trait Embedder: Send + Sync + std::fmt::Debug {
    /// Identidad estable del espacio vectorial (modelo + versión + variante).
    /// Dos embedders con el mismo `space_id` deben producir vectores
    /// comparables: es lo que impide mezclar modelos en una misma base.
    fn space_id(&self) -> &str;

    /// Número de componentes de cada vector.
    fn dimension(&self) -> usize;

    /// Embebe un lote de textos. Devuelve un vector por texto, en el mismo orden.
    fn embed(&self, texts: &[&str], kind: EmbedKind) -> crate::Result<Vec<Vec<f32>>>;
}
