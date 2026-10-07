import { describe, expect, it } from "bun:test";
import { HiveDB } from "../src/index.ts";

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
