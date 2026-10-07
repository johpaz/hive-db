//! Imprime en JSON el embedding de cada argumento: `cargo run --release -p
//! hivedb-embed --example embed -- "query:consulta" "passage:documento"`.
//! Cada argumento lleva el prefijo `query:` o `passage:` (sin espacio).

use hivedb_embed::LocalEmbedder;
use hivedb_index::{EmbedKind, Embedder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let embedder = LocalEmbedder::multilingual_e5_small()?;
    let mut out = Vec::new();
    for arg in std::env::args().skip(1) {
        let (kind, text) = match arg.split_once(':') {
            Some(("query", text)) => (EmbedKind::Query, text.to_string()),
            Some(("passage", text)) => (EmbedKind::Document, text.to_string()),
            _ => (EmbedKind::Document, arg.clone()),
        };
        let vector = embedder.embed(&[text.as_str()], kind)?.remove(0);
        out.push(serde_json::json!({ "arg": arg, "vector": vector }));
    }
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}
