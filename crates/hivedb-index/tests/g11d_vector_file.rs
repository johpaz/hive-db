//! Los vectores viven en un fichero plano (`vectors.N.dat`), no dentro de los
//! documentos de redb: una sola copia en disco, sobreviven a los fallos a medias
//! y las bases con el formato anterior se migran sin perder nada.

use hivedb_index::{HybridQuery, IndexDoc, ScalarFilter, SemanticIndex, VectorConfig};
use redb::{Database, TableDefinition};
use std::path::Path;

const DIM: usize = 16;

fn config() -> Option<VectorConfig> {
    Some(VectorConfig::new(DIM, "test:fichero"))
}

fn vector(seed: usize) -> Vec<f32> {
    let mut state = (seed as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (0..DIM)
        .map(|_| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        })
        .collect()
}

fn doc(i: usize) -> IndexDoc {
    IndexDoc::new(format!("d{i}"))
        .with_body(format!("documento numero{i}"))
        .with_vector(vector(i))
}

fn top(index: &SemanticIndex, seed: usize, k: usize) -> Vec<String> {
    index
        .query_hybrid(HybridQuery::default().with_vector(vector(seed)).with_k(k))
        .unwrap()
        .into_iter()
        .map(|hit| hit.id)
        .collect()
}

fn populate(path: &Path, count: usize) {
    let index = SemanticIndex::open(path, config()).unwrap();
    let docs: Vec<IndexDoc> = (0..count).map(doc).collect();
    index.upsert_batch(&docs).unwrap();
}

fn row_bytes() -> u64 {
    (DIM * 4) as u64
}

const HEADER: u64 = 16;

#[test]
fn los_vectores_estan_una_sola_vez_en_disco() {
    // El tamaño de redb no depende del tamaño de los vectores: con las mismas
    // 2 000 entradas, vectores de 16 y de 384 dimensiones dan un redb igual.
    let sizes: Vec<(u64, u64)> = [16usize, 384]
        .into_iter()
        .map(|dim| {
            let dir = tempfile::tempdir().unwrap();
            let index =
                SemanticIndex::open(dir.path(), Some(VectorConfig::new(dim, "t:dim"))).unwrap();
            let docs: Vec<IndexDoc> = (0..2_000)
                .map(|i| {
                    IndexDoc::new(format!("d{i}"))
                        .with_body(format!("documento numero{i}"))
                        .with_vector((0..dim).map(|j| ((i * 7 + j) % 13) as f32 + 1.0).collect())
                })
                .collect();
            index.upsert_batch(&docs).unwrap();
            drop(index);
            let redb = std::fs::metadata(dir.path().join("semantic.redb"))
                .unwrap()
                .len();
            let vectors = std::fs::metadata(dir.path().join("vectors.0.dat"))
                .unwrap()
                .len();
            assert_eq!(vectors, HEADER + 2_000 * (dim as u64 * 4));
            (redb, vectors)
        })
        .collect();
    assert_eq!(sizes[0].0, sizes[1].0, "redb depende del tamaño del vector");
    assert!(sizes[1].1 > 40 * sizes[0].1 / 10);
}

#[test]
fn los_datos_sobreviven_a_cerrar_y_abrir() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 300);
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((300, 0)));
    assert_eq!(top(&index, 42, 1), vec!["d42"]);
    // Consulta con filtro: usa el camino exacto, que lee del fichero.
    index
        .upsert(
            &IndexDoc::new("filtrado")
                .with_vector(vector(7))
                .with_filters(vec![ScalarFilter::eq("kind", "x")]),
        )
        .unwrap();
    let hits = index
        .query_hybrid(
            HybridQuery::default()
                .with_vector(vector(7))
                .with_filters(vec![ScalarFilter::eq("kind", "x")])
                .with_k(3),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "filtrado");
    assert!(hits[0].vector_score.unwrap() > 0.999);
}

#[test]
fn filas_huerfanas_de_una_tanda_sin_confirmar_se_descartan() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 100);
    // Simula un fallo entre escribir el vector y confirmar la transacción:
    // quedan filas al final que ningún documento referencia.
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("vectors.0.dat"))
            .unwrap();
        file.write_all(&vec![0x3F; (5 * row_bytes()) as usize])
            .unwrap();
    }

    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("vectors.0.dat"))
            .unwrap()
            .len(),
        HEADER + 100 * row_bytes()
    );
    index.upsert(&doc(500)).unwrap();
    assert_eq!(top(&index, 500, 1), vec!["d500"]);
    assert_eq!(top(&index, 3, 1), vec!["d3"]);
    drop(index);
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((101, 0)));
    assert_eq!(top(&index, 500, 1), vec!["d500"]);
}

#[test]
fn un_fichero_con_menos_filas_de_las_confirmadas_se_rechaza() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 100);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("vectors.0.dat"))
        .unwrap();
    file.set_len(HEADER + 40 * row_bytes()).unwrap();
    drop(file);

    let error = match SemanticIndex::open(dir.path(), config()) {
        Ok(_) => panic!("no debe abrir con vectores perdidos"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("vector file"), "{error}");
}

#[test]
fn actualizar_un_documento_deja_una_ranura_muerta_y_no_se_ve() {
    let dir = tempfile::tempdir().unwrap();
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    index.upsert(&doc(1)).unwrap();
    index.upsert(&doc(2)).unwrap();
    // d1 pasa a tener el vector de d9.
    index
        .upsert(&IndexDoc::new("d1").with_vector(vector(9)))
        .unwrap();
    assert_eq!(index.vector_stats(), Some((2, 1)));
    assert_eq!(top(&index, 9, 1), vec!["d1"]);
    // Y luego se queda sin vector.
    index
        .upsert(&IndexDoc::new("d1").with_body("solo texto"))
        .unwrap();
    assert_eq!(index.vector_stats(), Some((1, 2)));
    assert_eq!(top(&index, 2, 5), vec!["d2"]);
}

#[test]
fn una_tanda_con_el_mismo_id_conserva_la_ultima_version() {
    let dir = tempfile::tempdir().unwrap();
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    index
        .upsert_batch(&[
            IndexDoc::new("a").with_vector(vector(1)),
            IndexDoc::new("b").with_vector(vector(2)),
            IndexDoc::new("a").with_vector(vector(3)),
            IndexDoc::new("b"),
        ])
        .unwrap();
    assert_eq!(top(&index, 3, 1), vec!["a"]);
    assert_eq!(top(&index, 1, 5), vec!["a"]);
    assert_eq!(top(&index, 2, 5), vec!["a"]);
    drop(index);
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(top(&index, 3, 5), vec!["a"]);
}

/// Fichero de vectores que queda tras reabrir. El motor borra el fichero alterno
/// sobrante al abrir (en Windows no se puede borrar mientras sigue mapeado, así
/// que se limpia en la siguiente apertura), por eso se mira después de reabrir.
fn active_file_after_reopen(dir: &Path) -> std::path::PathBuf {
    drop(SemanticIndex::open(dir, config()).unwrap());
    let present: Vec<_> = ["vectors.0.dat", "vectors.1.dat"]
        .into_iter()
        .map(|name| dir.join(name))
        .filter(|path| path.exists())
        .collect();
    assert_eq!(
        present.len(),
        1,
        "debe quedar un único fichero: {present:?}"
    );
    present[0].clone()
}

#[test]
fn compactar_reescribe_el_fichero_sin_ranuras_muertas() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 3_000);
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    // Con 1 024 muertos (más de la cuarta parte) se compacta sola; los 76
    // borrados siguientes quedan como ranuras muertas.
    for i in 0..1_100 {
        index.delete(&format!("d{i}")).unwrap();
    }
    assert_eq!(index.vector_stats(), Some((1_900, 76)));
    assert_eq!(top(&index, 2_000, 1), vec!["d2000"]);
    drop(index);
    assert_eq!(
        std::fs::metadata(active_file_after_reopen(dir.path()))
            .unwrap()
            .len(),
        HEADER + 1_976 * row_bytes()
    );

    // Compactación explícita.
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    index.compact().unwrap();
    assert_eq!(index.vector_stats(), Some((1_900, 0)));
    drop(index);
    assert_eq!(
        std::fs::metadata(active_file_after_reopen(dir.path()))
            .unwrap()
            .len(),
        HEADER + 1_900 * row_bytes()
    );

    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((1_900, 0)));
    assert_eq!(top(&index, 1_700, 1), vec!["d1700"]);
    assert!(!top(&index, 10, 5).contains(&"d10".to_string()));
}

#[test]
fn clear_vacia_los_vectores_y_se_puede_seguir_usando() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 200);
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    index.clear().unwrap();
    assert_eq!(index.vector_stats(), Some((0, 0)));
    assert!(top(&index, 3, 5).is_empty());
    index.upsert(&doc(77)).unwrap();
    assert_eq!(top(&index, 77, 1), vec!["d77"]);
    drop(index);

    active_file_after_reopen(dir.path());
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((1, 0)));
    assert_eq!(top(&index, 77, 1), vec!["d77"]);
}

// ---- Migración desde el formato anterior (vector dentro del registro) ----

const LEGACY_DOCS: TableDefinition<&str, Vec<u8>> = TableDefinition::new("semantic_docs");
const LEGACY_META: TableDefinition<&str, u64> = TableDefinition::new("semantic_meta");

/// Crea una base como la escribía la versión anterior: documentos con su
/// vector serializado dentro del registro de redb y sin fichero de vectores.
fn legacy_database(path: &Path, count: usize, with_vectors: bool) {
    std::fs::create_dir_all(path).unwrap();
    let meta = serde_json::json!({
        "schema_version": 2,
        "metric": "cosine",
        "vector": if with_vectors { serde_json::json!({"dimension": DIM, "space_id": "test:fichero"}) } else { serde_json::Value::Null },
    });
    std::fs::write(
        path.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();
    let db = Database::create(path.join("semantic.redb")).unwrap();
    let txn = db.begin_write().unwrap();
    {
        let mut docs = txn.open_table(LEGACY_DOCS).unwrap();
        for i in 0..count {
            let mut d = IndexDoc::new(format!("d{i}")).with_body(format!("documento numero{i}"));
            if with_vectors {
                d = d.with_vector(vector(i));
            }
            docs.insert(d.id.as_str(), bincode::serialize(&d).unwrap())
                .unwrap();
        }
        let mut meta = txn.open_table(LEGACY_META).unwrap();
        meta.insert("generation", 7u64).unwrap();
    }
    txn.commit().unwrap();
}

#[test]
fn migra_una_base_del_formato_anterior_sin_perder_documentos() {
    let dir = tempfile::tempdir().unwrap();
    legacy_database(dir.path(), 2_500, true);
    let before = std::fs::metadata(dir.path().join("semantic.redb"))
        .unwrap()
        .len();
    let meta_before = std::fs::read(dir.path().join("meta.json")).unwrap();

    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((2_500, 0)));
    assert_eq!(top(&index, 1_234, 1), vec!["d1234"]);
    let text = index
        .query_hybrid(HybridQuery::default().with_text("numero77").with_k(1))
        .unwrap();
    assert_eq!(text[0].id, "d77");
    drop(index);

    // meta.json intacto: las bases de versiones anteriores no se reescriben.
    assert_eq!(
        std::fs::read(dir.path().join("meta.json")).unwrap(),
        meta_before
    );
    // Una sola copia de los vectores y redb mucho más pequeño.
    let vectors = std::fs::metadata(dir.path().join("vectors.0.dat"))
        .unwrap()
        .len();
    assert_eq!(vectors, HEADER + 2_500 * row_bytes());
    let after = std::fs::metadata(dir.path().join("semantic.redb"))
        .unwrap()
        .len();
    assert!(
        after <= before,
        "redb creció al migrar: {before} -> {after}"
    );

    // Reabrir es idempotente.
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((2_500, 0)));
    assert_eq!(top(&index, 5, 1), vec!["d5"]);
}

#[test]
fn una_version_anterior_no_puede_abrir_una_base_migrada() {
    let dir = tempfile::tempdir().unwrap();
    legacy_database(dir.path(), 10, true);
    drop(SemanticIndex::open(dir.path(), config()).unwrap());

    // La versión anterior abre `semantic_docs` con claves `&str`: debe fallar
    // con un error de tipo, no ver un índice vacío en el que escribir.
    let db = Database::open(dir.path().join("semantic.redb")).unwrap();
    let txn = db.begin_write().unwrap();
    assert!(txn.open_table(LEGACY_DOCS).is_err());
}

#[test]
fn una_base_nueva_tambien_queda_protegida_frente_a_versiones_anteriores() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 5);
    let db = Database::open(dir.path().join("semantic.redb")).unwrap();
    let txn = db.begin_write().unwrap();
    assert!(txn.open_table(LEGACY_DOCS).is_err());
}

#[test]
fn migrar_con_filas_huerfanas_previas_en_el_fichero_de_vectores() {
    let dir = tempfile::tempdir().unwrap();
    legacy_database(dir.path(), 50, true);
    // Una migración interrumpida pudo dejar filas escritas sin confirmar.
    {
        use std::io::Write;
        let mut header = Vec::new();
        header.extend_from_slice(b"HIVEVEC1");
        header.extend_from_slice(&(DIM as u32).to_le_bytes());
        header.extend_from_slice(&[0; 4]);
        header.extend(vec![0x3F; (7 * row_bytes()) as usize]);
        std::fs::File::create(dir.path().join("vectors.0.dat"))
            .unwrap()
            .write_all(&header)
            .unwrap();
    }
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((50, 0)));
    assert_eq!(top(&index, 20, 1), vec!["d20"]);
    drop(index);
    assert_eq!(
        std::fs::metadata(dir.path().join("vectors.0.dat"))
            .unwrap()
            .len(),
        HEADER + 50 * row_bytes()
    );
}

#[test]
fn migra_tambien_una_base_solo_de_texto() {
    let dir = tempfile::tempdir().unwrap();
    legacy_database(dir.path(), 30, false);
    let index = SemanticIndex::open(dir.path(), None).unwrap();
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("numero12").with_k(1))
        .unwrap();
    assert_eq!(hits[0].id, "d12");
    drop(index);
    let db = Database::open(dir.path().join("semantic.redb")).unwrap();
    assert!(db.begin_write().unwrap().open_table(LEGACY_DOCS).is_err());
}

#[test]
fn migrar_una_base_solo_de_texto_no_agranda_el_fichero() {
    // Documentos de texto grandes y ninguna vector: copiarlos a la tabla nueva y
    // borrar la antigua no debe dejar el fichero de redb más grande que antes.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(
        dir.path().join("meta.json"),
        br#"{"schema_version":2,"metric":"cosine","vector":null}"#,
    )
    .unwrap();
    {
        let db = Database::create(dir.path().join("semantic.redb")).unwrap();
        let txn = db.begin_write().unwrap();
        {
            let mut docs = txn.open_table(LEGACY_DOCS).unwrap();
            for i in 0..3_000 {
                let d = IndexDoc::new(format!("d{i}"))
                    .with_body(format!("palabra{i} {}", "texto de relleno ".repeat(300)));
                docs.insert(d.id.as_str(), bincode::serialize(&d).unwrap())
                    .unwrap();
            }
            let mut meta = txn.open_table(LEGACY_META).unwrap();
            meta.insert("generation", 1u64).unwrap();
        }
        txn.commit().unwrap();
    }
    let before = std::fs::metadata(dir.path().join("semantic.redb"))
        .unwrap()
        .len();

    let index = SemanticIndex::open(dir.path(), None).unwrap();
    let hits = index
        .query_hybrid(HybridQuery::default().with_text("palabra42").with_k(1))
        .unwrap();
    assert_eq!(hits[0].id, "d42");
    drop(index);

    let after = std::fs::metadata(dir.path().join("semantic.redb"))
        .unwrap()
        .len();
    assert!(
        after * 10 <= before * 11,
        "semantic.redb creció de {before} a {after} bytes al migrar"
    );
}
