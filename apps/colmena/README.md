# La Colmena

Visualización 3D en vivo de HiveDB. Agentes simulados usan el motor real (`@johpaz/hive-db`)
y cada operación (insertar, consultar, invalidar, permisos) se anima en una escena Three.js.

## Ejecutar

```bash
bun install && (cd web && bun install)
bun run start            # servidor en :3001 (DB temporal + agentes + WebSocket /live)
cd web && bun run dev    # UI en http://localhost:5173 (proxy a :3001)
```

## Qué es real
- Búsqueda híbrida: BM25 + HNSW + RRF (`queryHybrid`), con las tres listas por consulta.
- Event-log: `Fact`, `ToolCall`, `MemoryInvalidate` (`append`), working memory y `can()`.
- Embeddings: bolsa de palabras hasheada de 64 dim (sólo para la demo; el motor acepta cualquier vector).
- Posiciones 3D: PCA calculado en el servidor sobre los vectores reales.
