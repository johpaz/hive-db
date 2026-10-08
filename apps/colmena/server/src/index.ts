import { existsSync } from "node:fs";
import { join, normalize } from "node:path";
import { Elysia, t } from "elysia";
import { LIMITS, RateLimiter, SessionFull, SessionManager, type Session } from "./sessions";

const PORT = Number(process.env.PORT ?? 3001);
const WEB_DIR = process.env.WEB_DIR ?? join(import.meta.dir, "../../web/dist");
const sessions = new SessionManager();
const writes = new RateLimiter(Number(process.env.WRITES_PER_MIN ?? 40), 60_000);
const creates = new RateLimiter(Number(process.env.SESSIONS_PER_MIN ?? 6), 60_000);

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
  ".txt": "text/plain; charset=utf-8",
  ".xml": "application/xml; charset=utf-8",
};

function clientIp(req: Request, fallback?: string): string {
  return req.headers.get("x-forwarded-for")?.split(",")[0]?.trim() || fallback || "local";
}

function fail(set: { status?: number | string }, status: number, error: string) {
  set.status = status;
  return { error };
}

const app = new Elysia()
  .derive(({ request }) => ({
    session: sessions.get(request.headers.get("x-sid")) as Session | undefined,
  }))
  .get("/api/health", () => ({ ok: true, sessions: sessions.size, limits: LIMITS }))
  .post("/api/session", async ({ request, server, set }) => {
    const ip = clientIp(request, server?.requestIP(request)?.address);
    if (!creates.allow(ip)) return fail(set, 429, "Demasiadas sesiones nuevas, espera un minuto.");
    try {
      const s = await sessions.create();
      return { sid: s.id, idleMinutes: LIMITS.idleMs / 60_000 };
    } catch (e) {
      if (e instanceof SessionFull) return fail(set, 503, "La colmena está llena, vuelve en unos minutos.");
      throw e;
    }
  })
  .delete("/api/session", ({ session }) => ({ ok: session ? sessions.destroy(session.id) : false }))
  .ws("/live", {
    open(ws) {
      const sid = new URL(ws.data.request.url).searchParams.get("sid");
      const s = sessions.get(sid);
      if (!s) return void ws.close();
      (ws.data as { _s?: Session })._s = s;
      s.attach(ws);
    },
    close(ws) {
      (ws.data as { _s?: Session })._s?.detach(ws);
    },
  })
  .guard(
    {
      beforeHandle: ({ request, set }) =>
        sessions.get(request.headers.get("x-sid")) ? undefined : fail(set, 401, "Sesión inexistente o expirada."),
    },
    (app) =>
      app
        .get("/api/snapshot", ({ session }) => ({ ...session!.colmena.snapshot(), running: !session!.userPaused }))
        .guard(
          {
            beforeHandle: ({ request, server, set }) =>
              writes.allow(clientIp(request, server?.requestIP(request)?.address))
                ? undefined
                : fail(set, 429, "Límite de peticiones, espera un momento."),
          },
          (w) =>
            w
              .post(
                "/api/query",
                async ({ session, body }) => {
                  const r = await session!.colmena.recall(body.agent ?? "tú", body.text, {
                    k: body.k,
                    crossAgent: true,
                    efSearch: body.efSearch,
                    mode: body.mode,
                    only: body.only,
                  });
                  session!.colmena.emitStats();
                  return r;
                },
                {
                  body: t.Object({
                    text: t.String({ minLength: 1, maxLength: 200 }),
                    agent: t.Optional(t.String({ maxLength: 40 })),
                    k: t.Optional(t.Number({ minimum: 1, maximum: 20 })),
                    efSearch: t.Optional(t.Number({ minimum: 1, maximum: 512 })),
                    mode: t.Optional(t.Union([t.Literal("hybrid"), t.Literal("text"), t.Literal("vector")])),
                    only: t.Optional(t.String({ maxLength: 40 })),
                  }),
                },
              )
              .post(
                "/api/remember",
                async ({ session, body, set }) => {
                  try {
                    const d = await session!.colmena.remember(body.agent, body.text, undefined, LIMITS.maxDocs);
                    session!.colmena.emitStats();
                    return d;
                  } catch (e) {
                    return fail(set, 409, e instanceof Error ? e.message : String(e));
                  }
                },
                { body: t.Object({ agent: t.String({ maxLength: 40 }), text: t.String({ minLength: 1, maxLength: 300 }) }) },
              )
              .post("/api/invalidate", async ({ session, body }) => {
                const seq = await session!.colmena.invalidate(body.id);
                session!.colmena.emitStats();
                return { seq };
              }, {
                body: t.Object({ id: t.String({ maxLength: 32 }) }),
              })
              .post(
                "/api/reset",
                async ({ session, body }) => {
                  if (body.empty) session!.userPaused = true; // base limpia: solo tus datos
                  await session!.reset(body.empty ? 0 : LIMITS.seedDocs);
                  session!.colmena.emitStats();
                  return { ok: true };
                },
                { body: t.Object({ empty: t.Boolean() }) },
              )
              .post("/api/sim", ({ session, body }) => (session!.setRunning(body.running), { running: body.running }), {
                body: t.Object({ running: t.Boolean() }),
              })
              .post("/api/speed", ({ session, body }) => ((session!.sim.speed = body.speed), { speed: body.speed }), {
                body: t.Object({ speed: t.Number({ minimum: 0.1, maximum: 10 }) }),
              })
              .get("/api/context/:agent", ({ session, params }) => session!.colmena.context(params.agent, "demo")),
        ),
  )
  // Frontend estático (producción). En desarrollo lo sirve Vite.
  .get("/*", async ({ request, set }) => {
    const path = normalize(decodeURIComponent(new URL(request.url).pathname)).replace(/^(\.\.[/\\])+/, "");
    let file = join(WEB_DIR, path);
    if (!file.startsWith(WEB_DIR) || !existsSync(file) || path === "/" || path.endsWith("/")) {
      // Un recurso con extensión que no existe es un 404 real (evita "soft 404" en buscadores).
      if (/\.[a-z0-9]{2,5}$/i.test(path) && path !== "/") return fail(set, 404, "no encontrado");
      file = join(WEB_DIR, "index.html");
    }
    if (!existsSync(file)) return fail(set, 404, "frontend no construido (cd web && bun run build)");
    const ext = file.slice(file.lastIndexOf("."));
    return new Response(Bun.file(file), {
      headers: {
        "content-type": MIME[ext] ?? "application/octet-stream",
        "cache-control": file.includes("/assets/") ? "public, max-age=31536000, immutable" : "no-cache",
      },
    });
  })
  .listen({ port: PORT, hostname: process.env.HOST ?? "0.0.0.0" });

console.log(
  `🐝 colmena en http://localhost:${app.server?.port} · máx ${LIMITS.maxSessions} sesiones, ${LIMITS.idleMs / 60000} min de inactividad`,
);
const shutdown = () => {
  sessions.closeAll();
  process.exit(0);
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
