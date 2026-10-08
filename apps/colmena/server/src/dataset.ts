export interface Topic {
  id: string;
  label: string;
  color: string;
  words: string[];
  templates: string[];
}

export const TOPICS: Topic[] = [
  {
    id: "viajes",
    label: "Viajes",
    color: "#ffb347",
    words: ["vuelo", "hotel", "playa", "pasaporte", "maleta", "tren", "ruta", "mapa", "reserva", "isla", "aeropuerto", "turismo", "montaña", "hostal", "itinerario"],
    templates: [
      "El usuario prefiere {a} con {b} cerca de la {c}",
      "Reservó un {a} y guardó el {b} para el {c}",
      "Quiere un {a} barato, sin {b}, y buen {c}",
      "Anotó que el {a} sale temprano y el {b} está lejos del {c}",
    ],
  },
  {
    id: "cocina",
    label: "Cocina",
    color: "#ff7a59",
    words: ["receta", "horno", "harina", "tomate", "sartén", "ajo", "cena", "postre", "sopa", "vegano", "especias", "cuchillo", "marinar", "hervir", "ensalada"],
    templates: [
      "A la usuaria le gusta la {a} con {b} y poca {c}",
      "Alergia confirmada: evitar {a}, usar {b} en la {c}",
      "Prefiere cocinar {a} al {b}; la {c} sale mejor así",
      "Guardó la {a} de {b} para la {c} del domingo",
    ],
  },
  {
    id: "codigo",
    label: "Código",
    color: "#5ad1ff",
    words: ["rust", "compilador", "función", "bug", "memoria", "hilo", "índice", "test", "commit", "refactor", "latencia", "servidor", "paquete", "librería", "benchmark"],
    templates: [
      "El proyecto usa {a} y el {b} falla en el {c}",
      "Se decidió {a} el {b} antes de tocar el {c}",
      "Pendiente: medir la {a} del {b} con otro {c}",
      "Error recurrente: {a} sin {b} rompe el {c}",
    ],
  },
  {
    id: "salud",
    label: "Salud",
    color: "#7bf0a5",
    words: ["sueño", "dieta", "caminata", "pulso", "vitamina", "médico", "dolor", "hidratación", "estrés", "rutina", "gimnasio", "descanso", "análisis", "postura", "meditación"],
    templates: [
      "El usuario mejora el {a} con {b} y menos {c}",
      "Recordatorio: cita con el {a} para el {b} y el {c}",
      "Registró {a} bajo; probar {b} antes del {c}",
      "Meta semanal: más {a}, mejor {b} y control del {c}",
    ],
  },
  {
    id: "finanzas",
    label: "Finanzas",
    color: "#c792ff",
    words: ["presupuesto", "factura", "ahorro", "inversión", "impuesto", "gasto", "deuda", "interés", "cuenta", "tarjeta", "nómina", "balance", "contrato", "cobro", "reembolso"],
    templates: [
      "El cliente pidió revisar el {a} y la {b} del {c}",
      "Pendiente de pago: {a} con {b} y su {c}",
      "Se acordó reducir el {a} y subir el {b} del {c}",
      "Alerta: {a} duplicado en la {b} del {c}",
    ],
  },
  {
    id: "musica",
    label: "Música",
    color: "#ff6fb5",
    words: ["guitarra", "concierto", "playlist", "ritmo", "álbum", "melodía", "acorde", "batería", "vinilo", "festival", "letra", "estudio", "piano", "banda", "mezcla"],
    templates: [
      "Al usuario le encanta la {a} y el {b} del {c}",
      "Guardó una {a} para el {b} con {c} suave",
      "Quiere aprender {a} y ensayar el {b} y la {c}",
      "Compró entradas: {a} en el {b}, después del {c}",
    ],
  },
];

export interface AgentSpec {
  id: string;
  label: string;
  color: string;
  topics: string[];
}

export const AGENTS: AgentSpec[] = [
  { id: "ada", label: "Ada · asistente personal", color: "#ffd166", topics: ["viajes", "cocina", "salud"] },
  { id: "bruno", label: "Bruno · copiloto de código", color: "#4cc9f0", topics: ["codigo", "finanzas"] },
  { id: "cleo", label: "Cleo · gestora financiera", color: "#b794f6", topics: ["finanzas", "viajes"] },
  { id: "dante", label: "Dante · curador musical", color: "#ff7eb6", topics: ["musica", "salud", "cocina"] },
];

export function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function pick<T>(r: () => number, xs: T[]): T {
  return xs[Math.floor(r() * xs.length)]!;
}

export function makeFact(r: () => number, topic: Topic): string {
  const t = pick(r, topic.templates);
  return t.replace(/\{[abc]\}/g, () => pick(r, topic.words));
}
