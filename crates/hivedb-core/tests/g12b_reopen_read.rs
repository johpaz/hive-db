//! Tras un cierre limpio, el log debe seguir siendo legible por `seq`.
//!
//! `seq_to_agent` es un índice en memoria que solo se rellena al escribir o al
//! escanear tras un cierre sucio; si no se reconstruye en la reapertura limpia,
//! `read(seq)` no encuentra eventos que sí están en disco.

mod common;

use common::fact;
use hivedb_core::HiveDB;
use tempfile::tempdir;

#[test]
fn read_por_seq_funciona_tras_cierre_limpio() {
    let dir = tempdir().unwrap();
    let seqs: Vec<u64> = {
        let db = HiveDB::open(dir.path()).unwrap();
        vec![
            db.append(fact("swarmA:coordinator", "s1")).unwrap(),
            db.append(fact("swarmB:coordinator", "s2")).unwrap(),
            db.append(fact("swarmA:coordinator", "s1")).unwrap(),
        ]
    };

    let db = HiveDB::open(dir.path()).unwrap();
    for seq in seqs {
        let event = db
            .read(seq)
            .unwrap_or_else(|e| panic!("read({seq}) tras reabrir: {e}"));
        assert_eq!(event.seq, seq);
    }
}

#[test]
fn el_consentimiento_sobrevive_a_reabrir_y_reconstruir() {
    use hivedb_core::{AgentId, EventInput, EventKind, Scope, StreamId};

    let dir = tempdir().unwrap();
    {
        let db = HiveDB::open(dir.path()).unwrap();
        let from = AgentId::from("PM");
        db.append(EventInput::new(
            from.clone(),
            StreamId::from("consent"),
            EventKind::ConsentGranted {
                from,
                to: AgentId::from("Backend"),
                scope: Scope::new("deploy", "staging"),
                expires: None,
            },
        ))
        .unwrap();
    }

    // La reconstrucción lee los eventos por seq; si no los encontrara los
    // saltaría sin error y el permiso desaparecería.
    let db = HiveDB::open(dir.path()).unwrap();
    db.wipe_projections_and_rebuild().unwrap();
    assert!(db.can("Backend", "deploy", "staging").unwrap().allowed());
}
