import { Elysia, t } from "elysia";
import { Colmena } from "./colmena";
import { AgentSimulator } from "./sim";

const PORT = Number(process.env.PORT ?? 3001);
const colmena = await Colmena.create(Number(process.env.SEED_DOCS ?? 240));
const sim = new AgentSimulator(colmena);
sim.start();

const sockets = new Set<{ send(data: string): unknown }>();
colmena.on((m) => {
  const s = JSON.stringify(m);
  for (const ws of sockets) ws.send(s);
});

const cors = {
  "access-control-allow-origin": "*",
  "access-control-allow-headers": "content-type",
  "access-control-allow-methods": "GET,POST,OPTIONS",
};

const app = new Elysia()
  .onRequest(({ set }) => {
    Object.assign(set.headers, cors);
  })
  .options("/*", () => new Response(null, { status: 204, headers: cors }))
  .get("/health", () => ({ ok: true }))
  .get("/snapshot", () => colmena.snapshot())
  .post(
    "/query",
    async ({ body }) => {
      const m = await colmena.recall(body.agent ?? "visitante", body.text, {
        k: body.k,
        crossAgent: true,
        efSearch: body.efSearch,
      });
      return m;
    },
    {
      body: t.Object({
        text: t.String({ maxLength: 200 }),
        agent: t.Optional(t.String()),
        k: t.Optional(t.Number({ minimum: 1, maximum: 20 })),
        efSearch: t.Optional(t.Number({ minimum: 1, maximum: 512 })),
      }),
    }
  )
  .post(
    "/remember",
    async ({ body }) => colmena.remember(body.agent, body.text),
    { body: t.Object({ agent: t.String(), text: t.String({ maxLength: 300 }) }) }
  )
  .post("/invalidate", async ({ body }) => ({ seq: await colmena.invalidate(body.id) }), {
    body: t.Object({ id: t.String() }),
  })
  .post("/speed", ({ body }) => ((sim.speed = body.speed), { speed: sim.speed }), {
    body: t.Object({ speed: t.Number({ minimum: 0.1, maximum: 10 }) }),
  })
  .get("/context/:agent", async ({ params }) => colmena.context(params.agent, "demo"))
  .ws("/live", {
    open(ws) {
      sockets.add(ws);
      ws.send(JSON.stringify({ t: "stats", stats: colmena.stats() }));
    },
    close(ws) {
      sockets.delete(ws);
    },
  })
  .listen(PORT);

console.log(`🐝 colmena server en http://localhost:${app.server?.port}`);
const shutdown = () => {
  sim.stop();
  colmena.close();
  process.exit(0);
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
