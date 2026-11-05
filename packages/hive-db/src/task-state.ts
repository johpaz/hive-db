/**
 * Estado de tarea — el traspaso que un agente deja a su siguiente instancia.
 *
 * Un agente que reinicia su contexto con frecuencia (compactación, o una sola
 * acción por contexto) necesita un estado corto, explícito y legible que viaje
 * entre instancias. Seis campos, los de "You don't need long context"
 * (arXiv 2611.05306): objetivo, hechos establecidos, cambios aplicados,
 * verificación, callejones sin salida y plan restante.
 *
 * Es una colección de documentos como cualquier otra (duradera y con versión
 * optimista), no memoria de trabajo: la memoria de trabajo caduca y un estado
 * de tarea debe sobrevivir a un reinicio. Este archivo es puro (sin motor).
 */

export interface TaskState {
  /** Una línea: el resultado que se busca. */
  objective: string;
  /** Hechos verificados, exactos: rutas, ids, valores, errores, preferencias. */
  facts: string[];
  /** Qué se cambió o creó, y si se verificó. */
  changes: string[];
  /** Comprobaciones hechas y su resultado. */
  verification: string[];
  /** Enfoques que fallaron y no deben repetirse. */
  deadEnds: string[];
  /** Subtareas pendientes en orden; la primera es el siguiente paso. */
  plan: string[];
}

/** Un estado guardado, con lo que el motor añade al persistirlo. */
export interface StoredTaskState extends TaskState {
  agentId: string;
  taskId: string;
  /** Cuántas veces se ha guardado este estado (1 la primera). */
  step: number;
  /** Milisegundos desde epoch del último guardado. */
  updatedAt: number;
  /** Versión de la colección: pásala como `expectedVersion` para no pisar a otra instancia. */
  version: number;
}

export interface SaveTaskStateOptions {
  /**
   * Concurrencia optimista: la versión guardada debe coincidir (0 = no debe existir).
   * Omítelo para sobrescribir sin condición.
   */
  expectedVersion?: number;
}

export interface RenderTaskStateOptions {
  /** Tope de caracteres; se recorta la cola, que es lo de menor prioridad. */
  maxChars?: number;
}

/** Documento tal como se guarda en la colección. */
export interface TaskStateDoc extends TaskState {
  agentId: string;
  taskId: string;
  step: number;
  updatedAt: number;
}

export const TASK_STATE_COLLECTION = "task_states";

const MAX_ITEMS = 30;
const MAX_ITEM_CHARS = 500;
const MAX_OBJECTIVE_CHARS = 400;

/** Id de documento sin ambigüedad: dos agentes/tareas distintos nunca colisionan. */
export function taskStateId(agentId: string, taskId: string): string {
  if (!agentId || !taskId) throw new Error("taskState: agentId y taskId son obligatorios");
  return `${encodeURIComponent(agentId)}:${encodeURIComponent(taskId)}`;
}

function clean(value: unknown, max: number): string {
  const text = typeof value === "string" ? value : value == null ? "" : JSON.stringify(value);
  return text.replace(/\s+/g, " ").trim().slice(0, max);
}

function cleanList(value: unknown): string[] {
  const items = Array.isArray(value) ? value : typeof value === "string" && value.trim() ? [value] : [];
  return items
    .map((item) => clean(item, MAX_ITEM_CHARS))
    .filter(Boolean)
    .slice(0, MAX_ITEMS);
}

/**
 * Deja el estado en forma canónica: textos recortados, listas acotadas y sin
 * vacíos. Acepta entradas parciales (lo que falte queda vacío) para que un
 * modelo que omite un campo no rompa el guardado.
 */
export function normalizeTaskState(input: Partial<TaskState> | null | undefined): TaskState {
  const o = (input ?? {}) as Record<string, unknown>;
  return {
    objective: clean(o.objective, MAX_OBJECTIVE_CHARS),
    facts: cleanList(o.facts),
    changes: cleanList(o.changes),
    verification: cleanList(o.verification),
    deadEnds: cleanList(o.deadEnds),
    plan: cleanList(o.plan),
  };
}

/**
 * El estado como texto para un prompt. El orden es el de prioridad: quien
 * recorte la cola pierde primero la verificación y los cambios, y último los
 * hechos y los callejones sin salida.
 */
export function renderTaskState(state: TaskState, options: RenderTaskStateOptions = {}): string {
  const list = (title: string, items: string[]) =>
    items.length ? `${title}:\n${items.map((i) => `- ${i}`).join("\n")}` : "";
  const text = [
    state.objective ? `Objetivo: ${state.objective}` : "",
    list("Hechos establecidos", state.facts),
    list("Callejones sin salida (no repetir)", state.deadEnds),
    list("Plan restante", state.plan),
    list("Cambios aplicados", state.changes),
    list("Verificación", state.verification),
  ]
    .filter(Boolean)
    .join("\n\n");
  return options.maxChars !== undefined && text.length > options.maxChars
    ? text.slice(0, Math.max(0, options.maxChars))
    : text;
}
