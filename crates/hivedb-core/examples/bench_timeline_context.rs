//! Benchmark sintético: contexto actual vs. modo `timeline` (línea de tiempo + detalle).
//!
//! Hilo de 200 eventos en 4 fases; 20 llevan una «cadena exacta» (ticket, ruta, número) tras
//! más de 170 caracteres de relleno, de modo que la línea de tiempo (tope de línea) no la contiene y
//! solo el detalle puede devolverla. Determinista (semilla fija).
//!
//!   cargo run --release -p hivedb-core --example bench_timeline_context

use hivedb_core::{
    AgentContextRequest, ContextItem, ContextStrategy, EventInput, EventKind, HiveDB, StreamId,
    TimelineConfig,
};
use serde_json::json;
use std::collections::HashSet;
use std::time::Instant;

const EVENTS: usize = 200;
const PHASES: [&str; 4] = ["analisis", "diseno", "implementacion", "revision"];
const BUDGETS: [usize; 4] = [500, 1000, 2000, 4000];

fn filler(i: usize) -> String {
    let words = [
        "revisar",
        "módulo",
        "cola",
        "reintento",
        "contrato",
        "esquema",
        "migración",
        "caché",
        "permiso",
        "auditoría",
        "lote",
        "sesión",
    ];
    (0..28)
        .map(|k| words[(i * 7 + k * 3) % words.len()])
        .collect::<Vec<_>>()
        .join(" ")
}

fn needle(n: usize) -> String {
    match n % 3 {
        0 => format!("TKT-{}", 4000 + n * 13),
        1 => format!("/srv/app/config/prod-{}.yaml", 7000 + n * 7),
        _ => format!("pedido #{}", 91000 + n * 31),
    }
}

fn build() -> (HiveDB, Vec<String>) {
    let db = HiveDB::open_temp().unwrap();
    let mut needles = Vec::new();
    for i in 0..EVENTS {
        let phase = PHASES[i * PHASES.len() / EVENTS];
        let text = if i % 10 == 5 {
            let n = needle(needles.len());
            needles.push(n.clone());
            format!("paso {i}: {} detalle clave {n} fin", filler(i))
        } else {
            format!("paso {i}: {}", filler(i))
        };
        db.append(
            EventInput::new(
                "Agente",
                StreamId::from("bench"),
                EventKind::StateTransition,
            )
            .with_payload(json!({ "description": text, "phase": phase })),
        )
        .unwrap();
    }
    (db, needles)
}

fn text_of(item: &ContextItem) -> &str {
    match item {
        ContextItem::Decision { text, .. }
        | ContextItem::ToolCall { text, .. }
        | ContextItem::PhaseSummary { text, .. }
        | ContextItem::Anomaly { text } => text,
        ContextItem::TimelineEntry { line, .. } => line,
        ContextItem::Episode { summary, .. } => summary,
    }
}

fn main() {
    let (db, needles) = build();
    println!(
        "Hilo sintético: {EVENTS} eventos, {} fases, {} cadenas exactas (>170 caracteres de relleno antes).",
        PHASES.len(),
        needles.len()
    );
    println!("Cada celda promedia {} consultas.\n", needles.len());

    type Mode = (&'static str, Option<TimelineConfig>);
    let modes: [Mode; 4] = [
        ("actual", None),
        ("timeline", Some(TimelineConfig::default())),
        (
            "timeline160",
            Some(TimelineConfig {
                max_line_chars: 160,
                ..Default::default()
            }),
        ),
        (
            "timeline60",
            Some(TimelineConfig {
                max_line_chars: 60,
                ..Default::default()
            }),
        ),
    ];
    // A: la consulta ES la cadena (el modo actual la ancla por coincidencia de subcadena).
    // B: la consulta es una frase natural que contiene la cadena (el modo actual no puede anclarla).
    type Scenario = (&'static str, fn(&str) -> String);
    let scenarios: [Scenario; 2] = [
        ("A · consulta = cadena exacta", |n| n.to_string()),
        ("B · consulta en frase natural", |n| {
            format!("necesito saber qué pasó con {n} antes de seguir")
        }),
    ];

    for (scenario, make_objective) in scenarios {
        println!("== {scenario}");
        println!(
            "{:<12} {:<12} {:>16} {:>17} {:>14} {:>11}",
            "presupuesto",
            "modo",
            "cadena exacta %",
            "pasos con línea %",
            "tokens medios",
            "µs/llamada"
        );
        for budget in BUDGETS {
            for (label, timeline) in &modes {
                let (mut hits, mut covered, mut tokens, mut micros) =
                    (0usize, 0usize, 0usize, 0u128);
                for n in &needles {
                    let req = AgentContextRequest {
                        agents: Vec::new(),
                        task_id: "bench".into(),
                        current_phase: PHASES[PHASES.len() - 1].into(),
                        current_objective: make_objective(n),
                        max_tokens: budget,
                        strategy: ContextStrategy {
                            causal_anchors: true,
                            compress_completed_phases: true,
                            timeline: timeline.clone(),
                            ..Default::default()
                        },
                    };
                    let t = Instant::now();
                    let ctx = db.build_agent_context(req).unwrap();
                    micros += t.elapsed().as_micros();
                    tokens += ctx.estimated_tokens();
                    if ctx.items.iter().any(|i| text_of(i).contains(n.as_str())) {
                        hits += 1;
                    }
                    let seqs: HashSet<u64> = ctx.items.iter().filter_map(|i| i.seq()).collect();
                    covered += seqs.len();
                }
                let q = needles.len();
                println!(
                    "{:<12} {:<12} {:>15.0}% {:>16.0}% {:>14} {:>11}",
                    budget,
                    label,
                    100.0 * hits as f64 / q as f64,
                    100.0 * covered as f64 / (q * EVENTS) as f64,
                    tokens / q,
                    micros / q as u128
                );
            }
        }
        println!();
    }
}
