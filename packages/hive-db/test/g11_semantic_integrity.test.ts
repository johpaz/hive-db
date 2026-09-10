import { afterEach, describe, expect, it } from "bun:test";
import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HiveDB, HiveDBError } from "../src/index.ts";

const paths: string[] = [];

function tempDir(): string {
  const path = mkdtempSync(join(tmpdir(), "hive-g11-"));
  paths.push(path);
  return path;
}

afterEach(() => {
  for (const path of paths.splice(0)) rmSync(path, { recursive: true, force: true });
});

describe("G11 semantic index integrity", () => {
  it("requires explicit vector configuration", async () => {
    const db = await HiveDB.open(tempDir());
    try {
      await expect(
        db.upsertDoc({ id: "bad", vector: new Float32Array([1, 0, 0, 0, 0, 0, 0, 0]) })
      ).rejects.toMatchObject({ code: "INVALID_VECTOR" });
    } finally {
      db.close();
    }
  });

  it("rejects zero and non-finite vectors with a stable error code", async () => {
    const db = await HiveDB.open(tempDir(), {
      vector: { dimension: 8, spaceId: "test:8" },
    });
    try {
      for (const vector of [
        new Float32Array(8),
        new Float32Array([Number.NaN, 0, 0, 0, 0, 0, 0, 0]),
        new Float32Array([Number.POSITIVE_INFINITY, 0, 0, 0, 0, 0, 0, 0]),
      ]) {
        try {
          await db.upsertDoc({ id: "bad", vector });
          throw new Error("expected invalid vector error");
        } catch (error) {
          expect(error).toBeInstanceOf(HiveDBError);
          expect((error as HiveDBError).code).toBe("INVALID_VECTOR");
        }
      }
    } finally {
      db.close();
    }
  });

  it("persists the vector space and exposes manual compaction", async () => {
    const path = tempDir();
    const db = await HiveDB.open(path, {
      vector: { dimension: 8, spaceId: "model-a:8" },
    });
    const vector = new Float32Array([1, 0, 0, 0, 0, 0, 0, 0]);
    await db.upsertDoc({ id: "doc", body: "persistente", vector });
    await db.compactIndex();
    db.close();

    await expect(
      HiveDB.open(path, { vector: { dimension: 8, spaceId: "model-b:8" } })
    ).rejects.toMatchObject({ code: "VECTOR_SPACE_MISMATCH" });

    const reopened = await HiveDB.open(path, {
      vector: { dimension: 8, spaceId: "model-a:8" },
    });
    try {
      const hits = await reopened.queryHybrid({ vector, k: 1 });
      expect(hits[0].id).toBe("doc");
    } finally {
      reopened.close();
    }
  });

  it("opens a database created by 0.3.x without options", async () => {
    // Lo que hacen hive y hive-sdk al arrancar sobre una instalación existente.
    const path = tempDir();
    cpSync(join(import.meta.dir, "../../../crates/hivedb-index/tests/fixtures/v0.3.1"), path, {
      recursive: true,
    });

    const db = await HiveDB.open(path);
    try {
      await db.upsertDoc({ id: "tool:web_search", name: "web_search", body: "buscar en la web" });
      const hits = await db.queryHybrid({ text: "buscar", k: 1 });
      expect(hits[0].id).toBe("tool:web_search");
    } finally {
      db.close();
    }

    const meta = JSON.parse(readFileSync(join(path, "meta.json"), "utf8"));
    expect(meta).toEqual({
      schema_version: 2,
      metric: "cosine",
      vector: null,
      vector_dimension: 384,
    });
  });
});
