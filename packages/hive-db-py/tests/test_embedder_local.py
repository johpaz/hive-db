"""Pruebas con el modelo real. Se activan con HIVEDB_E2E_EMBEDDER=1 (y HIVEDB_MODEL_DIR apuntando
a una caché con el modelo, o red para descargarlo)."""

import os
import subprocess
import sys
import textwrap

import pytest
from hivedb import HiveDB

enabled = os.environ.get("HIVEDB_E2E_EMBEDDER") == "1"


def run_python(code, **env):
    return subprocess.run(
        [sys.executable, "-c", textwrap.dedent(code)],
        env={**os.environ, **env},
        capture_output=True,
        text=True,
        timeout=600,
    )


def test_sin_red_ni_modelo_falla_con_codigo(tmp_path):
    result = run_python(
        """
        from hivedb import HiveDB, HiveDBError
        try:
            HiveDB.prepare_embedder()
        except HiveDBError as e:
            print(e.code)
        """,
        HIVEDB_OFFLINE="1",
        HIVEDB_MODEL_DIR=str(tmp_path),
    )
    assert result.stdout.strip() == "EMBEDDER_UNAVAILABLE", result.stderr


@pytest.mark.skipif(not enabled, reason="HIVEDB_E2E_EMBEDDER=1")
def test_prepare_embedder_en_cache():
    prepared = HiveDB.prepare_embedder()
    assert prepared.cached is True
    assert "multilingual-e5-small" in prepared.space_id
    assert os.path.exists(os.path.join(prepared.dir, "model.safetensors"))


@pytest.mark.skipif(not enabled, reason="HIVEDB_E2E_EMBEDDER=1")
def test_busqueda_semantica_entre_idiomas():
    with HiveDB.open(":memory:", embedder="local") as db:
        db.upsert_batch(
            [
                {"id": "paella", "body": "La paella es un plato de arroz típico de Valencia"},
                {"id": "router", "body": "Cómo configurar el router de casa"},
                {"id": "futbol", "body": "El equipo ganó el partido de fútbol por tres goles"},
            ]
        )
        assert db.query_hybrid(text="cooking rice dish from Spain", k=3)[0].id == "paella"
        assert db.query_hybrid(text="cómo cocinar arroz", k=3)[0].id == "paella"


@pytest.mark.skipif(not enabled, reason="HIVEDB_E2E_EMBEDDER=1")
def test_varias_bases_comparten_el_modelo():
    dbs = [HiveDB.open(":memory:", embedder="local") for _ in range(3)]
    try:
        for i, db in enumerate(dbs):
            db.upsert_doc(f"d{i}", body="memoria de agentes")
            assert db.query_hybrid(text="agentes", k=1)[0].id == f"d{i}"
    finally:
        for db in dbs:
            db.close()
