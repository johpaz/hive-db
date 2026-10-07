//! Integración del trait `Embedder` con `HiveDB`, con un embedder falso y
//! determinista (sin modelo): comprueba el cableado, no la calidad semántica.

use hivedb_core::{HiveDB, HybridQuery, IndexDoc, OpenOptions, VectorOptions};
use hivedb_index::{EmbedKind, Embedder, IndexError};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::tempdir;

const DIM: usize = 32;

/// Cada grupo de sinónimos comparte una dimensión; el resto de palabras se
/// reparten por hash. Así "correo" y "email" producen el mismo vector aunque no
/// compartan ningún término para BM25.
const GRUPOS: &[&[&str]] = &[
    &["correo", "email", "mail"],
    &["pago", "payment", "cobro"],
    &["envio", "shipping", "entrega"],
];

#[derive(Debug, Default)]
struct FakeEmbedder {
    llamadas: AtomicUsize,
    textos: AtomicUsize,
    falla: bool,
}

impl FakeEmbedder {
    fn vector(texto: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; DIM];
        for palabra in texto.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if palabra.is_empty() {
                continue;
            }
            let dim = match GRUPOS.iter().position(|g| g.contains(&palabra)) {
                Some(i) => i,
                None => 8 + palabra.bytes().map(usize::from).sum::<usize>() % (DIM - 9),
            };
            v[dim] += 1.0;
        }
        if v.iter().all(|x| *x == 0.0) {
            v[DIM - 1] = 1.0;
        }
        let norma = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norma);
        v
    }
}

impl Embedder for FakeEmbedder {
    fn space_id(&self) -> &str {
        "fake:v1"
    }

    fn dimension(&self) -> usize {
        DIM
    }

    fn embed(&self, texts: &[&str], _kind: EmbedKind) -> hivedb_index::Result<Vec<Vec<f32>>> {
        self.llamadas.fetch_add(1, Ordering::SeqCst);
        self.textos.fetch_add(texts.len(), Ordering::SeqCst);
        if self.falla {
            return Err(IndexError::Embedder("modelo no disponible".into()));
        }
        Ok(texts.iter().map(|t| Self::vector(t)).collect())
    }
}

fn abrir(embedder: Arc<FakeEmbedder>) -> HiveDB {
    HiveDB::open_temp_with_options(OpenOptions {
        embedder: Some(embedder),
        ..Default::default()
    })
    .unwrap()
}

fn ids(hits: &[hivedb_core::Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

#[test]
fn un_documento_solo_con_texto_se_encuentra_por_significado() {
    let embedder = Arc::new(FakeEmbedder::default());
    let db = abrir(embedder);
    db.upsert_doc(&IndexDoc::new("a").with_body("recibir un email de confirmación"))
        .unwrap();
    db.upsert_doc(&IndexDoc::new("b").with_body("política de reembolsos"))
        .unwrap();

    // "correo" no aparece en ningún documento: solo el vector puede acertar.
    let hits = db
        .query_hybrid(HybridQuery::default().with_text("correo").with_k(1))
        .unwrap();
    assert_eq!(ids(&hits), ["a"]);
}

#[test]
fn sin_embedder_la_misma_consulta_no_encuentra_nada() {
    let db = HiveDB::open_temp().unwrap();
    db.upsert_doc(&IndexDoc::new("a").with_body("recibir un email de confirmación"))
        .unwrap();
    let hits = db
        .query_hybrid(HybridQuery::default().with_text("correo").with_k(1))
        .unwrap();
    assert!(hits.is_empty());
}

#[test]
fn un_lote_se_embebe_en_una_sola_llamada() {
    let embedder = Arc::new(FakeEmbedder::default());
    let db = abrir(Arc::clone(&embedder));
    let docs: Vec<IndexDoc> = (0..5)
        .map(|i| IndexDoc::new(format!("d{i}")).with_body(format!("documento {i} sobre pago")))
        .collect();
    db.upsert_batch(&docs).unwrap();
    assert_eq!(embedder.llamadas.load(Ordering::SeqCst), 1);
    assert_eq!(embedder.textos.load(Ordering::SeqCst), 5);
}

#[test]
fn un_vector_explicito_tiene_prioridad_y_no_se_reembebe() {
    let embedder = Arc::new(FakeEmbedder::default());
    let db = abrir(Arc::clone(&embedder));
    let propio = FakeEmbedder::vector("envio");
    db.upsert_doc(
        &IndexDoc::new("a")
            .with_body("pago")
            .with_vector(propio.clone()),
    )
    .unwrap();
    assert_eq!(embedder.llamadas.load(Ordering::SeqCst), 0);

    // Y una consulta con vector propio tampoco llama al embedder.
    let hits = db
        .query_hybrid(HybridQuery::default().with_vector(propio).with_k(1))
        .unwrap();
    assert_eq!(ids(&hits), ["a"]);
    assert_eq!(embedder.llamadas.load(Ordering::SeqCst), 0);
}

#[test]
fn un_documento_sin_texto_no_se_embebe() {
    let embedder = Arc::new(FakeEmbedder::default());
    let db = abrir(Arc::clone(&embedder));
    db.upsert_doc(&IndexDoc::new("solo-id")).unwrap();
    db.upsert_doc(&IndexDoc::new("vacio").with_body("   "))
        .unwrap();
    assert_eq!(embedder.llamadas.load(Ordering::SeqCst), 0);
}

#[test]
fn una_consulta_vacia_no_llama_al_embedder() {
    let embedder = Arc::new(FakeEmbedder::default());
    let db = abrir(Arc::clone(&embedder));
    db.upsert_doc(&IndexDoc::new("a").with_body("pago"))
        .unwrap();
    let antes = embedder.llamadas.load(Ordering::SeqCst);
    let _ = db.query_hybrid(HybridQuery::default().with_text("  ").with_k(3));
    assert_eq!(embedder.llamadas.load(Ordering::SeqCst), antes);
}

#[test]
fn un_fallo_del_embedder_se_propaga_con_su_codigo() {
    let embedder = Arc::new(FakeEmbedder {
        falla: true,
        ..Default::default()
    });
    let db = abrir(embedder);
    let error = db
        .upsert_doc(&IndexDoc::new("a").with_body("texto"))
        .unwrap_err();
    assert!(
        error.to_string().contains("EMBEDDER_UNAVAILABLE:"),
        "{error}"
    );
}

#[test]
fn un_espacio_explicito_distinto_al_del_embedder_se_rechaza() {
    let resultado = HiveDB::open_temp_with_options(OpenOptions {
        vector: Some(VectorOptions::new(DIM, "otro:espacio")),
        embedder: Some(Arc::new(FakeEmbedder::default())),
    });
    let error = resultado.err().expect("debe fallar");
    assert!(
        error.to_string().contains("VECTOR_SPACE_MISMATCH:"),
        "{error}"
    );
}

#[test]
fn un_espacio_explicito_igual_al_del_embedder_se_acepta() {
    HiveDB::open_temp_with_options(OpenOptions {
        vector: Some(VectorOptions::new(DIM, "fake:v1")),
        embedder: Some(Arc::new(FakeEmbedder::default())),
    })
    .unwrap();
}

#[test]
fn reabrir_con_otro_modelo_se_rechaza() {
    #[derive(Debug)]
    struct Otro;
    impl Embedder for Otro {
        fn space_id(&self) -> &str {
            "fake:v2"
        }
        fn dimension(&self) -> usize {
            DIM
        }
        fn embed(&self, texts: &[&str], _: EmbedKind) -> hivedb_index::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|t| FakeEmbedder::vector(t)).collect())
        }
    }

    let dir = tempdir().unwrap();
    {
        let db = HiveDB::open_with_options(
            dir.path(),
            OpenOptions {
                embedder: Some(Arc::new(FakeEmbedder::default())),
                ..Default::default()
            },
        )
        .unwrap();
        db.upsert_doc(&IndexDoc::new("a").with_body("pago"))
            .unwrap();
    }
    let reabierto = HiveDB::open_with_options(
        dir.path(),
        OpenOptions {
            embedder: Some(Arc::new(Otro)),
            ..Default::default()
        },
    );
    let error = reabierto.err().expect("otro modelo debe rechazarse");
    assert!(
        error.to_string().contains("VECTOR_SPACE_MISMATCH:"),
        "{error}"
    );
}
