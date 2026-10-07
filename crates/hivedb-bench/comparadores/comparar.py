#!/usr/bin/env python3
"""Comparadores de la capa vectorial: sqlite-vec y LanceDB.

Lee el corpus que exporta `hivedb-bench` (HIVE_BENCH_EXPORT) para medir
exactamente los mismos datos, con las mismas métricas que el binario Rust:
latencia p50/p99 de consultas secuenciales, recall@10 frente a fuerza bruta,
arranque (apertura + primera consulta), disco y RSS.

Uso: comparar.py <dir_export> <sistema> [consultas_max]
sistemas: sqlite-vec | lancedb-flat | lancedb-hnsw
Cada sistema debe correr en su propio proceso para que el RSS sea comparable.
"""
import os
import shutil
import sqlite3
import sys
import tempfile
import time

import numpy as np
import psutil

DIM = 384
K = 10


def cargar(directorio, consultas_max):
    vectores = np.fromfile(os.path.join(directorio, "vectors.f32"), dtype="<f4").reshape(-1, DIM)
    consultas = np.fromfile(os.path.join(directorio, "queries.f32"), dtype="<f4").reshape(-1, DIM)
    return vectores, consultas[:consultas_max]


def tamano_dir(ruta):
    total = 0
    for base, _, archivos in os.walk(ruta):
        total += sum(os.path.getsize(os.path.join(base, a)) for a in archivos)
    return total


def verdad_exacta(vectores, consultas):
    # Vectores normalizados: coseno = producto punto.
    return [set(np.argsort(-(vectores @ q))[:K].tolist()) for q in consultas]


# --- adaptadores: abrir(ruta) -> consulta(q) -> lista de ids ------------------

class SqliteVec:
    nombre = "sqlite-vec"

    def construir(self, ruta, vectores):
        import sqlite_vec
        db = sqlite3.connect(os.path.join(ruta, "v.db"))
        db.enable_load_extension(True)
        sqlite_vec.load(db)
        db.execute(f"create virtual table v using vec0(embedding float[{DIM}] distance_metric=cosine)")
        db.execute("begin")
        for i, v in enumerate(vectores):
            db.execute("insert into v(rowid, embedding) values (?, ?)", (i, v.tobytes()))
        db.commit()
        db.close()

    def abrir(self, ruta):
        import sqlite_vec
        self.db = sqlite3.connect(os.path.join(ruta, "v.db"))
        self.db.enable_load_extension(True)
        sqlite_vec.load(self.db)

    def consultar(self, q):
        filas = self.db.execute(
            "select rowid from v where embedding match ? and k = ? order by distance",
            (q.tobytes(), K),
        ).fetchall()
        return [f[0] for f in filas]


class LanceDB:
    def __init__(self, indice):
        self.indice = indice
        self.nombre = "lancedb-hnsw" if indice else "lancedb-flat"

    def construir(self, ruta, vectores):
        import lancedb
        import pyarrow as pa
        esquema = pa.schema([
            ("id", pa.int64()),
            ("vector", pa.list_(pa.float32(), DIM)),
        ])
        tabla_arrow = pa.table(
            {
                "id": pa.array(np.arange(len(vectores)), type=pa.int64()),
                "vector": pa.FixedSizeListArray.from_arrays(pa.array(vectores.ravel(), type=pa.float32()), DIM),
            },
            schema=esquema,
        )
        db = lancedb.connect(ruta)
        tabla = db.create_table("t", data=tabla_arrow)
        if self.indice:
            tabla.create_index(metric="cosine", index_type="IVF_HNSW_SQ", vector_column_name="vector")

    def abrir(self, ruta):
        import lancedb
        self.tabla = lancedb.connect(ruta).open_table("t")

    def consultar(self, q):
        # LANCE_NPROBES / LANCE_EF / LANCE_REFINE ajustan la búsqueda (por defecto, los de LanceDB).
        busqueda = self.tabla.search(q).metric("cosine").limit(K).select(["id"])
        if os.environ.get("LANCE_NPROBES"):
            busqueda = busqueda.nprobes(int(os.environ["LANCE_NPROBES"]))
        if os.environ.get("LANCE_EF"):
            busqueda = busqueda.ef(int(os.environ["LANCE_EF"]))
        if os.environ.get("LANCE_REFINE"):
            busqueda = busqueda.refine_factor(int(os.environ["LANCE_REFINE"]))
        filas = busqueda.to_list()
        return [f["id"] for f in filas]


SISTEMAS = {
    "sqlite-vec": SqliteVec,
    "lancedb-flat": lambda: LanceDB(False),
    "lancedb-hnsw": lambda: LanceDB(True),
}


def main():
    directorio, sistema = sys.argv[1], sys.argv[2]
    consultas_max = int(sys.argv[3]) if len(sys.argv) > 3 else 1000
    vectores, consultas = cargar(directorio, consultas_max)
    verdad = verdad_exacta(vectores, consultas)
    motor = SISTEMAS[sistema]()

    ruta = tempfile.mkdtemp(prefix="cmp-")
    try:
        t = time.perf_counter()
        motor.construir(ruta, vectores)
        ingesta = time.perf_counter() - t

        # Arranque en frío: abrir + primera consulta completada.
        t = time.perf_counter()
        motor.abrir(ruta)
        motor.consultar(consultas[0])
        arranque = time.perf_counter() - t

        tiempos, aciertos = [], 0
        for q, real in zip(consultas, verdad):
            t = time.perf_counter()
            ids = motor.consultar(q)
            tiempos.append(time.perf_counter() - t)
            aciertos += len(real & set(ids))
        tiempos.sort()
        p = lambda f: tiempos[round((len(tiempos) - 1) * f)] * 1e6

        print(f"# {motor.nombre} docs={len(vectores)} consultas={len(consultas)} dim={DIM} k={K}")
        print(f"ingesta_docs_por_s={len(vectores) / ingesta:.0f}")
        print(f"vector_p50_us={p(0.5):.0f} vector_p99_us={p(0.99):.0f}")
        print(f"recall_at_{K}={aciertos / (len(consultas) * K):.4f}")
        print(f"arranque_en_frio_ms={arranque * 1000:.0f}")
        print(f"disco_mib={tamano_dir(ruta) / 1048576:.1f}")
        print(f"rss_mib={psutil.Process().memory_info().rss / 1048576:.1f}")
    finally:
        shutil.rmtree(ruta, ignore_errors=True)


if __name__ == "__main__":
    main()
