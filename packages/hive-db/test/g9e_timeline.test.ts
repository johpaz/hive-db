import { expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HiveDB } from "../src/index.ts";

test("G9e timeline > línea por evento y detalle con el texto original", async () => {
  const db = await HiveDB.open(mkdtempSync(join(tmpdir(), "hivedb-tl-")));
  const exact = `${"x".repeat(200)} /srv/app/prod-7731.yaml`;
  await db.append({ agentId: "a", streamId: "t", kind: "StateTransition", payload: JSON.stringify({ description: exact, phase: "plan" }) });
  for (let i = 0; i < 100; i++) {
    await db.append({ agentId: "a", streamId: "t", kind: "StateTransition", payload: JSON.stringify({ description: `paso ${i}`, phase: "impl" }) });
  }
  const ctx = (await db.buildAgentContext({
    taskId: "t", currentPhase: "impl", currentObjective: "prod-7731 yaml", maxTokens: 1500,
    strategy: { timeline: {} },
  })) as { items: Array<{ type: string; text?: string }> };

  expect(ctx.items.some((i) => i.type === "timelineEntry")).toBe(true);
  expect(ctx.items.some((i) => i.type === "decision" && i.text?.includes("/srv/app/prod-7731.yaml"))).toBe(true);
  db.close();
});
