//! Benchmark reproducible de la capa semántica de HiveDB.
//!
//! Uso: `cargo run --release -p hivedb-bench -- [documentos] [consultas]`
//! (por defecto 10000 documentos y 200 consultas). Los datos salen de un PRNG
//! con semilla fija, así que dos ejecuciones generan el mismo corpus.

use hivedb_index::{HybridQuery, IndexDoc, SemanticIndex, VectorConfig};
use std::path::Path;
use std::time::{Duration, Instant};

const DIMENSION: usize = 384;
const K: usize = 10;
const SEED: u64 = 0x5EED_1234_ABCD_0001;

const PALABRAS: &[&str] = &[
    "pago",
    "factura",
    "cliente",
    "correo",
    "envio",
    "pedido",
    "cuenta",
    "tarjeta",
    "reembolso",
    "soporte",
    "usuario",
    "contrato",
    "agenda",
    "reunion",
    "tarea",
    "informe",
    "proyecto",
    "payment",
    "invoice",
    "customer",
    "email",
    "shipping",
    "order",
    "account",
    "card",
    "refund",
    "support",
    "user",
    "contract",
    "calendar",
    "meeting",
    "task",
    "report",
    "project",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn vector(&mut self) -> Vec<f32> {
        self.vector_desde(&[], 0.0)
    }

    /// Vector aleatorio mezclado con un centroide (`peso` en 0..1): imita la
    /// estructura de agrupamiento de embeddings reales.
    fn vector_desde(&mut self, centroide: &[f32], peso: f32) -> Vec<f32> {
        let mut v: Vec<f32> = (0..DIMENSION).map(|_| self.unit() - 0.5).collect();
        if !centroide.is_empty() {
            for (x, c) in v.iter_mut().zip(centroide) {
                *x = *x * (1.0 - peso) + c * peso;
            }
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }

    fn texto(&mut self, palabras: usize) -> String {
        (0..palabras)
            .map(|_| PALABRAS[(self.next() % PALABRAS.len() as u64) as usize])
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn percentil(muestras: &mut [Duration], p: f64) -> Duration {
    muestras.sort();
    let i = ((muestras.len() as f64 - 1.0) * p).round() as usize;
    muestras[i]
}

fn medir<F: FnMut(usize)>(consultas: usize, mut f: F) -> (Duration, Duration) {
    let mut muestras: Vec<Duration> = (0..consultas)
        .map(|i| {
            let t = Instant::now();
            f(i);
            t.elapsed()
        })
        .collect();
    (
        percentil(&mut muestras, 0.50),
        percentil(&mut muestras, 0.99),
    )
}

fn tamano_dir(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => tamano_dir(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Memoria residente del proceso en MiB (solo Linux; ceros en otras plataformas).
#[derive(Clone, Copy)]
struct Memoria {
    /// RSS total (`VmRSS`).
    total: f64,
    /// Memoria anónima (`RssAnon`): montón y pilas; no se puede descartar sin swap.
    anonima: f64,
    /// Páginas de ficheros mapeados (`RssFile`): el sistema las puede liberar y
    /// volver a leer del disco.
    ficheros: f64,
}

fn memoria() -> Memoria {
    let estado = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let campo = |nombre: &str| -> f64 {
        estado
            .lines()
            .find(|l| l.starts_with(nombre))
            .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
            .map_or(0.0, |kib| kib / 1024.0)
    };
    Memoria {
        total: campo("VmRSS:"),
        anonima: campo("RssAnon:"),
        // En un sistema de ficheros en RAM (tmpfs) el mapeo cuenta como `RssShmem`.
        ficheros: campo("RssFile:") + campo("RssShmem:"),
    }
}

/// Lee un fichero de `f32` little-endian fila a fila (`DIMENSION` por fila).
fn leer_f32(ruta: &Path) -> std::io::Result<Vec<Vec<f32>>> {
    let bytes = std::fs::read(ruta)?;
    let (filas, _) = bytes.as_chunks::<{ DIMENSION * 4 }>();
    Ok(filas
        .iter()
        .map(|fila| {
            fila.as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect()
        })
        .collect())
}

fn us(d: Duration) -> u128 {
    d.as_micros()
}

/// recall@K del camino vectorial frente a fuerza bruta (coseno = producto
/// punto, los vectores están normalizados).
fn recall_at_k(
    index: &SemanticIndex,
    vectores: &[Vec<f32>],
    consultas: &[Vec<f32>],
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut aciertos = 0usize;
    for q in consultas {
        let mut exacto: Vec<(usize, f32)> = vectores
            .iter()
            .enumerate()
            .map(|(i, v)| (i, dot(q, v)))
            .collect();
        exacto.sort_by(|a, b| b.1.total_cmp(&a.1));
        let verdad: Vec<String> = exacto
            .iter()
            .take(K)
            .map(|(i, _)| format!("doc-{i}"))
            .collect();
        let hits = index.query_hybrid(HybridQuery::default().with_vector(q.clone()).with_k(K))?;
        aciertos += hits.iter().filter(|h| verdad.contains(&h.id)).count();
    }
    Ok(aciertos as f64 / (consultas.len() * K) as f64)
}

/// Barrido de `ef` de búsqueda: recall@K y latencia vectorial p50 por valor.
/// La verdad exacta se calcula una sola vez.
fn barrido_ef(
    index: &SemanticIndex,
    vectores: &[Vec<f32>],
    consultas: &[Vec<f32>],
    valores: &[usize],
) -> Result<(), Box<dyn std::error::Error>> {
    let verdades: Vec<Vec<String>> = consultas
        .iter()
        .map(|q| {
            let mut exacto: Vec<(usize, f32)> = vectores
                .iter()
                .enumerate()
                .map(|(i, v)| (i, dot(q, v)))
                .collect();
            exacto.sort_by(|a, b| b.1.total_cmp(&a.1));
            exacto
                .iter()
                .take(K)
                .map(|(i, _)| format!("doc-{i}"))
                .collect()
        })
        .collect();
    for &ef in valores {
        let mut aciertos = 0usize;
        let (p50, p99) = medir(consultas.len(), |i| {
            let q = HybridQuery::default()
                .with_vector(consultas[i].clone())
                .with_k(K)
                .with_ef_search(ef);
            let hits = index.query_hybrid(q).expect("ef");
            aciertos += hits.iter().filter(|h| verdades[i].contains(&h.id)).count();
        });
        let recall = aciertos as f64 / (consultas.len() * K) as f64;
        println!(
            "ef={ef} recall_at_{K}={recall:.4} vector_p50_us={} vector_p99_us={}",
            us(p50),
            us(p99)
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let docs: usize = args
        .next()
        .map_or(10_000, |a| a.parse().expect("documentos"));
    let consultas: usize = args.next().map_or(200, |a| a.parse().expect("consultas"));

    let mut rng = Rng(SEED);
    // HIVE_BENCH_CLUSTERS=n genera n agrupamientos (vectores tipo embedding real).
    let grupos: usize = std::env::var("HIVE_BENCH_CLUSTERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let centroides: Vec<Vec<f32>> = (0..grupos).map(|_| rng.vector()).collect();
    let generar = |rng: &mut Rng, i: usize| match centroides.len() {
        0 => rng.vector(),
        n => rng.vector_desde(&centroides[i % n], 0.7),
    };
    // HIVE_BENCH_CORPUS=dir usa un corpus real ya embebido (vectors.f32, queries.f32
    // y, opcionalmente, texts.txt / queries.txt con los textos, uno por línea).
    let (vectores, textos, consultas_vec, consultas_txt, docs, consultas) = if let Ok(dir) =
        std::env::var("HIVE_BENCH_CORPUS")
    {
        let dir = Path::new(&dir);
        let mut vectores = leer_f32(&dir.join("vectors.f32"))?;
        let mut consultas_vec = leer_f32(&dir.join("queries.f32"))?;
        vectores.truncate(docs);
        consultas_vec.truncate(consultas);
        let lineas = |nombre: &str, n: usize| -> Vec<String> {
            std::fs::read_to_string(dir.join(nombre))
                .map(|t| t.lines().take(n).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let mut textos = lineas("texts.txt", vectores.len());
        textos.resize(vectores.len(), String::new());
        let mut consultas_txt = lineas("queries.txt", consultas_vec.len());
        consultas_txt.resize(consultas_vec.len(), String::new());
        let (d, c) = (vectores.len(), consultas_vec.len());
        (vectores, textos, consultas_vec, consultas_txt, d, c)
    } else {
        let vectores: Vec<Vec<f32>> = (0..docs).map(|i| generar(&mut rng, i)).collect();
        let textos: Vec<String> = (0..docs).map(|_| rng.texto(12)).collect();
        let consultas_vec: Vec<Vec<f32>> = (0..consultas).map(|i| generar(&mut rng, i)).collect();
        let consultas_txt: Vec<String> = (0..consultas).map(|_| rng.texto(2)).collect();
        (
            vectores,
            textos,
            consultas_vec,
            consultas_txt,
            docs,
            consultas,
        )
    };

    // HIVE_BENCH_EXPORT=ruta vuelca corpus y consultas (f32 little-endian,
    // fila a fila) para que los comparadores usen exactamente los mismos datos.
    if let Ok(ruta) = std::env::var("HIVE_BENCH_EXPORT") {
        std::fs::create_dir_all(&ruta)?;
        let volcar = |nombre: &str, filas: &[Vec<f32>]| -> std::io::Result<()> {
            let bytes: Vec<u8> = filas
                .iter()
                .flatten()
                .flat_map(|x| x.to_le_bytes())
                .collect();
            std::fs::write(Path::new(&ruta).join(nombre), bytes)
        };
        volcar("vectors.f32", &vectores)?;
        volcar("queries.f32", &consultas_vec)?;
        println!("exportado en {ruta}: {docs} vectores, {consultas} consultas, dim {DIMENSION}");
    }

    let dir = tempfile::tempdir()?;
    // HIVE_BENCH_KEEP=ruta copia la base a esa ruta al terminar (para inspeccionar el disco).
    let conservar = std::env::var("HIVE_BENCH_KEEP").ok();
    let config = Some(VectorConfig::new(DIMENSION, "bench:384"));
    let documentos: Vec<IndexDoc> = (0..docs)
        .map(|i| {
            IndexDoc::new(format!("doc-{i}"))
                .with_body(textos[i].clone())
                .with_vector(vectores[i].clone())
        })
        .collect();

    let index = SemanticIndex::open(dir.path(), config.clone())?;
    let t = Instant::now();
    for lote in documentos.chunks(1_000) {
        index.upsert_batch(lote)?;
    }
    let ingesta = t.elapsed();

    let (v50, v99) = medir(consultas, |i| {
        let q = HybridQuery::default()
            .with_vector(consultas_vec[i].clone())
            .with_k(K);
        index.query_hybrid(q).expect("vector");
    });
    let (t50, t99) = medir(consultas, |i| {
        let q = HybridQuery::default()
            .with_text(consultas_txt[i].clone())
            .with_k(K);
        index.query_hybrid(q).expect("texto");
    });
    let (h50, h99) = medir(consultas, |i| {
        let q = HybridQuery::default()
            .with_text(consultas_txt[i].clone())
            .with_vector(consultas_vec[i].clone())
            .with_k(K);
        index.query_hybrid(q).expect("hibrida");
    });

    let recall = recall_at_k(&index, &vectores, &consultas_vec)?;

    // HIVE_BENCH_EF=50,200,800 mide recall y latencia vectorial para cada `ef`.
    if let Ok(lista) = std::env::var("HIVE_BENCH_EF") {
        let valores: Vec<usize> = lista
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        barrido_ef(&index, &vectores, &consultas_vec, &valores)?;
    }

    let rss_poblado = memoria();
    let t = Instant::now();
    drop(index);
    let cierre = t.elapsed();
    let disco = tamano_dir(dir.path());
    if let Some(ruta) = conservar {
        let ruta = Path::new(&ruta);
        let _ = std::fs::remove_dir_all(ruta);
        std::fs::create_dir_all(ruta)?;
        for entrada in std::fs::read_dir(dir.path())?.flatten() {
            let destino = ruta.join(entrada.file_name());
            if entrada.path().is_dir() {
                std::fs::create_dir_all(&destino)?;
                for sub in std::fs::read_dir(entrada.path())?.flatten() {
                    std::fs::copy(sub.path(), destino.join(sub.file_name()))?;
                }
            } else {
                std::fs::copy(entrada.path(), destino)?;
            }
        }
        println!("base conservada en {}", ruta.display());
    }
    if let Ok(entradas) = std::fs::read_dir(dir.path()) {
        for e in entradas.flatten() {
            let bytes = if e.path().is_dir() {
                tamano_dir(&e.path())
            } else {
                e.metadata().map_or(0, |m| m.len())
            };
            let mib = bytes as f64 / 1_048_576.0;
            println!("disco_detalle {}={mib:.1}", e.file_name().to_string_lossy());
        }
    }

    // Memoria del motor aislada de la del arnés (que conserva los vectores y los
    // documentos de la prueba): se mide el incremento al reabrir y tras consultar.
    let antes = memoria();
    let t = Instant::now();
    let reabierto = SemanticIndex::open(dir.path(), config)?;
    let arranque = t.elapsed();
    let tras_abrir = memoria();
    for consulta in &consultas_vec {
        reabierto.query_hybrid(
            HybridQuery::default()
                .with_vector(consulta.clone())
                .with_k(K),
        )?;
    }
    let tras_consultar = memoria();
    let vivos = reabierto.vector_stats().map_or(0, |s| s.0);
    let recall_reabierto = recall_at_k(&reabierto, &vectores, &consultas_vec)?;

    println!("# hivedb-bench docs={docs} consultas={consultas} dim={DIMENSION} k={K}");
    println!(
        "ingesta_docs_por_s={:.0}",
        docs as f64 / ingesta.as_secs_f64()
    );
    println!("vector_p50_us={} vector_p99_us={}", us(v50), us(v99));
    println!("texto_p50_us={} texto_p99_us={}", us(t50), us(t99));
    println!("hibrida_p50_us={} hibrida_p99_us={}", us(h50), us(h99));
    println!("recall_at_{K}={recall:.4}");
    println!("recall_at_{K}_tras_reabrir={recall_reabierto:.4}");
    println!(
        "arranque_en_frio_ms={} vectores_vivos={vivos}",
        arranque.as_millis()
    );
    println!("cierre_ms={}", cierre.as_millis());
    println!("disco_mib={:.1}", disco as f64 / 1_048_576.0);
    // Con el motor poblado (incluye el arnés de la prueba).
    println!(
        "rss_mib={:.1} rss_anon_mib={:.1} rss_file_mib={:.1}",
        rss_poblado.total, rss_poblado.anonima, rss_poblado.ficheros
    );
    // Incremento que provoca solo el motor al reabrir la base y tras consultarla.
    println!(
        "reabierto_rss_anon_mib={:.1} reabierto_rss_file_mib={:.1} (tras abrir: anon {:.1}, file {:.1})",
        tras_consultar.anonima - antes.anonima,
        tras_consultar.ficheros - antes.ficheros,
        tras_abrir.anonima - antes.anonima,
        tras_abrir.ficheros - antes.ficheros,
    );
    Ok(())
}
