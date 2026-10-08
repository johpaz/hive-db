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
}

const firstOf = (agent: string) => useStore.getState().docs.find((d) => d.agent === agent && d.alive);

export const STEPS: TourStep[] = [
  {
    title: "Una memoria que puedes ver",
    body: "Cada punto es un recuerdo real guardado en HiveDB. Su posición viene de su vector (proyectado a 3D): los recuerdos parecidos quedan cerca y forman nubes por tema.",
    cam: [34, 20, 50],
    target: [0, 0, 0],
  },
  {
    title: "Agentes que usan el motor",
    body: "Estos cuatro agentes escriben y consultan la base de verdad. Cada destello es una operación: un rayo que nace en el agente es un recuerdo nuevo guardándose.",
    cam: [26, 12, 26],
    target: [8, 3, 8],
  },
  {
    title: "El event-log, inmutable",
    body: "A la izquierda, la hélice es el log append-only: cada esfera es un evento (ámbar = hecho, cian = consulta, rojo = olvido). Las líneas son enlaces de causalidad reales; haz clic en una esfera para ver su cadena.",
    cam: [-12, 4, 22],
    target: [-32, 0, 0],
  },
  {
    title: "Una búsqueda híbrida, paso a paso",
    body: "Lanzamos la consulta «vuelo hotel playa». Primero baja por las capas del HNSW (ruta real del grafo), luego el carril de texto BM25 en cian, y al final la fusión RRF ilumina en blanco los mejores resultados.",
    cam: [20, 10, 30],
    target: [-6, -1, 0],
    run: () => api("/query", { body: { text: "vuelo hotel playa", agent: "tú", k: 6 } }),
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
    body: "Cuando un agente intenta leer la memoria de otro, el motor consulta su grafo de consentimiento. Verás anillos verdes (permitido) o rojos (denegado) alrededor del agente en la actividad.",
    cam: [30, 14, 34],
    target: [0, 2, 0],
  },
  {
    title: "Ahora te toca a ti",
    body: "Escribe una consulta abajo (prueba «rust bug» o «receta ajo»), haz clic en cualquier punto para leer su recuerdo, o arrastra para orbitar. Esta colmena es solo tuya.",
    cam: [34, 20, 50],
    target: [0, 0, 0],
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
  try {
    await STEPS[step]?.run?.();
  } catch {
    // el tour sigue aunque una operación falle (límite de peticiones, etc.)
  }
}

export function endTour() {
  useStore.getState().setTour(null);
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
