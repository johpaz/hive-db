import type { Colmena } from "./colmena";
import { AGENTS, TOPICS, makeFact, pick, rng } from "./dataset";

/** Agentes de guion que usan la base real. Cada uno recuerda, consulta y corrige. */
export class AgentSimulator {
  private timers: ReturnType<typeof setTimeout>[] = [];
  private running = false;
  private r = rng(Date.now() & 0xffff);
  speed = 1;
  maxDocs = Number(process.env.MAX_DOCS ?? 1500);

  constructor(private c: Colmena) {}

  start() {
    if (this.running) return;
    this.running = true;
    for (const a of AGENTS) this.loop(a.id);
    const tick = () => {
      if (!this.running) return;
      this.c.emitStats();
      this.timers.push(setTimeout(tick, 1000));
    };
    tick();
  }

  stop() {
    this.running = false;
    for (const t of this.timers) clearTimeout(t);
    this.timers = [];
  }

  private loop(agentId: string) {
    if (!this.running) return;
    const delay = (700 + this.r() * 1800) / this.speed;
    this.timers.push(
      setTimeout(async () => {
        try {
          await this.step(agentId);
        } catch (e) {
          console.error("[sim]", agentId, e);
        }
        this.loop(agentId);
      }, delay)
    );
  }

  private ownAlive(agentId: string) {
    return [...this.c.docs.values()].filter((d) => d.agent === agentId && d.alive);
  }

  async step(agentId: string) {
    if (this.c.docs.size >= this.maxDocs) return;
    const agent = AGENTS.find((a) => a.id === agentId)!;
    const topicId = pick(this.r, agent.topics);
    const topic = TOPICS.find((t) => t.id === topicId)!;
    const roll = this.r();
    if (roll < 0.4) {
      await this.c.remember(agentId, makeFact(this.r, topic));
    } else if (roll < 0.8) {
      const q = [pick(this.r, topic.words), pick(this.r, topic.words), pick(this.r, topic.words)].join(" ");
      await this.c.recall(agentId, q);
    } else if (roll < 0.9) {
      const mine = this.ownAlive(agentId);
      if (mine.length > 20) {
        const old = pick(this.r, mine);
        await this.c.invalidate(old.id);
        await this.c.remember(agentId, makeFact(this.r, TOPICS.find((t) => t.id === old.topic) ?? topic), old.seq);
      }
    } else {
      const other = pick(this.r, AGENTS.filter((a) => a.id !== agentId));
      const allowed = await this.c.gate(agentId, "read", `memory:${other.id}`);
      if (allowed) await this.c.recall(agentId, pick(this.r, topic.words), { crossAgent: true });
    }
  }
}
