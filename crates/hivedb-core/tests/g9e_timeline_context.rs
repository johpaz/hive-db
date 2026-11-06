use hivedb_core::{
    AgentContextRequest, ContextItem, ContextStrategy, EventInput, EventKind, HiveDB, StreamId,
    TimelineConfig,
};
use serde_json::json;

fn decision(db: &HiveDB, text: &str, phase: &str) -> u64 {
    db.append(
        EventInput::new(
            "Backend",
            StreamId::from("task-1"),
            EventKind::StateTransition,
        )
        .with_payload(json!({ "description": text, "phase": phase })),
    )
    .unwrap()
}

fn request(
    max_tokens: usize,
    objective: &str,
    timeline: Option<TimelineConfig>,
) -> AgentContextRequest {
    AgentContextRequest {
        agents: Vec::new(),
        task_id: "task-1".into(),
        current_phase: "impl".into(),
        current_objective: objective.into(),
        max_tokens,
        strategy: ContextStrategy {
            timeline,
            ..Default::default()
        },
    }
}

fn timeline_tokens(items: &[ContextItem]) -> usize {
    items
        .iter()
        .filter(|i| matches!(i, ContextItem::TimelineEntry { .. }))
        .map(|i| i.estimated_tokens())
        .sum()
}

#[test]
fn without_timeline_output_is_unchanged() {
    let db = HiveDB::open_temp().unwrap();
    decision(&db, "uno", "impl");
    let ctx = db.build_agent_context(request(4096, "", None)).unwrap();
    assert!(
        !ctx.items
            .iter()
            .any(|i| matches!(i, ContextItem::TimelineEntry { .. }))
    );
}

#[test]
fn timeline_respects_its_share_and_total_budget() {
    let db = HiveDB::open_temp().unwrap();
    let mut first = u64::MAX;
    let mut last = 0;
    for i in 0..300 {
        let seq = decision(&db, &format!("paso {i} del plan"), "impl");
        first = first.min(seq);
        last = last.max(seq);
    }
    let budget = 1000;
    let ctx = db
        .build_agent_context(request(budget, "", Some(TimelineConfig::default())))
        .unwrap();
    assert!(ctx.estimated_tokens() <= budget);
    assert!(timeline_tokens(&ctx.items) as f32 <= budget as f32 * 0.35 + 1.0);
    // Conserva primero y último evento.
    assert!(ctx.items.iter().any(|i| i.seq() == Some(first)));
    assert!(ctx.items.iter().any(|i| i.seq() == Some(last)));
}

#[test]
fn detail_recovers_exact_string_beyond_the_line_cap() {
    let db = HiveDB::open_temp().unwrap();
    let long = format!(
        "{} ruta /srv/app/config/prod-7731.yaml {}",
        "x".repeat(200),
        "y".repeat(50)
    );
    decision(&db, &long, "planning");
    for i in 0..50 {
        decision(&db, &format!("relleno {i}"), "impl");
    }
    let ctx = db
        .build_agent_context(request(
            2000,
            "prod-7731 yaml",
            Some(TimelineConfig::default()),
        ))
        .unwrap();
    let full = ctx.items.iter().any(|i| {
        matches!(i, ContextItem::Decision { text, .. } if text.contains("/srv/app/config/prod-7731.yaml"))
    });
    assert!(full, "el detalle debe traer el texto original completo");
}

#[test]
fn oversized_detail_does_not_block_smaller_ones() {
    let db = HiveDB::open_temp().unwrap();
    decision(&db, &format!("{} clave", "g".repeat(8000)), "impl");
    decision(&db, "clave corta importante", "impl");
    let cfg = TimelineConfig {
        timeline_share: 0.2,
        ..Default::default()
    };
    let ctx = db
        .build_agent_context(request(300, "clave", Some(cfg)))
        .unwrap();
    assert!(ctx
        .items
        .iter()
        .any(|i| matches!(i, ContextItem::Decision { text, .. } if text == "clave corta importante")
            || matches!(i, ContextItem::TimelineEntry { line, .. } if line == "clave corta importante")));
}

#[test]
fn timeline_context_is_deterministic() {
    let db = HiveDB::open_temp().unwrap();
    for i in 0..40 {
        decision(&db, &format!("evento {i}"), "impl");
    }
    let a = db
        .build_agent_context(request(500, "evento", Some(TimelineConfig::default())))
        .unwrap();
    let b = db
        .build_agent_context(request(500, "evento", Some(TimelineConfig::default())))
        .unwrap();
    assert_eq!(a.content_hash(), b.content_hash());
}
