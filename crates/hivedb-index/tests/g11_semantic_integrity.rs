use hivedb_index::{HybridQuery, IndexDoc, ScalarFilter, SemanticIndex, VectorConfig};

fn config(dimension: usize) -> Option<VectorConfig> {
    Some(VectorConfig::new(dimension, format!("test:{dimension}")))
}

fn unit(dimension: usize, coordinate: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dimension];
    vector[coordinate] = 1.0;
    vector
}

#[test]
fn invalid_vectors_are_rejected_before_indexing() {
    let index = SemanticIndex::open_in_ram(config(8)).unwrap();
    for (name, vector) in [
        ("zero", vec![0.0; 8]),
        ("nan", vec![f32::NAN; 8]),
        ("infinite", vec![f32::INFINITY; 8]),
    ] {
        let error = index
            .upsert(&IndexDoc::new(name).with_vector(vector))
            .unwrap_err();
        assert!(error.to_string().starts_with("INVALID_VECTOR:"));
    }

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("zero nan infinite")
                .with_k(10),
        )
        .unwrap();
    assert!(
        hits.is_empty(),
        "validation must happen before the redb commit"
    );
}

#[test]
fn invalid_queries_and_text_only_mode_fail_explicitly() {
    let vector_index = SemanticIndex::open_in_ram(config(8)).unwrap();
    assert!(
        vector_index
            .query_hybrid(HybridQuery::default().with_vector(vec![0.0; 8]).with_k(1))
            .unwrap_err()
            .to_string()
            .starts_with("INVALID_VECTOR:")
    );
    assert!(
        vector_index
            .query_hybrid(HybridQuery::default().with_text("x").with_k(0))
            .unwrap_err()
            .to_string()
            .starts_with("INVALID_VECTOR:")
    );

    let text_only = SemanticIndex::open_in_ram(None).unwrap();
    text_only
        .upsert(&IndexDoc::new("text").with_body("solo texto"))
        .unwrap();
    assert!(
        text_only
            .upsert(&IndexDoc::new("vector").with_vector(unit(8, 0)))
            .unwrap_err()
            .to_string()
            .starts_with("INVALID_VECTOR:")
    );
}

#[test]
fn vector_space_is_immutable() {
    let dir = tempfile::tempdir().unwrap();
    drop(SemanticIndex::open(dir.path(), config(8)).unwrap());

    let error = match SemanticIndex::open(dir.path(), Some(VectorConfig::new(8, "another-model:8")))
    {
        Ok(_) => panic!("opening with another vector space must fail"),
        Err(error) => error,
    };
    assert!(error.to_string().starts_with("VECTOR_SPACE_MISMATCH:"));
}

#[test]
fn filtered_search_returns_exact_top_k_from_allowed_documents() {
    let index = SemanticIndex::open_in_ram(config(8)).unwrap();
    let mut docs = Vec::new();
    // These highly similar documents do not satisfy the filter and would
    // starve a fixed-size ANN post-filter.
    for position in 0..20 {
        let mut vector = unit(8, 0);
        vector[1] = position as f32 * 0.0001;
        docs.push(
            IndexDoc::new(format!("blocked-{position}"))
                .with_vector(vector)
                .with_filters(vec![ScalarFilter::eq("tenant", "blocked")]),
        );
    }
    docs.push(
        IndexDoc::new("allowed-best")
            .with_vector(vec![0.8, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            .with_filters(vec![ScalarFilter::eq("tenant", "allowed")]),
    );
    docs.push(
        IndexDoc::new("allowed-second")
            .with_vector(vec![0.6, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])
            .with_filters(vec![ScalarFilter::eq("tenant", "allowed")]),
    );
    index.upsert_batch(&docs).unwrap();

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_vector(unit(8, 0))
                .with_filters(vec![ScalarFilter::eq("tenant", "allowed")])
                .with_k(10),
        )
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, "allowed-best");
    assert_eq!(hits[1].id, "allowed-second");
}

#[test]
fn compact_removes_tombstones_without_changing_results() {
    let index = SemanticIndex::open_in_ram(config(8)).unwrap();
    index
        .upsert(&IndexDoc::new("doc").with_vector(unit(8, 0)))
        .unwrap();
    for coordinate in 1..8 {
        index
            .upsert(&IndexDoc::new("doc").with_vector(unit(8, coordinate)))
            .unwrap();
    }
    let before = index.vector_stats().unwrap();
    assert_eq!(before, (1, 7));
    let expected = index
        .query_hybrid(HybridQuery::default().with_vector(unit(8, 7)).with_k(1))
        .unwrap();

    index.compact().unwrap();
    assert_eq!(index.vector_stats(), Some((1, 0)));
    let after = index
        .query_hybrid(HybridQuery::default().with_vector(unit(8, 7)).with_k(1))
        .unwrap();
    assert_eq!(expected, after);
}

#[test]
fn tombstone_threshold_triggers_automatic_compaction() {
    let index = SemanticIndex::open_in_ram(config(8)).unwrap();
    let documents: Vec<IndexDoc> = (0..=1_024)
        .map(|version| IndexDoc::new("shared").with_vector(unit(8, version % 8)))
        .collect();
    index.upsert_batch(&documents).unwrap();
    assert_eq!(index.vector_stats(), Some((1, 0)));
}

#[test]
fn authoritative_documents_rebuild_both_indexes_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let index = SemanticIndex::open(dir.path(), config(8)).unwrap();
        index
            .upsert(
                &IndexDoc::new("doc")
                    .with_body("memoria persistente")
                    .with_vector(unit(8, 3)),
            )
            .unwrap();
    }
    let index = SemanticIndex::open(dir.path(), config(8)).unwrap();
    assert_eq!(
        index
            .query_hybrid(HybridQuery::default().with_text("persistente").with_k(1))
            .unwrap()[0]
            .id,
        "doc"
    );
    assert_eq!(
        index
            .query_hybrid(HybridQuery::default().with_vector(unit(8, 3)).with_k(1))
            .unwrap()[0]
            .id,
        "doc"
    );
}

/// Copia a un directorio temporal el fixture de una base creada con 0.3.1
/// (sólo lo que toca el índice semántico: `meta.json`, `vec/` y el Tantivy de
/// esa versión), porque abrirla la migra.
fn legacy_database() -> tempfile::TempDir {
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
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0.3.1"),
        dir.path(),
    );
    dir
}

fn meta(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("meta.json")).unwrap()).unwrap()
}

#[test]
fn opens_a_database_created_by_0_3_without_losing_the_way_back() {
    let dir = legacy_database();
    let index = SemanticIndex::open(dir.path(), None).unwrap();

    let migrated = meta(dir.path());
    assert_eq!(migrated["schema_version"], 2);
    assert!(migrated["vector"].is_null());
    // 0.3.x sólo lee esta clave: conservarla es lo que permite volver atrás.
    assert_eq!(migrated["vector_dimension"], 384);

    // 0.3.x no guardaba los documentos en ningún almacén autoritativo, así que
    // el índice arranca vacío y el consumidor reindexa.
    assert!(
        index
            .query_hybrid(HybridQuery::default().with_text("buscar web").with_k(5))
            .unwrap()
            .is_empty()
    );
    index
        .upsert(&IndexDoc::new("tool:web_search").with_body("buscar en la web"))
        .unwrap();
    drop(index);

    let reopened = SemanticIndex::open(dir.path(), None).unwrap();
    assert_eq!(
        reopened
            .query_hybrid(HybridQuery::default().with_text("buscar").with_k(1))
            .unwrap()[0]
            .id,
        "tool:web_search"
    );
    assert_eq!(meta(dir.path())["vector_dimension"], 384);
}

#[test]
fn a_database_created_by_0_3_can_adopt_an_explicit_vector_space() {
    let dir = legacy_database();
    let index = SemanticIndex::open(dir.path(), config(8)).unwrap();
    index
        .upsert(&IndexDoc::new("doc").with_vector(unit(8, 0)))
        .unwrap();
    drop(index);

    assert_eq!(meta(dir.path())["vector"]["dimension"], 8);
    // A partir de aquí rige la identidad del espacio como en cualquier base v2.
    let error = match SemanticIndex::open(dir.path(), None) {
        Ok(_) => panic!("reopening in text-only mode must fail"),
        Err(error) => error,
    };
    assert!(error.to_string().starts_with("VECTOR_SPACE_MISMATCH:"));
}

#[test]
fn a_current_meta_is_accepted_as_is_and_never_rewritten() {
    // Byte a byte lo que escriben 0.4.x y 0.5.0 —lo que tienen hoy las bases de
    // hive-sdk/hive-cloud—: abrirlas no puede tocar el archivo.
    let dir = tempfile::tempdir().unwrap();
    let written_by_0_4: &[u8] =
        b"{\n  \"schema_version\": 2,\n  \"metric\": \"cosine\",\n  \"vector\": null\n}";
    std::fs::write(dir.path().join("meta.json"), written_by_0_4).unwrap();

    drop(SemanticIndex::open(dir.path(), None).unwrap());
    assert_eq!(
        std::fs::read(dir.path().join("meta.json")).unwrap(),
        written_by_0_4
    );
}

#[test]
fn an_unreadable_meta_is_reported_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("meta.json"), br#"{"otra_cosa": 1}"#).unwrap();
    let error = match SemanticIndex::open(dir.path(), None) {
        Ok(_) => panic!("an unrecognised meta.json must fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("corrupt meta.json"), "{error}");
}
