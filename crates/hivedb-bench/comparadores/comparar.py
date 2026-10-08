#!/usr/bin/env python3
"""Comparadores de la capa vectorial: sqlite-vec y LanceDB.

Lee el corpus que exporta `hivedb-bench` (HIVE_BENCH_EXPORT) para medir
exactamente los mismos datos, con las mismas métricas que el binario Rust:
latencia p50/p99 de consultas secuenciales, recall@10 frente a fuerza bruta,
arranque (apertura + primera consulta), disco y RSS.

Uso: comparar.py <dir_export> <sistema> [consultas_max]
sistemas: sqlite-vec | libsql | lancedb-flat | lancedb-hnsw
Cada sistema debe correr en su propio proceso para que el RSS sea comparable.
"""
import os
import shutil
import sqlite3
import sys
import tempfile
import time

import numpy as np

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


class LibSql:
    """libSQL (el motor de Turso) en modo embebido, sobre un fichero local: índice
    vectorial DiskANN (`libsql_vector_idx`). No usa Turso Cloud: medir por red
    sumaría la latencia de la conexión y no sería comparable con un motor en proceso."""

    nombre = "libsql"

    def construir(self, ruta, vectores):
        import libsql
        db = libsql.connect(os.path.join(ruta, "v.db"))
        db.execute(f"create table v (id integer primary key, e F32_BLOB({DIM}))")
        for i, v in enumerate(vectores):
            db.execute("insert into v values (?, ?)", (i, v.tobytes()))
        db.commit()
        # El índice se crea con los datos ya cargados.
        # LIBSQL_COMPRESS=float8|float1bit comprime los vecinos del índice (por defecto
        # guarda cada vecino como `f32` completo: ~80 KB de índice por vector).
        opciones = "'metric=cosine'"
        if os.environ.get("LIBSQL_COMPRESS"):
            opciones += f", 'compress_neighbors={os.environ['LIBSQL_COMPRESS']}'"
        db.execute(f"create index vi on v(libsql_vector_idx(e, {opciones}))")
        db.commit()
        db.close()

    def abrir(self, ruta):
        import libsql
        self.db = libsql.connect(os.path.join(ruta, "v.db"))

    def consultar(self, q):
        filas = self.db.execute(
            "select id from vector_top_k('vi', ?, ?)", (q.tobytes(), K)
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
    "libsql": LibSql,
    "lancedb-flat": lambda: LanceDB(False),
    "lancedb-hnsw": lambda: LanceDB(True),
}


def memoria():
    """(total, anónima, ficheros) en MiB, de /proc/self/status (Linux)."""
    campos = {}
    with open("/proc/self/status") as f:
        for linea in f:
            clave, _, resto = linea.partition(":")
            if clave in ("VmRSS", "RssAnon", "RssFile", "RssShmem"):
                campos[clave] = int(resto.split()[0]) / 1024
    # En un sistema de ficheros en RAM (tmpfs) el mapeo cuenta como RssShmem.
    return (
        campos.get("VmRSS", 0.0),
        campos.get("RssAnon", 0.0),
        campos.get("RssFile", 0.0) + campos.get("RssShmem", 0.0),
    )


def main():
    directorio, sistema = sys.argv[1], sys.argv[2]
    consultas_max = int(sys.argv[3]) if len(sys.argv) > 3 else 1000
    vectores, consultas = cargar(directorio, consultas_max)
    verdad = verdad_exacta(vectores, consultas)
    motor = SISTEMAS[sistema]()
    # Memoria del motor aislada de la del arnés (vectores y verdad exacta en RAM).
    antes = memoria()

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
        total, anonima, ficheros = memoria()
        print(f"rss_mib={total:.1f} rss_anon_mib={anonima:.1f} rss_file_mib={ficheros:.1f}")
        print(
            f"motor_rss_anon_mib={anonima - antes[1]:.1f} motor_rss_file_mib={ficheros - antes[2]:.1f}"
        )
    finally:
        shutil.rmtree(ruta, ignore_errors=True)


if __name__ == "__main__":
    main()
