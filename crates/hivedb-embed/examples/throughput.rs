//! Mide documentos/s y latencia de consulta del embedder local en esta máquina.
//! `cargo run --release -p hivedb-embed --example throughput -- [documentos]`

use hivedb_embed::LocalEmbedder;
use hivedb_index::{EmbedKind, Embedder};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let n: usize = std::env::args().nth(1).map_or(256, |a| a.parse().unwrap());
    let t = Instant::now();
    let embedder = LocalEmbedder::multilingual_e5_small()?;
    println!("carga_modelo_ms={}", t.elapsed().as_millis());

    let texts: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "Documento {i}: política de reembolsos, envíos y soporte al cliente para pedidos \
                 realizados en la tienda en línea, con detalles sobre plazos y condiciones."
            )
        })
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    embedder.embed(&refs[..4.min(n)], EmbedKind::Document)?; // calentamiento

    let t = Instant::now();
    embedder.embed(&refs, EmbedKind::Document)?;
    let secs = t.elapsed().as_secs_f64();
    println!(
        "docs={n} docs_por_s={:.1} (~30 tokens/doc)",
        n as f64 / secs
    );

    let mut lat = Vec::new();
    for i in 0..30 {
        let t = Instant::now();
        embedder.embed(&[texts[i % n].as_str()], EmbedKind::Query)?;
        lat.push(t.elapsed().as_micros());
    }
    lat.sort();
    println!(
        "consulta_p50_ms={:.1} p99_ms={:.1}",
        lat[15] as f64 / 1e3,
        lat[29] as f64 / 1e3
    );
    Ok(())
}
