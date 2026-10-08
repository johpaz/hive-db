# La Colmena

Visualización 3D en vivo de HiveDB. Cada visitante recibe **su propia base** (`@johpaz/hive-db`) con cuatro
agentes simulados que escriben, consultan, corrigen y piden permisos; cada operación real del motor se anima
en una escena Three.js.

## Desarrollo

```bash
bun install && (cd web && bun install)
bun run start            # API + WebSocket en :3001
cd web && bun run dev    # UI en http://localhost:5173 (proxy a :3001)
bun test server/test     # sesiones, aislamiento, límites
```

Requiere el binding nativo compilado: `cd ../../packages/hive-db && bun run build:native`.

## Despliegue (contenedor)

```bash
# desde la raíz del repo
podman build -f apps/colmena/Dockerfile -t colmena .      # o docker build
podman run --rm -p 3001:3001 colmena                      # http://127.0.0.1:3001
```

Una sola imagen (~220 MB) sirve API, WebSocket y frontend. Recomendado ≥ 512 MB de RAM; una sesión ocupa pocos MB.
Detrás de un proxy inverso, reenvía `X-Forwarded-For` (el límite de peticiones es por IP) y las cabeceras de WebSocket.

| Variable | Defecto | Efecto |
|---|---|---|
| `PORT` / `HOST` | `3001` / `0.0.0.0` | dirección de escucha |
| `MAX_SESSIONS` | `8` | sesiones simultáneas; la siguiente recibe 503 |
| `SESSION_IDLE_MIN` | `15` | minutos sin pestaña abierta antes de borrar la sesión |
| `MAX_DOCS` | `1500` | tope de recuerdos por sesión |
| `SEED_DOCS` | `240` | recuerdos iniciales |
| `WRITES_PER_MIN` | `40` | peticiones de escritura/consulta por IP y minuto |
| `SESSIONS_PER_MIN` | `6` | sesiones nuevas por IP y minuto |

Sin pestañas abiertas los agentes de una sesión se pausan a los 5 s; los datos viven en un directorio temporal
y se borran al expirar la sesión.

## Qué es real
- Búsqueda híbrida: BM25 + HNSW + RRF (`queryHybrid`), con las tres listas por consulta.
- Ruta del HNSW: `traceVector` (nodos y capas visitados), sin filtro. Las consultas de los agentes filtran por agente
  y el motor las resuelve con búsqueda exacta, así que para ellas la traza es la del HNSW global del mismo vector.
- Event-log: `Fact`, `ToolCall`, `MemoryInvalidate` (`append`), enlaces `causation`, working memory y `can()`.
- Embeddings: bolsa de palabras hasheada de 64 dim (solo demo; el motor acepta cualquier vector).
- Posiciones 3D: PCA calculado en el servidor sobre los vectores reales.
