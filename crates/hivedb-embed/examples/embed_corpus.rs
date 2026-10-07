//! Embebe un fichero de textos (uno por línea) con el embedder local y vuelca
//! los vectores como `f32` little-endian, fila a fila: el formato que leen
//! `hivedb-bench` (`HIVE_BENCH_CORPUS`) y los comparadores de Python.
//!
//! `cargo run --release -p hivedb-embed --example embed_corpus -- textos.txt salida.f32 doc|query`
//!
//! Es reanudable: si `salida.f32` ya tiene filas completas, continúa tras ellas.

use hivedb_embed::LocalEmbedder;
use hivedb_index::{EmbedKind, Embedder};
use std::io::Write;
use std::time::Instant;

const CHUNK: usize = 512;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (input, output, kind) = (
        args.next().ok_or("falta el fichero de textos")?,
        args.next().ok_or("falta el fichero de salida")?,
        args.next().unwrap_or_else(|| "doc".into()),
    );
    let kind = match kind.as_str() {
        "query" => EmbedKind::Query,
        _ => EmbedKind::Document,
    };
    let texts: Vec<String> = std::fs::read_to_string(&input)?
        .lines()
        .map(str::to_owned)
        .collect();
    let embedder = LocalEmbedder::multilingual_e5_small()?;
    let row_bytes = embedder.dimension() * 4;

    let done = std::fs::metadata(&output).map_or(0, |m| m.len() as usize) / row_bytes;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&output)?;
    // Descarta una fila a medias de una ejecución interrumpida.
    file.set_len((done * row_bytes) as u64)?;
    eprintln!("{} textos, {done} ya hechos", texts.len());

    let start = Instant::now();
    for (n, chunk) in texts[done..].chunks(CHUNK).enumerate() {
        let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let vectors = embedder.embed(&refs, kind)?;
        let mut bytes = Vec::with_capacity(vectors.len() * row_bytes);
        for vector in &vectors {
            for value in vector {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        file.write_all(&bytes)?;
        let finished = done + (n + 1) * CHUNK;
        eprintln!(
            "{}/{} ({:.0} textos/s)",
            finished.min(texts.len()),
            texts.len(),
            ((n + 1) * CHUNK) as f64 / start.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
