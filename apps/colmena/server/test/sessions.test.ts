import { afterEach, describe, expect, it } from "bun:test";
import { RateLimiter, SessionFull, SessionManager } from "../src/sessions";

const limits = { maxSessions: 2, idleMs: 50, maxDocs: 100, seedDocs: 30 };
let mgr: SessionManager;
afterEach(() => mgr?.closeAll());

describe("SessionManager", () => {
  it("aísla las bases: lo que escribe una sesión no lo ve la otra", async () => {
    mgr = new SessionManager(limits);
    const a = await mgr.create();
    const b = await mgr.create();
    const doc = await a.colmena.remember("ada", "recuerdo secreto zanahoria cuántica");
    const inA = await a.colmena.recall("ada", "zanahoria cuántica", { crossAgent: true });
    const inB = await b.colmena.recall("ada", "zanahoria cuántica", { crossAgent: true });
    expect(inA.fused.map((h) => h.id)).toContain(doc.id);
    const textsInB = inB.fused.map((h) => b.colmena.docs.get(h.id)?.text ?? "");
    expect(textsInB.some((t) => t.includes("zanahoria"))).toBe(false);
    expect([...b.colmena.docs.values()].some((d) => d.text.includes("zanahoria"))).toBe(false);
  });

  it("rechaza sesiones por encima del tope y las libera al destruir", async () => {
    mgr = new SessionManager(limits);
    const a = await mgr.create();
    await mgr.create();
    await expect(mgr.create()).rejects.toBeInstanceOf(SessionFull);
    mgr.destroy(a.id);
    expect(mgr.size).toBe(1);
    await mgr.create();
  });

  it("expira las sesiones inactivas pero no las que tienen una pestaña abierta", async () => {
    mgr = new SessionManager(limits);
    const idle = await mgr.create();
    const open = await mgr.create();
    open.sockets.add({ send() {} });
    await Bun.sleep(80);
    const next = await mgr.create().catch(() => null); // dispara el barrido
    expect(mgr.get(idle.id)).toBeUndefined();
    expect(mgr.get(open.id)).toBeDefined();
    expect(next).not.toBeNull();
    open.sockets.clear();
  });

  it("respeta el tope de recuerdos", async () => {
    mgr = new SessionManager(limits);
    const s = await mgr.create();
    await expect(s.colmena.remember("ada", "otro", undefined, s.colmena.docs.size)).rejects.toThrow("LIMIT");
  });
});

describe("RateLimiter", () => {
  it("permite max peticiones por ventana y por clave", async () => {
    const r = new RateLimiter(3, 60);
    expect([1, 2, 3, 4].map(() => r.allow("ip1"))).toEqual([true, true, true, false]);
    expect(r.allow("ip2")).toBe(true);
    await Bun.sleep(70);
    expect(r.allow("ip1")).toBe(true);
  });
});
