use hivedb_index::{FieldBoosts, HybridQuery, IndexDoc, ScalarFilter, SemanticIndex, VectorConfig};

fn open_index(path: &std::path::Path) -> SemanticIndex {
    SemanticIndex::open(path, Some(VectorConfig::new(384, "test:384"))).unwrap()
}

fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0.0; 384];
    match text {
        "pago fallido" | "pago" => {
            v[0] = 1.0;
            v[1] = 1.0;
        }
        "transacción rechazada" | "transacción" => {
            v[0] = 1.0;
            v[2] = 1.0;
        }
        "logística" => {
            v[10] = 1.0;
        }
        "x" => {
            v[5] = 1.0;
        }
        _ => {
            v[100] = 1.0;
        }
    }
    v
}

fn doc(id: &str, body: &str) -> IndexDoc {
    IndexDoc::new(id).with_body(body)
}

#[test]
fn hybrid_query_fuses_text_and_vector() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(&doc("doc1", "error de compilación en pagos").with_vector(embed("pago fallido")))
        .unwrap();
    index
        .upsert(&doc("doc2", "documentación de la API de envíos").with_vector(embed("logística")))
        .unwrap();

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("error pagos")
                .with_vector(embed("transacción rechazada"))
                .with_k(5),
        )
        .unwrap();

    assert!(!hits.is_empty());
    assert_eq!(hits[0].id, "doc1");
    // Hybrid hits expose the raw per-source scores.
    assert!(hits[0].text_score.is_some());
    assert!(hits[0].vector_score.is_some());
}

#[test]
fn scalar_filters_pushed_into_index() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(
            &doc("d1", "deploy fallido")
                .with_vector(embed("x"))
                .with_filters(vec![ScalarFilter::eq("agent", "Backend")]),
        )
        .unwrap();
    index
        .upsert(
            &doc("d2", "deploy fallido")
                .with_vector(embed("x"))
                .with_filters(vec![ScalarFilter::eq("agent", "Frontend")]),
        )
        .unwrap();

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("deploy")
                .with_filters(vec![ScalarFilter::eq("agent", "Backend")])
                .with_k(10),
        )
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d1");
}

#[test]
fn vector_index_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();

    {
        let index = open_index(dir.path());
        index
            .upsert(&doc("doc1", "torre eiffel en parís").with_vector(embed("pago")))
            .unwrap();
        index
            .upsert(&doc("doc2", "sushi en tokio").with_vector(embed("logística")))
            .unwrap();
    }

    // Reopen from disk: the ANN index must be rebuilt from persisted vectors.
    let index = open_index(dir.path());
    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("pago")).with_k(1))
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "doc1");
}

#[test]
fn scalar_filters_work_on_arbitrary_fields() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(
            &doc("d1", "guía de viaje")
                .with_vector(embed("x"))
                .with_filters(vec![ScalarFilter::eq("city", "tokyo")]),
        )
        .unwrap();
    index
        .upsert(
            &doc("d2", "guía de viaje")
                .with_vector(embed("x"))
                .with_filters(vec![ScalarFilter::eq("city", "paris")]),
        )
        .unwrap();

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("guía")
                .with_filters(vec![ScalarFilter::eq("city", "paris")])
                .with_k(10),
        )
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d2");
}

#[test]
fn hit_has_positive_score() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index.upsert(&doc("doc1", "hello world")).unwrap();

    let hits = index
        .query_hybrid(HybridQuery::default().with_text("hello").with_k(1))
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert!(hits[0].score > 0.0);
}

// --- Spanish text analysis -------------------------------------------------

#[test]
fn spanish_stemming_matches_morphological_variants() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(&doc("d1", "procesa el pago de la factura"))
        .unwrap();

    // "pagos" must match a document containing "pago".
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("pagos").with_k(5))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d1");
}

#[test]
fn accent_folding_matches_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(&doc("d1", "transacción rechazada por el banco"))
        .unwrap();
    index
        .upsert(&doc("d2", "generacion de reportes mensuales"))
        .unwrap();

    // Accent-less query matches accented document...
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("transaccion").with_k(5))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d1");

    // ...and accented query matches accent-less document.
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("generación").with_k(5))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d2");
}

#[test]
fn malformed_query_never_fails() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index.upsert(&doc("d1", "envía el correo")).unwrap();

    // Raw user input with FTS operators, unbalanced quotes and punctuation.
    for query in [
        "\"correo sin cerrar",
        "correo AND OR NOT (",
        "¿puedes enviar el correo?",
        "field:value*^~",
        "***",
    ] {
        let result = index.query_hybrid(HybridQuery::default().with_text(query).with_k(5));
        assert!(result.is_ok(), "query {query:?} must not fail");
    }

    // The natural-language variant still finds the document.
    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("¿puedes enviar el correo?")
                .with_k(5),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d1");
}

// --- Field boosts ----------------------------------------------------------

#[test]
fn name_field_boost_ranks_title_matches_first() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(
            &IndexDoc::new("by-name")
                .with_name("enviar correo")
                .with_body("herramienta de mensajería"),
        )
        .unwrap();
    index
        .upsert(
            &IndexDoc::new("by-body")
                .with_name("notificaciones")
                .with_body("permite enviar un correo al usuario"),
        )
        .unwrap();

    let hits = index
        .query_hybrid(HybridQuery::default().with_text("correo").with_k(5))
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, "by-name", "name match must outrank body match");

    // Inverting the boosts inverts the ranking.
    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("correo")
                .with_k(5)
                .with_boosts(FieldBoosts {
                    name: 1.0,
                    body: 10.0,
                    tags: 1.0,
                }),
        )
        .unwrap();
    assert_eq!(hits[0].id, "by-body");
}

// --- Upsert / delete / clear -----------------------------------------------

#[test]
fn upsert_replaces_document() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index.upsert(&doc("d1", "contenido de pagos")).unwrap();
    index.upsert(&doc("d1", "contenido de envíos")).unwrap();

    let hits = index
        .query_hybrid(HybridQuery::default().with_text("pagos").with_k(5))
        .unwrap();
    assert!(hits.is_empty(), "old content must be gone after upsert");

    let hits = index
        .query_hybrid(HybridQuery::default().with_text("envíos").with_k(5))
        .unwrap();
    assert_eq!(hits.len(), 1, "no duplicates after upsert");
}

#[test]
fn delete_removes_text_and_vector() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(&doc("d1", "pago fallido").with_vector(embed("pago")))
        .unwrap();
    index
        .upsert(&doc("d2", "otro tema").with_vector(embed("logística")))
        .unwrap();

    index.delete("d1").unwrap();

    let hits = index
        .query_hybrid(HybridQuery::default().with_text("pago").with_k(5))
        .unwrap();
    assert!(hits.is_empty());

    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("pago")).with_k(5))
        .unwrap();
    assert!(
        hits.iter().all(|h| h.id != "d1"),
        "vector must be tombstoned"
    );

    // Deletion survives a reopen (tombstone is persisted).
    drop(index);
    let index = open_index(dir.path());
    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("pago")).with_k(5))
        .unwrap();
    assert!(hits.iter().all(|h| h.id != "d1"));
}

#[test]
fn delete_by_filter_removes_matching_docs() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    for i in 0..3 {
        index
            .upsert(
                &doc(&format!("srv-a-{i}"), "herramienta del servidor A")
                    .with_vector(embed("x"))
                    .with_filters(vec![ScalarFilter::eq("server_id", "a")]),
            )
            .unwrap();
    }
    index
        .upsert(
            &doc("srv-b-0", "herramienta del servidor B")
                .with_filters(vec![ScalarFilter::eq("server_id", "b")]),
        )
        .unwrap();

    index
        .delete_by_filter(&ScalarFilter::eq("server_id", "a"))
        .unwrap();

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("herramienta servidor")
                .with_k(10),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "srv-b-0");

    // Vectors of the deleted docs are gone too.
    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("x")).with_k(10))
        .unwrap();
    assert!(hits.iter().all(|h| !h.id.starts_with("srv-a-")));
}

#[test]
fn clear_empties_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(&doc("d1", "algo de texto").with_vector(embed("x")))
        .unwrap();
    index.clear().unwrap();

    let hits = index
        .query_hybrid(HybridQuery::default().with_text("texto").with_k(5))
        .unwrap();
    assert!(hits.is_empty());

    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("x")).with_k(5))
        .unwrap();
    assert!(hits.is_empty());

    // Clear survives reopen (persistence file was truncated).
    drop(index);
    let index = open_index(dir.path());
    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("x")).with_k(5))
        .unwrap();
    assert!(hits.is_empty());
}

#[test]
fn batch_upsert_indexes_all_docs() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    let docs: Vec<IndexDoc> = (0..50)
        .map(|i| {
            doc(
                &format!("d{i}"),
                &format!("documento número {i} sobre pagos"),
            )
        })
        .collect();
    index.upsert_batch(&docs).unwrap();

    let hits = index
        .query_hybrid(HybridQuery::default().with_text("pagos").with_k(50))
        .unwrap();
    assert_eq!(hits.len(), 50);
}

// --- Optional vector / mixed corpora ---------------------------------------

#[test]
fn text_only_docs_coexist_with_vector_queries() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(&doc("text-only", "solo texto sin vector"))
        .unwrap();
    index
        .upsert(&doc("with-vec", "documento con vector").with_vector(embed("pago")))
        .unwrap();

    // Vector query only sees the vectorized doc.
    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("pago")).with_k(10))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "with-vec");

    // Text query sees both.
    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_text("texto vector documento")
                .with_k(10),
        )
        .unwrap();
    assert_eq!(hits.len(), 2);
}

#[test]
fn vector_only_query_respects_filters() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(
            &doc("d1", "pago uno")
                .with_vector(embed("pago"))
                .with_filters(vec![ScalarFilter::eq("type", "tool")]),
        )
        .unwrap();
    index
        .upsert(
            &doc("d2", "pago dos")
                .with_vector(embed("pago fallido"))
                .with_filters(vec![ScalarFilter::eq("type", "skill")]),
        )
        .unwrap();

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_vector(embed("pago"))
                .with_filters(vec![ScalarFilter::eq("type", "skill")])
                .with_k(10),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "d2");
}

// --- Score semantics ---------------------------------------------------------

#[test]
fn single_source_scores_are_raw() {
    let dir = tempfile::tempdir().unwrap();
    let index = open_index(dir.path());

    index
        .upsert(&doc("d1", "pago fallido").with_vector(embed("pago")))
        .unwrap();

    // Text-only: raw BM25, well above the ~0.016 ceiling of RRF scores.
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("pago fallido").with_k(1))
        .unwrap();
    assert!(
        hits[0].score > 0.1,
        "expected raw BM25, got {}",
        hits[0].score
    );
    assert_eq!(hits[0].text_score, Some(hits[0].score));

    // Vector-only: cosine similarity of identical vectors ≈ 1.0.
    let hits = index
        .query_hybrid(HybridQuery::default().with_vector(embed("pago")).with_k(1))
        .unwrap();
    assert!(
        hits[0].score > 0.99,
        "expected cosine ≈ 1, got {}",
        hits[0].score
    );
    assert_eq!(hits[0].vector_score, Some(hits[0].score));
}

#[test]
fn in_ram_index_supports_full_lifecycle() {
    let index = SemanticIndex::open_in_ram(Some(VectorConfig::new(384, "test:384"))).unwrap();

    index
        .upsert(&doc("d1", "transacción rechazada").with_vector(embed("transacción")))
        .unwrap();
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("transacciones").with_k(5))
        .unwrap();
    assert_eq!(hits.len(), 1);

    index.delete("d1").unwrap();
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("transacción").with_k(5))
        .unwrap();
    assert!(hits.is_empty());
}

#[test]
fn ef_search_mejora_recall_vectorial() {
    // Corpus pseudoaleatorio determinista: con `ef` bajo el recall@10 cae
    // frente al exacto; con `ef` alto debe superar claramente al bajo.
    let dir = tempfile::tempdir().unwrap();
    let index = SemanticIndex::open(dir.path(), Some(VectorConfig::new(32, "test:32"))).unwrap();
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    let vectors: Vec<Vec<f32>> = (0..2_000)
        .map(|_| (0..32).map(|_| next()).collect())
        .collect();
    let docs: Vec<IndexDoc> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| IndexDoc::new(format!("d{i}")).with_vector(v.clone()))
        .collect();
    index.upsert_batch(&docs).unwrap();

    let queries: Vec<Vec<f32>> = (0..30).map(|_| (0..32).map(|_| next()).collect()).collect();
    let recall = |ef: usize| {
        let mut hits_ok = 0;
        for q in &queries {
            let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
            let mut exact: Vec<(usize, f32)> = vectors
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let dot: f32 = v.iter().zip(q).map(|(a, b)| a * b).sum();
                    (i, dot / (norm(v) * norm(q)))
                })
                .collect();
            exact.sort_by(|a, b| b.1.total_cmp(&a.1));
            let truth: Vec<String> = exact
                .iter()
                .take(10)
                .map(|(i, _)| format!("d{i}"))
                .collect();
            let got = index
                .query_hybrid(
                    HybridQuery::default()
                        .with_vector(q.clone())
                        .with_k(10)
                        .with_ef_search(ef),
                )
                .unwrap();
            hits_ok += got.iter().filter(|h| truth.contains(&h.id)).count();
        }
        hits_ok as f64 / (queries.len() * 10) as f64
    };
    let bajo = recall(10);
    let alto = recall(800);
    assert!(
        alto >= bajo,
        "ef alto ({alto}) no debe empeorar a ef bajo ({bajo})"
    );
    assert!(alto > 0.9, "ef=800 debe dar recall@10 > 0.9, dio {alto}");
}

#[test]
fn lote_grande_conserva_semantica_de_upsert_y_borrado() {
    // Un lote de >= 64 se inserta en paralelo; debe comportarse igual que
    // insertar uno a uno: gana la última versión de un id y un documento sin
    // vector borra el vector anterior.
    let dir = tempfile::tempdir().unwrap();
    let index = SemanticIndex::open(dir.path(), Some(VectorConfig::new(4, "test:4"))).unwrap();
    let vec_de = |i: usize| vec![1.0, (i % 7) as f32, (i % 11) as f32, 0.5];

    let mut docs: Vec<IndexDoc> = (0..200)
        .map(|i| IndexDoc::new(format!("d{i}")).with_vector(vec_de(i)))
        .collect();
    // d5 aparece de nuevo con otro vector (gana este) y d6 queda sin vector.
    docs.push(IndexDoc::new("d5").with_vector(vec![0.0, 0.0, 0.0, 9.0]));
    docs.push(IndexDoc::new("d6"));
    index.upsert_batch(&docs).unwrap();

    assert_eq!(index.vector_stats().unwrap().0, 199);

    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_vector(vec![0.0, 0.0, 0.0, 1.0])
                .with_k(1),
        )
        .unwrap();
    assert_eq!(hits[0].id, "d5");

    let todos = index
        .query_hybrid(
            HybridQuery::default()
                .with_vector(vec![1.0, 1.0, 1.0, 0.5])
                .with_k(50),
        )
        .unwrap();
    assert!(todos.iter().all(|h| h.id != "d6"));
}
