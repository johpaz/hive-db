//! Pruebas con el modelo real (`multilingual-e5-small`, ~470 MB).
//!
//! Van marcadas `#[ignore]` para no descargar el modelo en cada `cargo test`.
//! Se ejecutan con: `cargo test -p hivedb-embed -- --ignored`
//! (usa `HIVEDB_MODEL_DIR` para reutilizar una caché existente).

use hivedb_core::{HiveDB, HybridQuery, IndexDoc, OpenOptions};
use hivedb_embed::LocalEmbedder;
use hivedb_index::{EmbedKind, Embedder};
use std::sync::{Arc, OnceLock};

fn embedder() -> &'static Arc<LocalEmbedder> {
    static EMBEDDER: OnceLock<Arc<LocalEmbedder>> = OnceLock::new();
    EMBEDDER.get_or_init(|| Arc::new(LocalEmbedder::multilingual_e5_small().unwrap()))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[test]
#[ignore = "requiere el modelo (470 MB)"]
fn los_vectores_tienen_la_dimension_y_estan_normalizados() {
    let e = embedder();
    assert_eq!(e.dimension(), 384);
    let v = e.embed(&["hola mundo"], EmbedKind::Document).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].len(), 384);
    let norma = dot(&v[0], &v[0]).sqrt();
    assert!((norma - 1.0).abs() < 1e-4, "norma {norma}");
    assert!(v[0].iter().all(|x| x.is_finite()));
}

#[test]
#[ignore = "requiere el modelo (470 MB)"]
fn el_significado_gana_a_las_palabras_en_comun() {
    let e = embedder();
    let q = &e.embed(&["correo electrónico"], EmbedKind::Query).unwrap()[0];
    let docs = e
        .embed(
            &[
                "Send an email message to the customer",
                "Receta de paella valenciana con mariscos",
                "El motor de la bicicleta necesita aceite",
            ],
            EmbedKind::Document,
        )
        .unwrap();
    let sims: Vec<f32> = docs.iter().map(|d| dot(q, d)).collect();
    assert!(sims[0] > sims[1] && sims[0] > sims[2], "{sims:?}");
}

#[test]
#[ignore = "requiere el modelo (470 MB)"]
fn el_resultado_no_depende_del_relleno_del_lote() {
    let e = embedder();
    let corto = "pago rechazado";
    let largo = "Este es un texto bastante más largo que obliga al lote a rellenar el corto con tokens vacíos para igualar la longitud";
    let solo = &e.embed(&[corto], EmbedKind::Document).unwrap()[0];
    let en_lote = &e.embed(&[corto, largo], EmbedKind::Document).unwrap()[0];
    let similitud = dot(solo, en_lote);
    assert!(similitud > 0.9999, "similitud {similitud}");
}

#[test]
#[ignore = "requiere el modelo (470 MB)"]
fn admite_texto_vacio_y_texto_muy_largo() {
    let e = embedder();
    let largo = "palabra ".repeat(5_000);
    let v = e.embed(&["", &largo], EmbedKind::Document).unwrap();
    assert_eq!(v.len(), 2);
    assert!(v.iter().flatten().all(|x| x.is_finite()));
}

#[test]
#[ignore = "requiere el modelo (470 MB)"]
fn hivedb_encuentra_correo_con_un_documento_que_dice_email() {
    let db = HiveDB::open_temp_with_options(OpenOptions {
        embedder: Some(Arc::clone(embedder()) as Arc<dyn Embedder>),
        ..Default::default()
    })
    .unwrap();
    db.upsert_batch(&[
        IndexDoc::new("email")
            .with_body("Cómo configurar tu cuenta de email y la bandeja de entrada"),
        IndexDoc::new("paella").with_body("Receta tradicional de paella valenciana"),
        IndexDoc::new("bici").with_body("Mantenimiento de la cadena de la bicicleta"),
        IndexDoc::new("refund").with_body("Refund policy for cancelled orders"),
    ])
    .unwrap();

    let hits = db
        .query_hybrid(
            HybridQuery::default()
                .with_text("correo electrónico")
                .with_k(1),
        )
        .unwrap();
    assert_eq!(hits[0].id, "email");

    let hits = db
        .query_hybrid(
            HybridQuery::default()
                .with_text("devolución de dinero")
                .with_k(1),
        )
        .unwrap();
    assert_eq!(hits[0].id, "refund");
}
