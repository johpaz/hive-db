import { test, expect } from "bun:test";
import { HiveDB, normalizeTaskState, renderTaskState, type TaskState } from "../src";

function tmpDir(): string {
  return `/tmp/hivedb-taskstate-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

const state: TaskState = {
  objective: "Migrar la facturación a Alegra",
  facts: ["API base: https://api.alegra.com/v1", "El usuario prefiere respuestas cortas"],
  changes: ["Creado src/billing/alegra.ts (verificado)"],
  verification: ["bun test billing: 12 pass"],
  deadEnds: ["Basic auth falla con 401; usar token"],
  plan: ["Mapear impuestos", "Probar nota crédito"],
};

test("task state > round-trips and reports step, version and time", async () => {
  const db = await HiveDB.open(":memory:");
  const saved = await db.saveTaskState("agent-a", "task-1", state);
  expect(saved.step).toBe(1);
  expect(saved.version).toBe(1);

  const loaded = await db.loadTaskState("agent-a", "task-1");
  expect(loaded).toEqual(saved);
  expect(loaded?.updatedAt).toBeGreaterThan(0);
  db.close();
});

test("task state > each save replaces the previous state and bumps step", async () => {
  const db = await HiveDB.open(":memory:");
  await db.saveTaskState("agent-a", "task-1", state);
  const second = await db.saveTaskState("agent-a", "task-1", { objective: "Otro", plan: ["Uno"] });

  expect(second.step).toBe(2);
  expect(second.version).toBe(2);
  expect(second.facts).toEqual([]);
  expect((await db.loadTaskState("agent-a", "task-1"))?.objective).toBe("Otro");
  db.close();
});

test("task state > expectedVersion refuses to overwrite a newer save", async () => {
  const db = await HiveDB.open(":memory:");
  const first = await db.saveTaskState("agent-a", "task-1", state, { expectedVersion: 0 });
  await db.saveTaskState("agent-a", "task-1", { ...state, objective: "de otra instancia" });

  await expect(
    db.saveTaskState("agent-a", "task-1", state, { expectedVersion: first.version })
  ).rejects.toThrow();
  expect((await db.loadTaskState("agent-a", "task-1"))?.objective).toBe("de otra instancia");
  db.close();
});

test("task state > survives reopening the database", async () => {
  const dir = tmpDir();
  const db = await HiveDB.open(dir);
  await db.saveTaskState("agent-a", "task-1", state);
  db.close();

  const reopened = await HiveDB.open(dir);
  expect((await reopened.loadTaskState("agent-a", "task-1"))?.deadEnds).toEqual(state.deadEnds);
  reopened.close();
});

test("task state > agents and tasks are isolated, even with ':' in the ids", async () => {
  const db = await HiveDB.open(":memory:");
  await db.saveTaskState("a:b", "c", { objective: "uno" });
  await db.saveTaskState("a", "b:c", { objective: "dos" });

  expect((await db.loadTaskState("a:b", "c"))?.objective).toBe("uno");
  expect((await db.loadTaskState("a", "b:c"))?.objective).toBe("dos");
  expect(await db.loadTaskState("a", "c")).toBeUndefined();
  db.close();
});

test("task state > delete removes it and reports whether it existed", async () => {
  const db = await HiveDB.open(":memory:");
  await db.saveTaskState("agent-a", "task-1", state);
  expect(await db.deleteTaskState("agent-a", "task-1")).toBe(true);
  expect(await db.deleteTaskState("agent-a", "task-1")).toBe(false);
  expect(await db.loadTaskState("agent-a", "task-1")).toBeUndefined();
  db.close();
});

test("task state > does not write to the event log", async () => {
  const db = await HiveDB.open(":memory:");
  await db.saveTaskState("agent-a", "task-1", state);
  expect(await db.logLen()).toBe(0);
  db.close();
});

test("task state > requires agentId and taskId", async () => {
  const db = await HiveDB.open(":memory:");
  await expect(db.saveTaskState("", "task-1", state)).rejects.toThrow();
  await expect(db.loadTaskState("agent-a", "")).rejects.toThrow();
  db.close();
});

test("normalizeTaskState > trims, drops empties and caps sizes; tolerates partial input", () => {
  const n = normalizeTaskState({
    objective: "  o  ".repeat(300),
    facts: ["  a   b  ", "", "x".repeat(2000), ...Array.from({ length: 50 }, (_, i) => `h${i}`)],
  });
  expect(n.objective.length).toBeLessThanOrEqual(400);
  expect(n.facts[0]).toBe("a b");
  expect(n.facts[1]!.length).toBe(500);
  expect(n.facts.length).toBe(30);
  expect(n.plan).toEqual([]);
  expect(normalizeTaskState(null).objective).toBe("");
});

test("renderTaskState > orders by priority and keeps facts and dead ends when truncated", () => {
  const text = renderTaskState(state);
  const at = (s: string) => text.indexOf(s);
  expect(at("Hechos establecidos")).toBeLessThan(at("Callejones sin salida"));
  expect(at("Callejones sin salida")).toBeLessThan(at("Plan restante"));
  expect(at("Plan restante")).toBeLessThan(at("Cambios aplicados"));
  expect(at("Cambios aplicados")).toBeLessThan(at("Verificación"));

  const cut = renderTaskState(state, { maxChars: 220 });
  expect(cut.length).toBeLessThanOrEqual(220);
  expect(cut).toContain("API base");
  expect(cut).toContain("Callejones");
});

test("renderTaskState > omits empty sections", () => {
  const text = renderTaskState(normalizeTaskState({ objective: "x", plan: ["a"] }));
  expect(text).toBe("Objetivo: x\n\nPlan restante:\n- a");
});
