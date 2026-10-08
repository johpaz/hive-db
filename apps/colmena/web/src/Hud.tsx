import { useState } from "react";
import { api } from "./api";
import { useStore } from "./store";
import { Lab } from "./Lab";
import { causalChain } from "./scene/EventHelix";

const KIND_ICON: Record<string, string> = { event: "◆", query: "◎", gate: "⛨", invalidate: "✕" };

export function Hud() {
  const { tour, error, stats, connected, feed, topics, agents, selected, docs, docIndex, lastQuery, events, selectedEvent, lab } = useStore();
  const chain = selectedEvent != null ? events.filter((e) => causalChain(events, selectedEvent).has(e.seq)) : [];
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const doc = selected ? docs[docIndex.get(selected) ?? -1] : undefined;

  const ask = async (text: string) => {
    if (!text.trim() || busy) return;
    setBusy(true);
    try {
      await api("/query", { body: { text, agent: "tú", k: 6 } });
    } catch (e) {
      setMsg(e instanceof Error ? e.message : String(e));
      setTimeout(() => setMsg(null), 4000);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="hud">
      <header className="title">
        <h1><img className="logo" src="/icon-192.png" alt="Logo de Hive" width={34} height={34} />LA COLMENA</h1>
        <p>HiveDB · motor de memoria para agentes · en vivo</p>
        <span className={connected ? "dot on" : "dot"}>{connected ? "conectado" : "reconectando…"}</span>
      </header>

      <section className="panel stats">
        <div><b>{stats?.alive ?? "–"}</b><span>recuerdos vivos</span></div>
        <div><b>{stats?.events ?? "–"}</b><span>eventos (log)</span></div>
        <div><b>{stats?.queries ?? "–"}</b><span>consultas</span></div>
        <div><b>{stats ? stats.p50.toFixed(2) : "–"}<i>ms</i></b><span>latencia p50</span></div>
        <div><b>{stats ? stats.p99.toFixed(2) : "–"}<i>ms</i></b><span>latencia p99</span></div>
      </section>

      <section className="panel legend">
        <h3>Espacio vectorial</h3>
        {topics.map((t) => (
          <p key={t.id}><i style={{ background: t.color }} />{t.label}</p>
        ))}
        <h3>Búsqueda híbrida</h3>
        <p><i style={{ background: "#4de2ff" }} />BM25 (texto)</p>
        <p><i style={{ background: "#ffb020" }} />HNSW (vector)</p>
        <p><i style={{ background: "#fff" }} />RRF (fusión) → top-k</p>
      </section>

      <Lab />

      <section className={"panel feed" + (lab ? " off" : "")}>
        <h3>Actividad real del motor</h3>
        <ul>
          {feed.slice(0, 14).map((f) => {
            const a = agents.find((x) => x.id === f.agent);
            return (
              <li key={f.id} className={f.kind + (f.ok === false ? " no" : "")}>
                <em style={{ color: a?.color ?? "#fff" }}>{KIND_ICON[f.kind]} {f.agent}</em> {f.text}
              </li>
            );
          })}
        </ul>
      </section>

      {lastQuery && !lab && (
        <section className="panel result">
          <h3>Última consulta · “{lastQuery.text}”</h3>
          {lastQuery.hnsw && (
            <p className="muted">
              HNSW real: {lastQuery.hnsw.total} nodos alcanzados · {lastQuery.hnsw.layers} capas · términos BM25: {lastQuery.terms.join(", ")}
            </p>
          )}
          <ol>
            {lastQuery.fused.slice(0, 4).map((h) => {
              const d = docs[docIndex.get(h.id) ?? -1];
              return (
                <li key={h.id} onClick={() => useStore.getState().select(h.id)}>
                  <small>
                    rrf {h.score.toFixed(3)}
                    {h.textScore != null && ` · bm25 ${h.textScore.toFixed(1)}`}
                    {h.vectorScore != null && ` · cos ${h.vectorScore.toFixed(2)}`}
                  </small>
                  {d?.text}
                </li>
              );
            })}
          </ol>
        </section>
      )}

      {doc && (
        <section className="panel detail">
          <h3>{doc.id} · {doc.agent} · seq {doc.seq}</h3>
          <p>{doc.text}</p>
          <p className="muted">tema: {doc.topic} · {doc.alive ? "vivo" : "invalidado"}</p>
          {doc.alive && (
            <button
              onClick={() => api("/invalidate", { body: { id: doc.id } }).catch((e) => setMsg(String(e.message ?? e)))}
            >
              Olvidar este recuerdo
            </button>
          )}
        </section>
      )}

      {selectedEvent != null && (
        <section className="panel causal">
          <h3>Hilo causal · seq {selectedEvent} <button className="x" onClick={() => useStore.getState().selectEvent(null)}>×</button></h3>
          <ol>
            {chain.map((e) => (
              <li key={e.seq}><small>#{e.seq} · {e.agent} · {e.kind}</small>{e.label}</li>
            ))}
          </ol>
          {chain.length < 2 && <p className="muted">Sin causa previa visible. Prueba un evento tras “olvidar” (se enlaza a su Fact original).</p>}
        </section>
      )}

      {tour === null && <p className="hint">Clic en una esfera de la hélice → su cadena causal · clic en un punto → su recuerdo · esferas junto a cada agente = working memory (TTL 30 s)</p>}

      {(error || msg) && <div className="toast">{error ?? msg}</div>}

      <form className="ask" onSubmit={(e) => (e.preventDefault(), ask(q))}>
        <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Pregúntale a la colmena…  prueba: vuelo playa · rust bug · receta ajo" />
        <button disabled={busy}>Consultar</button>
      </form>
    </div>
  );
}
