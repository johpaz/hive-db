use hivedb_core::{HiveDB, HybridQuery, IndexDoc, OpenOptions, VectorOptions};
use std::sync::{Arc, Barrier};

fn vector(coordinate: usize) -> Vec<f32> {
    let mut vector = vec![0.0; 8];
    vector[coordinate] = 1.0;
    vector
}

#[test]
fn concurrent_upserts_never_mix_text_and_vector_generations() {
    let db = Arc::new(
        HiveDB::open_temp_with_options(OpenOptions {
            vector: Some(VectorOptions::new(8, "test:8")),
        })
        .unwrap(),
    );
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for (body, coordinate) in [("alpha-unique", 0), ("beta-unique", 1)] {
        let db = db.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..25 {
                db.upsert_doc(
                    &IndexDoc::new("shared")
                        .with_body(body)
                        .with_vector(vector(coordinate)),
                )
                .unwrap();
            }
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }

    let alpha_text = db
        .query_hybrid(HybridQuery::default().with_text("alpha-unique").with_k(1))
        .unwrap();
    let alpha_vector = db
        .query_hybrid(HybridQuery::default().with_vector(vector(0)).with_k(1))
        .unwrap();
    let is_alpha = !alpha_text.is_empty();
    let vector_is_alpha = alpha_vector[0].score > 0.99;
    assert_eq!(is_alpha, vector_is_alpha);
}

/// El camino que siguen hive y hive-sdk al arrancar: `HiveDB::open` sin
/// opciones sobre una base cuyo `meta.json` escribió 0.3.x.
#[test]
fn hive_db_opens_a_database_created_by_0_3() {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    copy(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hivedb-index/tests/fixtures/v0.3.1"),
        dir.path(),
    );

    let db = HiveDB::open(dir.path()).unwrap();
    db.upsert_doc(&IndexDoc::new("tool:web_search").with_body("buscar en la web"))
        .unwrap();
    let hits = db
        .query_hybrid(HybridQuery::default().with_text("buscar").with_k(1))
        .unwrap();
    assert_eq!(hits[0].id, "tool:web_search");
}
