# Construir desde la raíz del repo:
#   podman build -f apps/colmena/Dockerfile -t colmena .
#   podman run --rm -p 3001:3001 colmena
# Nota: la imagen de la UI es `linux/amd64`/glibc; el binding nativo se compila aquí.

# ---- 1) Binding nativo (Rust → .node) -------------------------------------
FROM rust:1-bookworm AS native
COPY --from=oven/bun:1 /usr/local/bin/bun /usr/local/bin/bun
WORKDIR /repo
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY packages/hive-db ./packages/hive-db
RUN rustc --version
WORKDIR /repo/packages/hive-db
RUN bun install --frozen-lockfile || bun install
RUN bun run build:native

# ---- 2) Frontend (Vite) ---------------------------------------------------
FROM oven/bun:1 AS web
WORKDIR /app/web
COPY apps/colmena/web/package.json apps/colmena/web/bun.lock* ./
RUN bun install --frozen-lockfile || bun install
COPY apps/colmena/web ./
RUN bun run build

# ---- 3) Runtime -----------------------------------------------------------
FROM oven/bun:1-slim
ENV NODE_ENV=production \
    PORT=3001 \
    WEB_DIR=/app/apps/colmena/web/dist \
    MAX_SESSIONS=8 \
    SESSION_IDLE_MIN=15 \
    MAX_DOCS=1500
WORKDIR /app
COPY packages/hive-db/package.json packages/hive-db/
COPY packages/hive-db/src packages/hive-db/src
COPY --from=native /repo/packages/hive-db/native.cjs /repo/packages/hive-db/*.node packages/hive-db/
COPY apps/colmena/package.json apps/colmena/bun.lock* apps/colmena/
WORKDIR /app/apps/colmena
RUN bun install --production
COPY apps/colmena/server ./server
COPY --from=web /app/web/dist ./web/dist
USER bun
EXPOSE 3001
HEALTHCHECK --interval=30s --timeout=3s CMD bun -e "fetch('http://localhost:'+process.env.PORT+'/api/health').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"
CMD ["bun", "server/src/index.ts"]
