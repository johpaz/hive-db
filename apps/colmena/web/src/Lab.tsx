import { useState } from "react";
import { api } from "./api";
import { useStore } from "./store";

type Mode = "hybrid" | "text" | "vector";
const MODES: { id: Mode; label: string; hint: string }[] = [
  { id: "hybrid", label: "Híbrida", hint: "BM25 + HNSW fusionados con RRF" },
  { id: "text", label: "Solo texto", hint: "BM25 (índice invertido)" },
  { id: "vector", label: "Solo vector", hint: "HNSW (coseno)" },
];
const SAMPLE = ["Mi vuelo a Lisboa sale el viernes a las 7:40", "El cliente prefiere facturas en euros", "Bug en el parser de fechas: usar UTC"].join("\n");

export function Lab() {
  const { lab, setLab, agents, lastQuery, docs, docIndex, running, stats } = useStore();
  const [tab, setTab] = useState<"remember" | "query">("remember");
  const [agent, setAgent] = useState("tú");
  const [lines, setLines] = useState("");
  const [text, setText] = useState("");
  const [mode, setMode] = useState<Mode>("hybrid");
  const [k, setK] = useState(5);
  const [only, setOnly] = useState("");
  const [ef, setEf] = useState(200);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setNote(null);
    try {
      await fn();
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const remember = () =>
    run(async () => {
      const items = lines.split("\n").map((l) => l.trim()).filter(Boolean).slice(0, 10);
      for (const t of items) await api("/remember", { body: { agent, text: t.slice(0, 300) } });
      setLines("");
      setNote(`${items.length} recuerdo(s) guardados en tu colmena`);
      if (items.length) setTab("query");
    });

  const query = () =>
    run(async () => {
      if (!text.trim()) return;
      await api("/query", { body: { text, agent: "tú", k, mode, efSearch: ef, only: only || undefined } });
    });

  const reset = (empty: boolean) => run(async () => void (await api("/reset", { body: { empty } })));
  const toggle = () => run(async () => void useStore.setState({ running: (await api<{ running: boolean }>("/sim", { body: { running: !running } })).running }));

  if (!lab) {
    return (
      <button className="lab-btn" onClick={() => setLab(true)}>
        🧪 Laboratorio · prueba tú el motor
      </button>
    );
  }

  const mine = lastQuery && lastQuery.agent === "tú" ? lastQuery : null;
  return (
    <section className="panel lab">
      <h3>
        Laboratorio · tu colmena <button className="x" onClick={() => setLab(false)}>×</button>
      </h3>
      <p className="muted">
        Tu sesión tiene su propia base HiveDB aislada ({stats?.alive ?? 0} recuerdos vivos). Empieza vacía, guarda lo tuyo y consúltalo.
      </p>
      <div className="row">
        <button disabled={busy} onClick={() => reset(true)}>Empezar vacía</button>
        <button className="ghost" disabled={busy} onClick={() => reset(false)}>Datos de ejemplo</button>
        <button className="ghost" disabled={busy} onClick={toggle}>{running ? "⏸ Pausar agentes" : "▶ Reanudar agentes"}</button>
      </div>

      <div className="tabs">
        <button className={tab === "remember" ? "on" : ""} onClick={() => setTab("remember")}>1 · Memorizar</button>
        <button className={tab === "query" ? "on" : ""} onClick={() => setTab("query")}>2 · Consultar</button>
      </div>

      {tab === "remember" ? (
        <div className="form">
          <label>
            Un recuerdo por línea (máx. 10)
            <textarea rows={5} value={lines} onChange={(e) => setLines(e.target.value)} placeholder={SAMPLE} />
          </label>
          <label>
            Agente que lo guarda
            <select value={agent} onChange={(e) => setAgent(e.target.value)}>
              <option value="tú">tú</option>
              {agents.map((a) => (
                <option key={a.id} value={a.id}>{a.id}</option>
              ))}
            </select>
          </label>
          <div className="row">
            <button disabled={busy || !lines.trim()} onClick={remember}>Guardar en HiveDB</button>
            <button className="ghost" disabled={busy} onClick={() => setLines(SAMPLE)}>Usar ejemplo</button>
          </div>
          <p className="muted">Cada línea ejecuta upsert_doc + append (evento Fact) y aparece como un punto blanco en el espacio.</p>
        </div>
      ) : (
        <div className="form">
          <label>
            Consulta
            <input value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && query()} placeholder="p. ej. vuelo lisboa · facturas · bug fechas" />
          </label>
          <div className="grid">
            <label>
              Modo
              <select value={mode} onChange={(e) => setMode(e.target.value as Mode)}>
                {MODES.map((m) => (
                  <option key={m.id} value={m.id}>{m.label}</option>
                ))}
              </select>
            </label>
            <label>
              k (resultados)
              <input type="number" min={1} max={20} value={k} onChange={(e) => setK(Math.min(20, Math.max(1, +e.target.value || 1)))} />
            </label>
            <label>
              Solo del agente
              <select value={only} onChange={(e) => setOnly(e.target.value)}>
                <option value="">todos</option>
                <option value="tú">tú</option>
                {agents.map((a) => (
                  <option key={a.id} value={a.id}>{a.id}</option>
                ))}
              </select>
            </label>
            <label>
              efSearch
              <input type="number" min={1} max={512} value={ef} onChange={(e) => setEf(Math.min(512, Math.max(1, +e.target.value || 1)))} />
            </label>
          </div>
          <p className="muted">{MODES.find((m) => m.id === mode)!.hint}. Con filtro por agente la búsqueda vectorial es exacta, sin HNSW.</p>
          <button disabled={busy || !text.trim()} onClick={query}>Consultar</button>

          {mine && (
            <div className="out">
              <h3>{mine.fused.length} resultados · {mine.latencyMs.toFixed(2)} ms</h3>
              {mine.hnsw && <p className="muted">HNSW: {mine.hnsw.total} nodos recorridos · {mine.hnsw.layers} capas</p>}
              <ol>
                {mine.fused.map((h) => {
                  const d = docs[docIndex.get(h.id) ?? -1];
                  return (
                    <li key={h.id}>
                      <small>
                        {h.id} · {d?.agent} · score {h.score.toFixed(3)}
                        {h.textScore != null && ` · bm25 ${h.textScore.toFixed(1)}`}
                        {h.vectorScore != null && ` · cos ${h.vectorScore.toFixed(2)}`}
                      </small>
                      <span onClick={() => useStore.getState().select(h.id)}>{d?.text}</span>
                      {d?.alive && (
                        <button className="mini" disabled={busy} onClick={() => run(async () => void (await api("/invalidate", { body: { id: h.id } })))}>
                          olvidar
                        </button>
                      )}
                    </li>
                  );
                })}
              </ol>
              {mine.fused.length === 0 && <p className="muted">Sin resultados: guarda recuerdos primero o prueba otro modo.</p>}
            </div>
          )}
        </div>
      )}
      {note && <p className="note">{note}</p>}
    </section>
  );
}
