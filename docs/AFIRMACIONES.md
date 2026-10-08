# Qué podemos afirmar sobre HiveDB (y cómo decirlo)

Guía de redacción para el README, la web, las presentaciones y los mensajes de lanzamiento. Su
objetivo es que **todo lo que se diga de HiveDB sea verificable**: una promesa que no se cumple
cuesta más confianza de la que gana.

Cada afirmación tiene un estado:

- ✔ **Verificada.** Se puede decir tal cual (con su condición).
- ⚠ **Matizar.** Es cierta con una condición que hay que decir.
- ✖ **Evitar.** Promete algo que el motor no hace hoy.

Las cifras salen de [`BENCHMARKS.md`](BENCHMARKS.md) (100.000 frases reales de Wikipedia en español
e inglés, embeddings `multilingual-e5-small`, una máquina de 16 hilos, disco NVMe). Si cambian allí,
cambian aquí.

---

## 1. Contra qué nos comparamos de verdad

La mayoría de quienes programan agentes arman la memoria con **tres piezas**: una base relacional
(Postgres) para los datos y la memoria del agente, una base vectorial (pgvector, Pinecone, Qdrant…)
para recuperar por significado, y un almacén en memoria (Redis) para la caché de conversación y para
juntar mensajes antes de pasárselos al agente. A veces se suma un buscador de texto
(Elasticsearch/OpenSearch) y un sistema de logs aparte para saber qué hizo el agente.

Ese es el marco correcto: **es la alternativa que el lector ya tiene**. Qué sustituye HiveDB y qué no:

| Pieza habitual | ¿La sustituye HiveDB? | Matiz |
|---|---|---|
| Postgres para el estado y la memoria del agente | **En parte** | Colecciones de documentos con índices y versionado: sí. SQL, joins, reportes: no |
| Base vectorial (pgvector, Pinecone…) | **Sí** (hasta lo probado) | Búsqueda vectorial y por palabras a la vez. Probado hasta 100.000 documentos; millones no |
| Redis para la caché de conversación | **En parte** | Memoria de trabajo con TTL **en la RAM del proceso** (se pierde al cerrar y no se comparte entre procesos). No hay pub/sub entre procesos ni entre máquinas |
| Juntar mensajes antes de dárselos al agente | **Ayuda, no lo hace solo** | Mensajes en colecciones, memoria de trabajo para el buffer y `buildAgentContext` para armar la ventana; el *debounce* o la unión de mensajes lo programa tu aplicación |
| Elasticsearch / búsqueda de texto | **Sí**, a escala de agente | BM25 con *stemming* en español |
| Sistema de logs / auditoría | **Sí** | Log inmutable con causalidad, si tu runtime emite los eventos con la forma del contrato |

Y la comparación con los **motores embebidos** (la que haría quien ya usa SQLite o LanceDB): frente a
sqlite-vec y LanceDB, HiveDB añade el log causal, el consentimiento, la búsqueda híbrida y la
reactividad; a cambio ingiere más despacio y ocupa más disco (ver §3).

---

## 2. Afirmaciones: qué decir y qué no

| # | Afirmación habitual | Estado | Por qué | Redacción recomendada |
|---|---|:---:|---|---|
| 1 | «Es todo eso en una sola librería» | ✔ | Colecciones, log, búsqueda híbrida, memoria de trabajo, consentimiento y reactividad están en el mismo motor | «Una librería en lugar de tres o cuatro servicios» (con la tabla del §1 a mano) |
| 2 | «Responde en ~1 ms con 100.000 documentos» | ⚠ | Vector: 1,4 ms (p50), 1,8 ms (p99). Texto: 2,5 ms. Híbrida: 4,3 ms (p99 9,5 ms) | «Entre 1 y 5 ms con 100.000 frases reales» |
| 3 | «Sin servidor, sin cuenta, sin API key» | ⚠ | Cierto para el motor. La búsqueda por significado necesita embeddings: el modelo local descarga ~470 MB una vez; un proveedor externo exige su clave | «Sin servidor ni cuenta. Para buscar por significado usa un modelo local (descarga única) o el proveedor que prefieras» |
| 4 | «Funciona sin internet» | ⚠ | El motor sí. Activar el embedder local requiere una descarga la primera vez (se puede hacer de antemano) | «Funciona sin conexión; la primera vez que activas el modelo local lo descarga, o puedes llevarlo ya descargado» |
| 5 | «Cada cosa queda registrada y no se borra» | ⚠ | Cierto para el log. Pero lo inmutable choca con el **derecho de supresión** (RGPD) | «El historial es inmutable: no guardes datos personales en él; guarda referencias» |
| 6 | «Puedes reconstruir por qué hizo algo, como una caja negra» | ⚠ | Solo si el runtime emite eventos con `causation` e intención, con la forma del contrato ([`AGENT_INTEGRATION.md`](AGENT_INTEGRATION.md)). No es automático | «Si registras las decisiones y las llamadas de tu agente, HiveDB reconstruye el camino» |
| 7 | «Si revocas un permiso, se revoca en el motor, no en una capa que se pueda saltar» | ✖ | `can()` consulta el grafo y deja un evento de auditoría, pero **no bloquea** nada: si tu código no la llama, nada se impide | «El motor registra quién autorizó qué y hasta cuándo y responde si una acción está permitida; tu agente lo consulta antes de actuar» |
| 8 | «Los agentes se enteran solos de los cambios» | ⚠ | Las suscripciones funcionan **dentro del mismo proceso**; la base se abre en exclusiva | «Los componentes de tu aplicación reciben avisos al instante, sin sondear» |
| 9 | «Ideal para hospitales, bancos y entidades públicas» | ✖ | No hay cifrado en reposo ni control de acceso propio; el log no se puede purgar; no hay certificaciones | «Tus datos no salen de tu máquina. El cifrado, el acceso y la política de borrado dependen de tu despliegue» |
| 10 | «Ninguna otra base te da esto» | ✖ | Otros dan partes (EventStoreDB, el grafo temporal de Zep, los *checkpoints* de LangGraph) | «Pocas combinan en un solo motor log causal, consentimiento y búsqueda híbrida» |
| 11 | «Memoria de trabajo que expira sola» | ⚠ | Cierto, pero vive en la RAM del proceso: se pierde al cerrar y no se comparte | «Memoria de trabajo con caducidad, en memoria del proceso» |
| 12 | «sqlite-vec ingiere más rápido» | ✔ | 88.000 frente a 4.900 documentos/s, por lo que HiveDB hace además (texto, log, grafo) | Decirlo con la cifra y la razón |
| 13 | «Más rápida que LanceDB» | ⚠ | Solo a igual recall y con los parámetros de búsqueda ajustados en LanceDB | «A igual precisión, 1,4 ms frente a ~3 ms de LanceDB ajustado (100.000 frases reales)» |
| 14 | «Todavía no tiene benchmarks con datos reales» | ✖ (obsoleta) | Ya los hay: 100.000 frases reales, con comparadores | «Medido con 100.000 frases reales; no se ha probado con millones» |
| 15 | «Es un proyecto joven, sin versión 1.0» | ✔ | Versión 0.x; la API puede cambiar | Mantenerlo |
| 16 | «Tus datos no salen de tu máquina» | ⚠ | Cierto con el modelo local. Con embeddings de una API, el texto va a ese proveedor | «Con el modelo local, el texto no sale de tu máquina» |

---

## 3. Cifras que sí se pueden citar

Todas con su condición: **100.000 documentos de 384 dimensiones, embeddings reales, k = 10, una
máquina de 16 hilos con disco NVMe**.

| | HiveDB | Referencia |
|---|---|---|
| Búsqueda vectorial (p50 / p99) | 1,4 / 1,8 ms con recall 0,982 | LanceDB ajustado: ~3 ms con recall 0,965 · libSQL (`float8`): 10 ms con recall 0,975 |
| Con recall 0,994 | 2,5 / 3,5 ms | LanceDB: ~10 ms para 0,9998 |
| Búsqueda de texto / híbrida (p50) | 2,5 ms / 4,3 ms | — |
| Apertura de una base poblada | 46 ms | sqlite-vec 75 ms · LanceDB 89–212 ms |
| Ingesta por lotes | ~4.900 docs/s | sqlite-vec 88.000 · LanceDB 9.500–65.000 · libSQL 79–524 |
| Disco | 242,5 MiB | sqlite-vec 149 MiB · LanceDB 147–203 MiB · libSQL (`float8`) 2.758 MiB |
| Memoria anónima del motor (tras reabrir y consultar) | ~31 MiB (+148 MiB de vectores mapeados desde disco) | — |

Lo que **no** se ha medido y por tanto no se afirma: más de 100.000 documentos, varias máquinas,
Windows, concurrencia de muchos clientes, ni la latencia con el modelo local calculando la consulta
(+~50 ms por la frase).

---

## 4. Cuándo NO usar HiveDB

Esta sección genera confianza; conviene mantenerla y ampliarla:

- **Necesitas SQL, reportes o una aplicación tradicional** (facturación, inventario): usa Postgres o
  SQLite. HiveDB va al lado, no los reemplaza.
- **Cargas millones de documentos de golpe:** sqlite-vec ingiere unas 18 veces más rápido, y lo probado
  llega a 100.000.
- **Quieres que un servicio «extraiga recuerdos» con un LLM:** eso lo hacen Mem0 o Zep. HiveDB es el
  almacén que podrías usar debajo.
- **Varios procesos o varias máquinas sobre la misma base:** hoy un solo proceso es dueño de cada
  base y no hay sincronización.
- **Necesitas cifrado en reposo, control de acceso o purga selectiva del historial.**
- **Necesitas estabilidad de API garantizada:** es una versión 0.x.

---

## 5. Texto propuesto (versión corregida)

> ### ¿Por qué HiveDB?
>
> **Tu agente necesita memoria, y hoy armarla es un rompecabezas.**
> Lo habitual es juntar varias piezas: Postgres para los datos y la memoria del agente, una base
> vectorial (pgvector, Pinecone, Qdrant…) para recordar por significado, Redis para la caché de la
> conversación y para juntar mensajes antes de pasárselos al agente, y, si quieres saber qué hizo el
> agente, un sistema de logs aparte. Son varias cosas que instalar, conectar, pagar y mantener
> sincronizadas.
>
> **HiveDB reúne gran parte de eso en una librería.** Con `bun add @johpaz/hive-db` tu agente tiene:
>
> - **Memoria de trabajo** con caducidad, en la memoria de tu proceso.
> - **Memoria de largo plazo** que busca por significado y por palabras exactas a la vez. Con 100.000
>   frases reales responde en 1–5 ms.
> - **Historial inmutable** de lo que el agente aprende y hace, con la relación entre causas y efectos.
> - **Colecciones de documentos** para el estado que cambia (sesiones, mensajes, configuración).
>
> No hay servidor que levantar ni cuenta en la nube. Vive dentro de tu aplicación, como un directorio.
> Para buscar por significado usas un modelo local (se descarga una vez) o el proveedor de
> embeddings que prefieras.
>
> ### Lo que aporta frente a juntar piezas sueltas
>
> 1. **Puedes saber por qué tu agente hizo algo.** Si registras sus decisiones y llamadas, HiveDB
>    reconstruye el camino paso a paso. Sirve para depurar y para explicar lo que pasó.
> 2. **Llevas la cuenta de qué puede hacer cada agente.** El motor registra quién autorizó qué y
>    hasta cuándo, y responde si una acción está permitida; tu agente lo consulta antes de actuar,
>    y cada consulta queda en el historial.
> 3. **Los componentes se enteran de los cambios al instante,** sin preguntar cada segundo si hay
>    algo nuevo.
> 4. **Con el modelo local, tus datos no salen de tu máquina.** El motor funciona sin conexión.
>
> ### Cuándo NO usarla
>
> - Si necesitas SQL, reportes o una aplicación tradicional, usa Postgres o SQLite; HiveDB va al lado.
> - Si cargas millones de documentos de golpe, sqlite-vec ingiere mucho más rápido (88.000 frente a
>   4.900 documentos por segundo): HiveDB indexa además el texto y guarda el historial.
> - Si quieres que un servicio extraiga recuerdos con un LLM, eso lo hacen Mem0 o Zep; HiveDB es el
>   almacén que podrías usar debajo.
> - Si necesitas varios procesos o máquinas sobre la misma base, cifrado en reposo o borrado
>   selectivo del historial, todavía no lo ofrece.
> - Es una versión 0.x: la API puede cambiar. Medido con 100.000 frases reales; no se ha probado con
>   millones.
>
> ### En una frase
>
> **Postgres guarda tus datos. HiveDB guarda la memoria de tu agente: lo que sabe, lo que hizo, por
> qué lo hizo y qué tiene permitido hacer, en una sola librería y en tu máquina.**

---

## 6. Reglas para seguir escribiendo

1. **Cada cifra lleva su condición:** cuántos documentos, qué datos, qué máquina. Sin condición no se cita.
2. **Sin absolutos** («ninguna», «siempre», «imposible de saltar»). Si hay una excepción, se dice.
3. **Comparar con lo que el lector usa hoy** (el stack del §1), con datos medidos o, si no los hay, con
   la documentación pública del otro proyecto, señalándolo.
4. **Decir lo que no hace** al lado de lo que hace; es lo que da credibilidad.
5. **Antes de publicar,** repasar esta tabla: ¿el motor ha cambiado desde la última revisión?
   ¿Cambió alguna cifra en `BENCHMARKS.md`? Si sí, actualizar aquí primero.
6. **Revisar cuando se publique cada pieza pendiente:** embedder local en los paquetes, cifrado,
   sincronización. Cada una puede mover una afirmación de ⚠/✖ a ✔.
