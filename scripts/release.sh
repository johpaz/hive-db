#!/usr/bin/env bash
# Publica una versión nueva de HiveDB: sube la versión, actualiza el CHANGELOG,
# hace el commit, crea el tag y lo sube. El tag `vX.Y.Z` dispara el job `publish` de
# .github/workflows/ci.yml, que compila los seis binarios, pasa los tests y publica en
# npm y en GitHub Releases.
#
# Uso:
#   scripts/release.sh 0.6.0                # versión explícita
#   scripts/release.sh patch|minor|major    # sube desde la versión actual
#   scripts/release.sh 0.6.0 --dry-run      # muestra qué haría, sin tocar nada
#
# Opciones:
#   --dry-run     No modifica nada: solo comprueba y enseña los pasos.
#   --yes         No pide confirmación antes de crear el commit, el tag y subirlos.
#   --no-checks   Salta fmt / clippy / tests locales (el CI los repite igualmente).
#   --no-push     Crea el commit y el tag en local, pero no los sube.
#   --allow-dirty Permite árbol de trabajo con cambios (por defecto se exige limpio).
#   --branch X    Rama desde la que se publica (por defecto: main).
#
# Requisitos: git, cargo, node, y el secret NPM_TOKEN configurado en el repositorio de
# GitHub (Settings → Secrets and variables → Actions).

set -euo pipefail

cd "$(dirname "$0")/.."

# ---- argumentos -------------------------------------------------------------------
VERSION_ARG=""
DRY_RUN=0 YES=0 CHECKS=1 PUSH=1 ALLOW_DIRTY=0 BRANCH="main"
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --yes | -y) YES=1 ;;
    --no-checks) CHECKS=0 ;;
    --no-push) PUSH=0 ;;
    --allow-dirty) ALLOW_DIRTY=1 ;;
    --branch) shift; BRANCH="${1:?falta el nombre de la rama}" ;;
    -h | --help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "Opción desconocida: $1" >&2; exit 2 ;;
    *) [ -z "$VERSION_ARG" ] && VERSION_ARG="$1" || { echo "Sobra el argumento: $1" >&2; exit 2; } ;;
  esac
  shift
done
[ -n "$VERSION_ARG" ] || { echo "Falta la versión. Uso: scripts/release.sh 0.6.0 | patch | minor | major" >&2; exit 2; }

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
run() { if [ "$DRY_RUN" = 1 ]; then printf '   [dry-run] %s\n' "$*"; else "$@"; fi; }

# ---- versión actual y nueva -------------------------------------------------------
CURRENT=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' Cargo.toml | head -1)
[ -n "$CURRENT" ] || die "no encuentro la versión en [workspace.package] de Cargo.toml"
PKG_VERSION=$(node -p "require('./packages/hive-db/package.json').version")
[ "$CURRENT" = "$PKG_VERSION" ] ||
  die "Cargo.toml ($CURRENT) y packages/hive-db/package.json ($PKG_VERSION) no coinciden; alinéalas primero"

case "$VERSION_ARG" in
  major | minor | patch)
    IFS=. read -r MA MI PA <<<"$CURRENT"
    case "$VERSION_ARG" in
      major) NEW="$((MA + 1)).0.0" ;;
      minor) NEW="$MA.$((MI + 1)).0" ;;
      patch) NEW="$MA.$MI.$((PA + 1))" ;;
    esac ;;
  *) NEW="${VERSION_ARG#v}" ;;
esac
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "versión no válida: '$NEW' (esperaba X.Y.Z)"
[ "$NEW" != "$CURRENT" ] || die "la versión nueva coincide con la actual ($CURRENT)"
[ "$(printf '%s\n%s\n' "$CURRENT" "$NEW" | sort -V | tail -1)" = "$NEW" ] ||
  die "la versión nueva ($NEW) es menor que la actual ($CURRENT)"
TAG="v$NEW"
TODAY=$(date +%Y-%m-%d)

say "Release $CURRENT → $NEW  (tag $TAG)"

# ---- comprobaciones previas -------------------------------------------------------
say "Comprobaciones previas"
[ "$(git rev-parse --abbrev-ref HEAD)" = "$BRANCH" ] ||
  die "estás en '$(git rev-parse --abbrev-ref HEAD)'; se publica desde '$BRANCH' (usa --branch para cambiarlo)"
if [ "$ALLOW_DIRTY" = 0 ] && [ -n "$(git status --porcelain)" ]; then
  git status --short
  die "hay cambios sin commitear (commitéalos o usa --allow-dirty)"
fi
if git remote get-url origin >/dev/null 2>&1; then
  git fetch --quiet origin "$BRANCH" "refs/tags/*:refs/tags/*" 2>/dev/null || true
  if git rev-parse --verify --quiet "origin/$BRANCH" >/dev/null; then
    [ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$BRANCH")" ] ||
      die "tu rama '$BRANCH' no coincide con origin/$BRANCH (haz pull/push antes)"
  fi
fi
git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null && die "el tag $TAG ya existe"
grep -q '^## Sin publicar' CHANGELOG.md ||
  die "CHANGELOG.md no tiene una sección '## Sin publicar' que convertir en la versión $NEW"
echo "   ok: rama $BRANCH al día, árbol limpio, tag $TAG libre"

# ---- comprobaciones locales -------------------------------------------------------
if [ "$CHECKS" = 1 ]; then
  say "fmt, clippy y tests (el CI los repite; --no-checks para saltarlos)"
  run cargo fmt --all -- --check
  run cargo clippy --workspace --all-targets --locked -- -D warnings
  run cargo clippy -p hivedb-napi --all-targets --locked --features embedder-local -- -D warnings
  run cargo test --workspace --locked
fi

# ---- subir la versión -------------------------------------------------------------
say "Actualizando versiones"
bump_files() {
  # Cargo.toml: solo la línea `version` de [workspace.package].
  python3 - "$NEW" <<'PY'
import re, sys
new = sys.argv[1]
src = open("Cargo.toml", encoding="utf-8").read()
out, n = re.subn(r'(\[workspace\.package\][^\[]*?\nversion = ")[^"]+(")', rf'\g<1>{new}\g<2>', src, count=1, flags=re.S)
assert n == 1, "no se pudo actualizar Cargo.toml"
open("Cargo.toml", "w", encoding="utf-8").write(out)
PY
  # package.json del paquete principal y los subpaquetes por plataforma que haya en git.
  node -e '
    const fs = require("fs");
    const v = process.argv[1];
    const files = ["packages/hive-db/package.json"];
    for (const d of fs.existsSync("packages/hive-db/npm") ? fs.readdirSync("packages/hive-db/npm") : []) {
      const f = `packages/hive-db/npm/${d}/package.json`;
      if (fs.existsSync(f)) files.push(f);
    }
    for (const f of files) {
      const raw = fs.readFileSync(f, "utf8");
      const out = raw.replace(/("version":\s*")[^"]+(")/, `$1${v}$2`);
      fs.writeFileSync(f, out);
      console.log("   " + f + " → " + v);
    }
  ' "$NEW"
  # Cargo.lock: refrescar las versiones de los crates del workspace (sin tocar dependencias).
  cargo update --workspace --offline --quiet
  # CHANGELOG: la sección "Sin publicar" pasa a ser la versión, y queda una nueva vacía.
  python3 - "$NEW" "$TODAY" <<'PY'
import sys
new, today = sys.argv[1:3]
src = open("CHANGELOG.md", encoding="utf-8").read()
marker = "## Sin publicar"
head, _, rest = src.partition(marker)
open("CHANGELOG.md", "w", encoding="utf-8").write(f"{head}{marker}\n\n## {new} — {today}{rest}")
PY
  echo "   Cargo.toml, Cargo.lock y CHANGELOG.md → $NEW"
}
if [ "$DRY_RUN" = 1 ]; then
  echo "   [dry-run] Cargo.toml, packages/hive-db/package.json (+ npm/*), Cargo.lock y CHANGELOG.md → $NEW"
else
  bump_files
fi

# ---- commit, tag y subida ---------------------------------------------------------
say "Cambios que se van a commitear"
if [ "$DRY_RUN" = 1 ]; then echo "   [dry-run] (se mostraría git diff --stat)"; else git --no-pager diff --stat; fi

if [ "$DRY_RUN" = 0 ] && [ "$YES" = 0 ]; then
  printf '\n¿Crear el commit "chore(release): %s" y el tag %s' "$TAG" "$TAG"
  [ "$PUSH" = 1 ] && printf ' y subirlos a origin (esto PUBLICA en npm)'
  printf '? [s/N] '
  read -r answer
  case "$answer" in s | S | si | SI | y | Y | yes) ;; *) die "cancelado (los cambios de versión quedan sin commitear: git restore . para deshacerlos)" ;; esac
fi

say "Commit y tag"
run git add Cargo.toml Cargo.lock CHANGELOG.md packages/hive-db/package.json
# Los subpaquetes npm/ están en .gitignore pero hay cinco versionados: solo se actualizan esos.
run git add -u packages/hive-db/npm
run git commit -m "chore(release): $TAG"
run git tag -a "$TAG" -m "HiveDB $NEW"

if [ "$PUSH" = 1 ]; then
  say "Subiendo $BRANCH y $TAG"
  run git push --atomic origin "$BRANCH" "$TAG"
  say "Hecho. El workflow publicará $NEW al terminar los seis builds y los tests."
  REMOTE=$(git remote get-url origin 2>/dev/null | sed -E 's#(git@github.com:|https://github.com/)##; s#\.git$##')
  echo "   Seguimiento: https://github.com/${REMOTE}/actions"
  echo "   Paquete:     https://www.npmjs.com/package/@johpaz/hive-db"
else
  say "Hecho en local (--no-push). Para publicar:  git push --atomic origin $BRANCH $TAG"
fi
