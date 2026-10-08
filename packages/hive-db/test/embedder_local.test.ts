import { describe, expect, it } from "bun:test";
import { copyFileSync, existsSync, linkSync, mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { HiveDB } from "../src/index.ts";

const SRC = fileURLToPath(new URL("../src/index.ts", import.meta.url));
const MODEL_REVISION_DIR = "multilingual-e5-small-614241f6";

/** Ejecuta un script en un proceso aparte con un entorno propio (las variables de entorno
 *  se leen al arrancar el motor nativo, así que no se pueden cambiar dentro de la prueba). */
function runWithEnv(script: string, env: Record<string, string>) {
  const result = Bun.spawnSync([process.execPath, "-e", script], {
    env: { ...process.env, ...env },
  });
  return { out: result.stdout.toString().trim(), err: result.stderr.toString().trim() };
}

// Necesita un binario compilado con la feature `embedder-local` y el modelo
// (~470 MB; HIVEDB_MODEL_DIR reutiliza una caché). Se activa a propósito:
//   HIVEDB_E2E_EMBEDDER=1 bun test test/embedder_local.test.ts
const enabled = process.env.HIVEDB_E2E_EMBEDDER === "1";

describe("embedder local", () => {
  it("rechaza un embedder desconocido", async () => {
    await expect(HiveDB.open(":memory:", { embedder: "otro" as "local" })).rejects.toThrow(
      /unknown embedder/
    );
  });

  it("prepareEmbedder falla con EMBEDDER_UNAVAILABLE sin modelo y sin red", () => {
    const emptyCache = mkdtempSync(join(tmpdir(), "hivedb-model-"));
    const { out } = runWithEnv(
      `import { HiveDB } from ${JSON.stringify(SRC)};
       try { await HiveDB.prepareEmbedder(); console.log("OK"); }
       catch (e) { console.log(e.code ?? "SIN_CODIGO"); }`,
      { HIVEDB_OFFLINE: "1", HIVEDB_MODEL_DIR: emptyCache }
    );
    expect(out).toBe("EMBEDDER_UNAVAILABLE");
  });

  it.skipIf(!enabled)("prepareEmbedder encuentra el modelo en la caché sin descargar", async () => {
    const prepared = await HiveDB.prepareEmbedder();
    expect(prepared.cached).toBe(true);
    expect(prepared.spaceId).toContain("multilingual-e5-small");
    expect(existsSync(join(prepared.dir, "model.safetensors"))).toBe(true);
  });

  it.skipIf(!enabled)(
    "prepareEmbedder avisa del avance y descarga solo lo que falta",
    () => {
      // Caché con el modelo y el tokenizador, pero sin config.json (655 bytes): hay que
      // descargar solo ese archivo, de verdad, y avisar del avance.
      const source = join(process.env.HIVEDB_MODEL_DIR ?? "", MODEL_REVISION_DIR);
      expect(existsSync(join(source, "model.safetensors"))).toBe(true);
      const cache = mkdtempSync(join(tmpdir(), "hivedb-model-"));
      const target = join(cache, MODEL_REVISION_DIR);
      mkdirSync(target, { recursive: true });
      for (const name of ["model.safetensors", "tokenizer.json"]) {
        try {
          linkSync(join(source, name), join(target, name));
        } catch {
          copyFileSync(join(source, name), join(target, name));
        }
      }
      const { out, err } = runWithEnv(
        `import { HiveDB } from ${JSON.stringify(SRC)};
         const events = [];
         const prepared = await HiveDB.prepareEmbedder({ onProgress: (p) => events.push(p) });
         console.log(JSON.stringify({ cached: prepared.cached, events }));`,
        { HIVEDB_MODEL_DIR: cache, HIVEDB_OFFLINE: "0" }
      );
      const result = JSON.parse(out) as {
        cached: boolean;
        events: { file: string; downloaded: number; total: number; fileCount: number }[];
      };
      expect(result.cached, err).toBe(false);
      expect(result.events.length).toBeGreaterThan(0);
      expect(result.events.every((e) => e.file === "config.json" && e.fileCount === 1)).toBe(true);
      expect(result.events.at(-1)?.downloaded).toBe(655);
      expect(result.events.at(-1)?.total).toBe(655);
      expect(existsSync(join(target, "config.json"))).toBe(true);
    },
    120_000
  );

  it.skipIf(!enabled)(
    "encuentra por significado sin que el usuario aporte vectores",
    async () => {
      const db = await HiveDB.open(":memory:", { embedder: "local" });
      try {
        await db.upsertBatch([
          { id: "email", body: "Cómo configurar tu cuenta de email y la bandeja de entrada" },
          { id: "paella", body: "Receta tradicional de paella valenciana" },
          { id: "refund", body: "Refund policy for cancelled orders" },
        ]);

        const correo = await db.queryHybrid({ text: "correo electrónico", k: 1 });
        expect(correo[0]?.id).toBe("email");

        const dinero = await db.queryHybrid({ text: "devolución de dinero", k: 1 });
        expect(dinero[0]?.id).toBe("refund");
      } finally {
        db.close();
      }
    },
    120_000
  );
});
