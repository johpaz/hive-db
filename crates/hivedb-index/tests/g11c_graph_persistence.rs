//! El grafo ANN se vuelca al cerrar y se restaura al abrir, pero solo si
//! coincide exactamente con los documentos; ante cualquier duda se reconstruye.

use hivedb_index::{HybridQuery, IndexDoc, SemanticIndex, VectorConfig};
use std::path::Path;

const DIM: usize = 16;

fn config() -> Option<VectorConfig> {
    Some(VectorConfig::new(DIM, "test:persist"))
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
        .with_body(format!("documento {i}"))
        .with_vector(vector(i))
}

fn populate(path: &Path, count: usize) {
    let index = SemanticIndex::open(path, config()).unwrap();
    let docs: Vec<IndexDoc> = (0..count).map(doc).collect();
    index.upsert_batch(&docs).unwrap();
    assert!(!index.vector_graph_persisted());
}

fn top(index: &SemanticIndex, seed: usize) -> Vec<String> {
    index
        .query_hybrid(HybridQuery::default().with_vector(vector(seed)).with_k(5))
        .unwrap()
        .into_iter()
        .map(|hit| hit.id)
        .collect()
}

fn graph_files(path: &Path) -> Vec<std::path::PathBuf> {
    ["vectors.meta", "vectors.hnsw.graph"]
        .iter()
        .map(|name| path.join("hnsw").join(name))
        .collect()
}

#[test]
fn el_grafo_se_restaura_al_reabrir() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 300);
    for file in graph_files(dir.path()) {
        assert!(file.exists(), "falta {file:?} tras cerrar");
    }

    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert!(index.vector_graph_persisted(), "no se restauró el grafo");
    assert_eq!(index.vector_stats(), Some((300, 0)));
    assert_eq!(top(&index, 5)[0], "d5");
    assert_eq!(top(&index, 123)[0], "d123");
}

#[test]
fn se_puede_insertar_y_borrar_tras_restaurar() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 200);
    {
        let index = SemanticIndex::open(dir.path(), config()).unwrap();
        assert!(index.vector_graph_persisted());
        index.upsert(&doc(900)).unwrap();
        index.delete("d7").unwrap();
        assert!(!index.vector_graph_persisted());
        assert_eq!(top(&index, 900)[0], "d900");
        assert!(!top(&index, 7).contains(&"d7".to_string()));
    }

    // Segundo ciclo: el volcado incluye la inserción y el tombstone.
    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert!(index.vector_graph_persisted());
    assert_eq!(index.vector_stats(), Some((200, 1)));
    assert_eq!(top(&index, 900)[0], "d900");
    assert!(!top(&index, 7).contains(&"d7".to_string()));
}

#[test]
fn un_volcado_de_otra_generacion_se_descarta() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 200);

    // Guarda el volcado válido de la generación actual.
    let saved: Vec<(std::path::PathBuf, Vec<u8>)> = graph_files(dir.path())
        .into_iter()
        .map(|file| {
            let bytes = std::fs::read(&file).unwrap();
            (file, bytes)
        })
        .collect();

    {
        let index = SemanticIndex::open(dir.path(), config()).unwrap();
        index.upsert(&doc(500)).unwrap();
    }
    // Simula un cierre sucio: reemplaza el volcado nuevo por el viejo.
    for (file, bytes) in &saved {
        std::fs::write(file, bytes).unwrap();
    }

    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert!(
        !index.vector_graph_persisted(),
        "aceptó un volcado obsoleto"
    );
    assert_eq!(index.vector_stats(), Some((201, 0)));
    assert_eq!(top(&index, 500)[0], "d500");
}

#[test]
fn un_volcado_corrupto_se_reconstruye_sin_fallar() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 200);

    let graph = dir.path().join("hnsw").join("vectors.hnsw.graph");
    let mut bytes = std::fs::read(&graph).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&graph, bytes).unwrap();

    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert!(!index.vector_graph_persisted());
    assert_eq!(index.vector_stats(), Some((200, 0)));
    assert_eq!(top(&index, 42)[0], "d42");
}

#[test]
fn un_volcado_con_contenido_alterado_no_provoca_panico() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 200);

    // Mismo tamaño, contenido basura: pasa la comprobación de longitudes.
    let graph = dir.path().join("hnsw").join("vectors.hnsw.graph");
    let len = std::fs::metadata(&graph).unwrap().len() as usize;
    std::fs::write(&graph, vec![0xAB; len]).unwrap();

    let index = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(index.vector_stats(), Some((200, 0)));
    assert_eq!(top(&index, 42)[0], "d42");
}

#[test]
fn otro_espacio_vectorial_no_reutiliza_el_volcado() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 100);
    let error = SemanticIndex::open(dir.path(), Some(VectorConfig::new(DIM, "otro:espacio")));
    assert!(
        error.is_err(),
        "meta.json debe rechazar el espacio distinto"
    );
}

#[test]
fn los_indices_en_ram_no_escriben_volcados() {
    let index = SemanticIndex::open_in_ram(config()).unwrap();
    index.upsert(&doc(1)).unwrap();
    assert!(index.persist_vector_graph().is_ok());
    assert!(!index.vector_graph_persisted());
}

#[test]
fn sin_vectores_no_hay_volcado() {
    let dir = tempfile::tempdir().unwrap();
    {
        let index = SemanticIndex::open(dir.path(), None).unwrap();
        index
            .upsert(&IndexDoc::new("a").with_body("solo texto"))
            .unwrap();
    }
    assert!(!dir.path().join("hnsw").exists());
}

fn text_ids(index: &SemanticIndex, text: &str) -> Vec<String> {
    let mut ids: Vec<String> = index
        .query_hybrid(HybridQuery::default().with_text(text).with_k(50))
        .unwrap()
        .into_iter()
        .map(|hit| hit.id)
        .collect();
    ids.sort();
    ids
}

#[test]
fn indice_de_texto_se_restaura_sin_reconstruir() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 60);
    // El marcador solo existe tras un cierre limpio.
    assert!(dir.path().join("fts.generation").exists());
    // 8 bytes de generación + 4 de versión del análisis de texto.
    assert_eq!(
        std::fs::read(dir.path().join("fts.generation"))
            .unwrap()
            .len(),
        12
    );

    let reopened = SemanticIndex::open(dir.path(), config()).unwrap();
    // Al abrir se invalida: si el proceso muriera ahora, no quedaría marcador.
    assert!(!dir.path().join("fts.generation").exists());
    assert!(reopened.vector_graph_persisted());
    assert_eq!(text_ids(&reopened, "7"), vec!["d7".to_string()]);
    assert_eq!(top(&reopened, 3)[0], "d3");
}

#[test]
fn cierre_sucio_reconstruye_el_texto_y_ve_las_escrituras_nuevas() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 20);
    let marcador_viejo = std::fs::read(dir.path().join("fts.generation")).unwrap();

    {
        let index = SemanticIndex::open(dir.path(), config()).unwrap();
        index
            .upsert(
                &IndexDoc::new("nuevo")
                    .with_body("palabraunica")
                    .with_vector(vector(99)),
            )
            .unwrap();
    }
    // Simula un crash tras escribir: el marcador es el de antes de la sesión.
    std::fs::write(dir.path().join("fts.generation"), marcador_viejo).unwrap();

    let reopened = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(
        text_ids(&reopened, "palabraunica"),
        vec!["nuevo".to_string()]
    );
    assert_eq!(top(&reopened, 99)[0], "nuevo");
}

#[test]
fn marcador_de_otra_generacion_obliga_a_reconstruir() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 20);
    std::fs::write(dir.path().join("fts.generation"), 9_999u64.to_le_bytes()).unwrap();

    let reopened = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(text_ids(&reopened, "5"), vec!["d5".to_string()]);
    assert_eq!(text_ids(&reopened, "19"), vec!["d19".to_string()]);
}

#[test]
fn indice_de_texto_borrado_con_marcador_valido_se_reconstruye() {
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 20);
    std::fs::remove_dir_all(dir.path().join("fts")).unwrap();

    let reopened = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(text_ids(&reopened, "5"), vec!["d5".to_string()]);
}

#[test]
fn un_marcador_de_otra_version_del_analisis_se_ignora() {
    // Un marcador de versiones sin el campo de versión del análisis (8 bytes) no vale: el
    // texto se tokenizaba distinto y hay que reconstruirlo.
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path(), 20);
    let marker = std::fs::read(dir.path().join("fts.generation")).unwrap();
    std::fs::write(dir.path().join("fts.generation"), &marker[..8]).unwrap();
    // Y se simula un índice de texto con otro análisis: vacío.
    std::fs::remove_dir_all(dir.path().join("fts")).unwrap();
    std::fs::create_dir_all(dir.path().join("fts")).unwrap();

    let reopened = SemanticIndex::open(dir.path(), config()).unwrap();
    assert_eq!(text_ids(&reopened, "5"), vec!["d5".to_string()]);
}
