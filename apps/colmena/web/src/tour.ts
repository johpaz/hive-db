import { api } from "./api";
import { useStore } from "./store";
import type { XYZ } from "./types";

export interface TourStep {
  title: string;
  body: string;
  cam: XYZ;
  target: XYZ;
  /** Operación real que se dispara al entrar al paso. */
  run?: () => Promise<unknown>;
  /** Abre el Laboratorio al entrar al paso (y lo cierra en los demás). */
  lab?: boolean;
}

const firstOf = (agent: string) => useStore.getState().docs.find((d) => d.agent === agent && d.alive);

const ask = (text: string, extra: Record<string, unknown> = {}) => () =>
  api("/query", { body: { text, agent: "tú", k: 6, ...extra } });

export const STEPS: TourStep[] = [
  {
    title: "Una memoria que puedes ver",
    body: "Cada punto es un recuerdo real guardado en HiveDB. Su posición viene de su vector (proyectado a 3D): los recuerdos parecidos quedan cerca y forman nubes por tema. Los colores de la leyenda son los temas.",
    cam: [34, 20, 50],
    target: [0, 0, 0],
  },
  {
    title: "Agentes que usan el motor",
    body: "Estos cuatro agentes escriben y consultan la base de verdad. Cada destello es una operación: un rayo que nace en el agente es un recuerdo nuevo guardándose (upsert_doc + un evento Fact).",
    cam: [26, 12, 26],
    target: [8, 3, 8],
  },
  {
    title: "Working memory: lo que tienen a mano",
    body: "Las esferitas que orbitan junto a cada agente son su memoria de trabajo: lo último que guardó, con un TTL de 30 s (workingSet). Cuando caduca, la esfera desaparece, pero el recuerdo sigue en la memoria a largo plazo.",
    cam: [18, 8, 20],
    target: [6, 2, 6],
  },
  {
    title: "El event-log, inmutable",
    body: "A la izquierda, la hélice es el log append-only: cada esfera es un evento (ámbar = hecho, cian = consulta, rojo = olvido). Las líneas son enlaces de causalidad reales; haz clic en una esfera para ver su cadena.",
    cam: [-12, 4, 22],
    target: [-32, 0, 0],
  },
  {
    title: "Una búsqueda híbrida, paso a paso",
    body: "Lanzamos la consulta «vuelo hotel playa». Primero baja por las capas del HNSW (ruta real del grafo, en ámbar), luego el carril de texto BM25 en cian, y al final la fusión RRF ilumina en blanco los mejores resultados.",
    cam: [20, 10, 30],
    target: [-6, -1, 0],
    run: ask("vuelo hotel playa"),
  },
  {
    title: "Solo texto: BM25",
    body: "Misma idea, pero solo con el índice invertido (tantivy). Sin vectores: solo cuentan las palabras exactas. Fíjate en que no hay ruta HNSW y los scores son BM25 puros, no similitud.",
    cam: [16, 8, 26],
    target: [-4, 0, 0],
    run: ask("hotel playa", { mode: "text" }),
  },
  {
    title: "Solo vector: HNSW",
    body: "Ahora solo el índice vectorial. El grafo salta de capa en capa hasta el vecindario más cercano al vector de la consulta y devuelve similitud coseno. Aquí el embedder es un modelo simple de demostración; en producción usarías uno semántico.",
    cam: [22, 10, 28],
    target: [-2, 0, 0],
    run: ask("vuelo hotel playa", { mode: "vector" }),
  },
  {
    title: "Filtrar por agente",
    body: "Limitamos la búsqueda a los recuerdos de «ada». Con un filtro, HiveDB hace una búsqueda vectorial exacta sobre el subconjunto (sin recorrer el HNSW), así que la ruta animada desaparece pero el resultado es preciso.",
    cam: [24, 12, 30],
    target: [0, 0, 0],
    run: ask("vuelo hotel playa", { only: "ada" }),
  },
  {
    title: "Olvidar también deja huella",
    body: "Invalidar un recuerdo no borra la historia: añade un evento «MemoryInvalidate» enlazado al original (rojo en la hélice), y el punto se apaga y sale de las búsquedas.",
    cam: [18, 8, 26],
    target: [0, 0, 0],
    run: async () => {
      const d = firstOf("ada") ?? useStore.getState().docs.find((x) => x.alive);
      if (d) await api("/invalidate", { body: { id: d.id } });
    },
  },
  {
    title: "Permisos entre agentes",
    body: "Ahora «bruno» intenta leer la memoria de «ada» y «dante» la de «cleo». El motor consulta su grafo de consentimiento (can()): anillo verde si se permite, rojo si se deniega. Míralo en la actividad de la derecha.",
    cam: [30, 14, 34],
    target: [0, 2, 0],
    run: async () => {
      await api("/gate", { body: { agent: "bruno", target: "ada" } });
      await api("/gate", { body: { agent: "dante", target: "cleo" } });
    },
  },
  {
    title: "Llévalo a tu proyecto",
    body: "Lo que ves corre sobre HiveDB, un motor en Rust con binding para Node y Bun: instálalo con «bun add @johpaz/hive-db» y usa upsertDoc, queryHybrid, append y buildAgentContext. El código es abierto (Apache-2.0): github.com/johpaz/hive-db.",
    cam: [34, 20, 50],
    target: [0, 0, 0],
  },
  {
    title: "Ahora te toca a ti",
    body: "Se abrió el Laboratorio: pulsa «Empezar vacía», guarda tus propios recuerdos y consúltalos cambiando modo, k y filtros. También puedes hacer clic en cualquier punto para leerlo, o arrastrar para orbitar. Esta colmena es solo tuya.",
    cam: [34, 20, 50],
    target: [0, 0, 0],
    lab: true,
  },
];

export function startTour() {
  useStore.getState().setTour(0);
  void enter(0);
}

export async function goto(step: number) {
  if (step < 0) return;
  if (step >= STEPS.length) return endTour();
  useStore.getState().setTour(step);
  await enter(step);
}

async function enter(step: number) {
  useStore.setState({ lab: !!STEPS[step]?.lab });
  try {
    await STEPS[step]?.run?.();
  } catch {
    // el tour sigue aunque una operación falle (límite de peticiones, etc.)
  }
}

export function endTour() {
  useStore.getState().setTour(null); // el Laboratorio abierto en el último paso se queda abierto
  try {
    localStorage.setItem("colmena-tour", "done");
  } catch {}
}

export function tourSeen(): boolean {
  try {
    return localStorage.getItem("colmena-tour") === "done";
  } catch {
    return false;
  }
}
