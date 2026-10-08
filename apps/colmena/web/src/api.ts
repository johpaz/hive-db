let sid: string | null = null;
let creating: Promise<string> | null = null;

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string
  ) {
    super(message);
  }
}

async function newSession(): Promise<string> {
  const r = await fetch("/api/session", { method: "POST" });
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw new ApiError(r.status, body.error ?? "No se pudo crear la sesión");
  sid = body.sid as string;
  return sid;
}

export function ensureSession(force = false): Promise<string> {
  if (sid && !force) return Promise.resolve(sid);
  creating ??= newSession().finally(() => (creating = null));
  return creating;
}

export function currentSid() {
  return sid;
}

/** fetch con la sesión; si expiró (401) crea otra y reintenta una vez. */
export async function api<T = unknown>(path: string, init: { method?: string; body?: unknown } = {}): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const id = await ensureSession();
    const r = await fetch(`/api${path}`, {
      method: init.method ?? (init.body ? "POST" : "GET"),
      headers: { "x-sid": id, ...(init.body ? { "content-type": "application/json" } : {}) },
      body: init.body ? JSON.stringify(init.body) : undefined,
    });
    if (r.status === 401 && attempt === 0) {
      sid = null;
      await ensureSession(true);
      continue;
    }
    const body = await r.json().catch(() => ({}));
    if (!r.ok) throw new ApiError(r.status, (body as { error?: string }).error ?? `Error ${r.status}`);
    return body as T;
  }
}
