# HiveDB — Guía de distribución

> Cómo consumir `@johpaz/hive-db` y cómo publicarlo en npm con binarios para todos los sistemas operativos usando `@napi-rs/cli`.

---

## 1. ¿De dónde lo importo?

El paquete está **publicado en npm** como `@johpaz/hive-db` (`bun add @johpaz/hive-db`); para desarrollar contra el código de este monorepo (`packages/hive-db`) tienes tres formas de consumirlo sin publicar:

### a) Dentro de este monorepo (workspace)

El `package.json` raíz declara `"workspaces": ["packages/*"]`. Cualquier paquete o app que añadas bajo `packages/` puede importarlo directamente:

```ts
import { HiveDB } from "@johpaz/hive-db";
```

### b) Desde otro proyecto Bun en la misma máquina (`file:`)

```bash
cd mi-otro-proyecto
bun add file:../ruta/a/hive-db/packages/hive-db
```

### c) Con `bun link` (desarrollo activo)

```bash
cd hive-db/packages/hive-db && bun link
cd mi-otro-proyecto && bun link @johpaz/hive-db
```

En los tres casos necesitas el binario nativo compilado para **tu** máquina:

```bash
cd packages/hive-db
bun run build:native   # napi build --platform --release + renombra index.js -> native.cjs
```

---

## 2. Distribución multiplataforma (npm)

La estrategia usa `@napi-rs/cli` 3.x: un paquete principal en TypeScript/JavaScript más **un paquete de binarios por plataforma** que npm/bun instala automáticamente según el SO gracias a los campos `os`/`cpu`/`libc`:

| Paquete | Plataforma |
|---|---|
| `@johpaz/hive-db` | principal (TS/JS, sin binarios) |
| `@johpaz/hive-db-linux-x64-gnu` | Linux x64 (glibc) |
| `@johpaz/hive-db-linux-x64-musl` | Linux x64 (musl / Alpine) |
| `@johpaz/hive-db-linux-arm64-gnu` | Linux arm64 (glibc) |
| `@johpaz/hive-db-darwin-x64` | macOS Intel |
| `@johpaz/hive-db-darwin-arm64` | macOS Apple Silicon |
| `@johpaz/hive-db-win32-x64-msvc` | Windows x64 |


### Tamaño del binario

Los binarios publicados (desde la 0.6.0) se compilan con `--features embedder-local`, es decir, **incluyen el
embedder local** (el código que ejecuta el modelo; los pesos de ~470 MB no viajan en el paquete y se descargan al
activarlo). Tamaño descomprimido de cada paquete de plataforma en npm:

| Plataforma | Con embedder (publicado) |
|---|---:|
| `linux-x64-gnu` | 16,8 MB |
| `linux-x64-musl` | 14,1 MB |
| `linux-arm64-gnu` | 15,5 MB |
| `darwin-x64` | 15,4 MB |
| `darwin-arm64` | 13,7 MB |
| `win32-x64-msvc` | 15,5 MB |

Sin el embedder, el binario de `linux-x64-gnu` pesa 10,0 MB (13,0 MB con la crate `hnsw_rs`, que sustituyó
el HNSW propio): el embedder añade unos 6,8 MB. Si prefieres un binario sin él, compila el binding tú sin la
feature (`bun run build:native`), y `embedder: "local"` fallará con `EMBEDDER_UNAVAILABLE`. El motor vectorial
usa `memmap2` (mapeo de memoria) y `rayon`; en Windows el índice se prueba en CI (`test-os`).

### Verificación del embedder por plataforma

Antes de publicar, el job `embedder-e2e` del CI prepara el modelo real con `HiveDB.prepareEmbedder` y ejecuta el
test de extremo a extremo en **linux x64 glibc, linux arm64, macOS arm64, Windows y linux x64 musl (Alpine)**.
`darwin-x64` solo se compila: GitHub no ofrece ya runners Intel. `publish` depende de toda esa matriz.

El loader `native.cjs` (generado por `napi build --platform`) detecta la plataforma en runtime —incluyendo la distinción glibc vs musl— y carga el binario correcto desde el subpaquete instalado o desde el archivo local de desarrollo.

El consumidor final solo hace:

```bash
bun add @johpaz/hive-db     # o npm install / pnpm add
```

y recibe el binario correcto para su SO sin necesitar Rust.

---

## 2b. Distribución en PyPI (`johpaz-hive-db`)

El binding Python se publica como **`johpaz-hive-db`** (import `hivedb`) con `maturin`: una wheel `abi3` por
plataforma, compilada con el embedder local (el modelo no viaja; se descarga al activarlo). Sirve a
Python 3.9 en adelante.

| Wheel | Plataforma |
|---|---|
| `manylinux_2_28_x86_64` / `aarch64` | Linux glibc, x64 y arm64 |
| `musllinux_1_2_x86_64` | Linux musl (Alpine) |
| `macosx_x86_64` / `macosx_arm64` | macOS Intel y Apple Silicon |
| `win_amd64` | Windows x64 |
| sdist | compila desde el código fuente (necesita Rust) |

La **versión** es la de `[workspace.package]` en `Cargo.toml` (`pyproject.toml` declara `dynamic = ["version"]`),
así que `scripts/release.sh` publica npm y PyPI con el mismo número. `johpaz-langchain-hivedb` (Python puro) lleva su
propia versión en `packages/langchain-hivedb/pyproject.toml` y se publica con el mismo workflow; si esa versión
ya está en PyPI se omite.

Requisitos (una sola vez): crear el proyecto `johpaz-hive-db` (y `johpaz-langchain-hivedb`) en PyPI y configurar
**Trusted Publishing** para el repositorio, workflow `ci.yml` y entorno `pypi`; o bien definir el secreto
`PYPI_API_TOKEN` (un token de la cuenta). Antes del primer release, comprueba que el nombre `johpaz-hive-db` esté libre. PyPI no tiene scopes como npm (`@johpaz/`): el prefijo `johpaz-` es su equivalente.

El workflow ejecuta, además de lo de npm: `wheels` (6 plataformas), `sdist`, `test-python` (pytest en Python
3.9 y 3.13 con la wheel recién construida), `python-e2e` (modelo real en 5 plataformas, musl en Alpine),
`test-langchain` (adaptadores, incluida la suite de contrato de `langchain-tests`) y, solo en tags `v*`,
`publish-pypi` (idempotente con `skip-existing`). `publish` (npm) también espera a todos ellos: si falla
Python, no se publica nada.

Manual (sin CI): `cd packages/hive-db-py && maturin build --release --out dist` (la wheel del SO actual) y
`maturin upload dist/*` / `twine upload dist/*`.


## 3. Cómo publicar una versión

### Requisitos (una sola vez)

1. **Repo git en GitHub.** `git init`, primer commit y push a `github.com/johpaz/hive-db`.
2. **Cuenta npm con el scope `@johpaz`.** Publica con `--access public` (el workflow ya lo hace).
3. **Token npm** de tipo *Automation* → guardarlo como secret `NPM_TOKEN` en el repo de GitHub (Settings → Secrets → Actions).

### Flujo de release

Un solo comando, desde la raíz del repositorio y con `main` al día y el árbol limpio:

```bash
scripts/release.sh 0.6.0 --dry-run   # ensayo: comprueba y enseña los pasos, sin tocar nada
scripts/release.sh 0.6.0             # o: patch | minor | major
```

El script (`scripts/release.sh`):

1. Comprueba que estás en `main`, sin cambios sin commitear, al día con `origin/main`, que el tag no
   existe y que `CHANGELOG.md` tiene una sección «Sin publicar».
2. Ejecuta `cargo fmt`, `clippy` (también con `embedder-local`) y `cargo test` (`--no-checks` para saltarlos).
3. Sube la versión en `Cargo.toml`, `packages/hive-db/package.json` (y los subpaquetes versionados),
   refresca `Cargo.lock` y convierte «Sin publicar» del CHANGELOG en la versión con su fecha.
4. Pide confirmación, crea el commit `chore(release): vX.Y.Z` y el tag anotado `vX.Y.Z`.
5. Sube la rama y el tag de forma atómica (`git push --atomic origin main vX.Y.Z`).

Al detectar el tag `v*`, el workflow `.github/workflows/ci.yml` ejecuta, por orden: lint y tests,
tests del índice en Windows y macOS, compilación de los seis binarios (con el embedder local),
tests de Bun, tests del embedder con el modelo real y, solo si todo pasó, el job `publish`:

1. `napi create-npm-dirs` — regenera los subpaquetes `npm/<triple>/`.
2. Descarga los binarios compilados en los runners de la matriz.
3. `napi artifacts` — coloca cada `.node` en su subpaquete.
4. Publica cada subpaquete: `npm publish ./npm/<triple> --access public` (se omite el que ya esté publicado).
5. Compila el TypeScript (`tsc`).
6. Publica el paquete principal: `npm publish --access public`.
7. Crea la GitHub Release con los binarios.

En paralelo, `publish-pypi` sube las wheels y el sdist de `johpaz-hive-db` y los artefactos de `johpaz-langchain-hivedb` (ver §2b).

Si algo falla a medias, corrige y vuelve a lanzar el job desde la pestaña Actions: la publicación es
idempotente. Para repetir una versión que no llegó a publicarse hay que borrar el tag
(`git push --delete origin vX.Y.Z && git tag -d vX.Y.Z`) y repetir el script.

También puedes lanzar el workflow a mano desde la pestaña Actions (`workflow_dispatch`): compila y
prueba, pero **no publica** (solo publican los tags `v*`).

### Publicación manual (sin CI)

Solo publicarías el binario de tu plataforma — útil para probar el flujo, no para distribuir a todos los SO:

```bash
cd packages/hive-db
bun run build:native
cp hivedb-napi.linux-x64-gnu.node npm/linux-x64-gnu/
npm publish ./npm/linux-x64-gnu --access public
bun run tsc -p tsconfig.json
npm publish --access public
```

---

## 4. Alternativas a npm público

- **GitHub Packages** (registro npm privado): añade `"publishConfig": { "registry": "https://npm.pkg.github.com" }` y usa `GITHUB_TOKEN` en el workflow. Los consumidores necesitan un `.npmrc` con auth. Útil si Hive es de código cerrado.
- **Registro privado propio** (Verdaccio) para uso interno del ecosistema Hive.
- **Vendoring**: copiar `packages/hive-db` + el `.node` como submódulo git. Solo razonable mientras todo corra en máquinas Linux x64 idénticas.

---

## 5. Plataformas no cubiertas (por ahora)

- **Windows arm64** (`win32-arm64-msvc`): añadir `aarch64-pc-windows-msvc` a `napi.targets` y a la matriz de CI.
- **Linux arm64 musl** (`linux-arm64-musl`): añadir `aarch64-unknown-linux-musl` a `napi.targets` y a la matriz con `-x` (zigbuild). El `target-feature=-crt-static` ya está en `.cargo/config.toml`.
- **Android**: `aarch64-linux-android` y `armv7-linux-androideabi` son soportados por `@napi-rs/cli`.

Para añadir una plataforma: (1) nueva entrada en `napi.targets` del `package.json`, (2) nueva entrada en la matriz de `ci.yml`, (3) ejecutar `napi create-npm-dirs` para regenerar los subpaquetes.