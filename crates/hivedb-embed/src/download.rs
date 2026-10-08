//! Descarga y caché de los pesos del modelo.
//!
//! Las URLs apuntan a un commit concreto del repositorio del modelo, y cada
//! archivo se verifica contra un SHA-256 fijado en el código: si el contenido
//! no es exactamente el esperado, no se usa.
//!
//! La descarga es robusta frente a redes malas: tiene tiempos máximos, reintenta con
//! espera creciente y **se reanuda** (`Range`) desde donde se cortó en lugar de empezar
//! de cero. Dos procesos que descargan a la vez no se pisan (fichero de bloqueo).

use hivedb_index::IndexError;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const REPO: &str = "intfloat/multilingual-e5-small";
const REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";

/// Reintentos tras el primer intento (la espera se duplica: 1 s, 2 s, 4 s, 8 s).
const RETRIES: usize = 4;
const BASE_BACKOFF: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// Velocidad mínima exigida (el cuerpo tiene un tiempo máximo proporcional a lo que falta).
/// Una conexión más lenta se corta y se reanuda en el siguiente intento.
const MIN_BYTES_PER_SECOND: u64 = 256 * 1024;
/// Un bloqueo que no se ha tocado en este tiempo es de un proceso muerto.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(120);

struct Artifact {
    name: &'static str,
    size: u64,
    sha256: &'static str,
}

const ARTIFACTS: &[Artifact] = &[
    Artifact {
        name: "config.json",
        size: 655,
        sha256: "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
    },
    Artifact {
        name: "tokenizer.json",
        size: 17_082_730,
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    },
    Artifact {
        name: "model.safetensors",
        size: 470_641_600,
        sha256: "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
    },
];

/// Directorio con los archivos del modelo ya verificados.
#[derive(Debug, Clone)]
pub struct ModelFiles {
    dir: PathBuf,
    space_id: String,
    downloaded: bool,
}

impl ModelFiles {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Identidad del espacio vectorial: modelo y revisión.
    pub fn space_id(&self) -> &str {
        &self.space_id
    }

    /// `true` si todos los archivos ya estaban en la caché (no se descargó nada).
    pub fn was_cached(&self) -> bool {
        !self.downloaded
    }
}

/// Avance de la descarga del archivo en curso.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Nombre del archivo (`model.safetensors`…).
    pub file: String,
    /// Posición de este archivo entre los que hay que descargar (desde 1).
    pub file_index: usize,
    pub file_count: usize,
    /// Bytes de este archivo ya descargados (incluye lo reanudado).
    pub downloaded: u64,
    /// Tamaño total de este archivo.
    pub total: u64,
}

/// `archivo.json` → `archivo.json.part` (se añade al nombre completo, no se sustituye la extensión).
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn fail(message: String) -> IndexError {
    IndexError::Embedder(message)
}

/// Raíz de la caché: `HIVEDB_MODEL_DIR`, o el directorio de caché del usuario.
pub fn default_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HIVEDB_MODEL_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("hivedb").join("models")
}

fn offline() -> bool {
    std::env::var("HIVEDB_OFFLINE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Dónde están los archivos del modelo: Hugging Face con la revisión fijada, o el espejo que
/// indique `HIVEDB_MODEL_BASE_URL` (misma estructura: `{base}/{archivo}`). Los SHA-256 se
/// comprueban igual: un espejo no puede servir un contenido distinto.
fn base_url() -> String {
    match std::env::var("HIVEDB_MODEL_BASE_URL") {
        Ok(url) if !url.trim().is_empty() => url.trim().trim_end_matches('/').to_string(),
        _ => format!("https://huggingface.co/{REPO}/resolve/{REVISION}"),
    }
}

/// Garantiza que los archivos de `multilingual-e5-small` están en la caché,
/// descargando los que falten (o tengan un tamaño inesperado). Imprime en `stderr` una
/// línea por cada archivo grande que descarga.
pub fn ensure_multilingual_e5_small() -> hivedb_index::Result<ModelFiles> {
    let mut announced = String::new();
    ensure_multilingual_e5_small_with(&mut |progress: &Progress| {
        if progress.total > 1_000_000 && announced != progress.file {
            announced = progress.file.clone();
            eprintln!(
                "hivedb-embed: descargando {} ({} MB) …",
                progress.file,
                progress.total / 1_000_000
            );
        }
    })
}

/// Como [`ensure_multilingual_e5_small`], pero avisando del avance de cada descarga.
pub fn ensure_multilingual_e5_small_with(
    on_progress: &mut dyn FnMut(&Progress),
) -> hivedb_index::Result<ModelFiles> {
    let dir = default_cache_dir().join(format!("multilingual-e5-small-{}", &REVISION[..8]));
    std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;

    // Un archivo presente se acepta por tamaño: el hash completo se comprobó al
    // descargarlo, y la caché solo se escribe por rename.
    let missing: Vec<&Artifact> = ARTIFACTS
        .iter()
        .filter(|a| !std::fs::metadata(dir.join(a.name)).is_ok_and(|m| m.len() == a.size))
        .collect();
    if offline() && !missing.is_empty() {
        return Err(fail(format!(
            "falta {} en {} y HIVEDB_OFFLINE=1 impide descargarlo",
            missing[0].name,
            dir.display()
        )));
    }

    let base = base_url();
    let count = missing.len();
    for (index, artifact) in missing.iter().enumerate() {
        let url = format!("{base}/{}", artifact.name);
        let mut report = |downloaded: u64| {
            on_progress(&Progress {
                file: artifact.name.to_string(),
                file_index: index + 1,
                file_count: count,
                downloaded,
                total: artifact.size,
            });
        };
        fetch(
            &url,
            artifact.size,
            artifact.sha256,
            &dir.join(artifact.name),
            Retry::default(),
            &mut report,
        )?;
    }

    Ok(ModelFiles {
        dir,
        space_id: format!("{REPO}@{}:mean-l2", &REVISION[..8]),
        downloaded: count > 0,
    })
}

#[derive(Clone, Copy)]
struct Retry {
    retries: usize,
    base_backoff: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            retries: RETRIES,
            base_backoff: BASE_BACKOFF,
        }
    }
}

/// Fallo de un intento: si merece otro intento (red, 5xx…) o no (404, contenido distinto).
enum Attempt {
    Retry(String),
    Fatal(String),
}

/// Descarga `url` a `destination` verificando tamaño y SHA-256. Con reintentos y reanudación.
fn fetch(
    url: &str,
    size: u64,
    sha256: &str,
    destination: &Path,
    retry: Retry,
    report: &mut dyn FnMut(u64),
) -> hivedb_index::Result<()> {
    let Some(_lock) = Lock::acquire(destination, size)? else {
        return Ok(()); // otro proceso ya lo dejó completo
    };
    let part = suffixed(destination, "part");
    let mut restarted_after_mismatch = false;
    let mut last_error = String::new();

    for attempt in 0..=retry.retries {
        if attempt > 0 {
            std::thread::sleep(retry.base_backoff * 2u32.pow(attempt as u32 - 1));
        }
        match attempt_once(url, size, sha256, &part, report) {
            Ok(()) => {
                std::fs::rename(&part, destination)
                    .map_err(|e| fail(format!("{}: {e}", destination.display())))?;
                return Ok(());
            }
            Err(Attempt::Retry(message)) => last_error = message,
            Err(Attempt::Fatal(message)) => {
                let _ = std::fs::remove_file(&part);
                // Un prefijo reanudado puede venir corrupto: se prueba una vez desde cero.
                if message.starts_with("MISMATCH") && !restarted_after_mismatch {
                    restarted_after_mismatch = true;
                    last_error = message;
                    continue;
                }
                return Err(fail(format!("{url}: {message}")));
            }
        }
    }
    Err(fail(format!(
        "no se pudo descargar {url} tras {} intentos: {last_error}. Lo ya descargado se conserva y \
         se reanudará la próxima vez.",
        retry.retries + 1
    )))
}

fn attempt_once(
    url: &str,
    size: u64,
    sha256: &str,
    part: &Path,
    report: &mut dyn FnMut(u64),
) -> Result<(), Attempt> {
    let io = |what: &str, e: std::io::Error| Attempt::Fatal(format!("{what}: {e}"));

    let mut existing = std::fs::metadata(part).map_or(0, |m| m.len());
    if existing > size {
        existing = 0;
        std::fs::remove_file(part).map_err(|e| io("limpiando descarga parcial", e))?;
    }
    if existing == size {
        // Descarga completa que no llegó a renombrarse: solo falta verificar.
        return verify_file(part, size, sha256);
    }

    let remaining = size - existing;
    let budget = Duration::from_secs(60 + remaining / MIN_BYTES_PER_SECOND);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .timeout_recv_body(Some(budget))
        .build()
        .into();
    let mut request = agent.get(url);
    if existing > 0 {
        request = request.header("Range", format!("bytes={existing}-"));
    }
    let mut response = request
        .call()
        .map_err(|e| Attempt::Retry(format!("conexión: {e}")))?;

    let status = response.status().as_u16();
    let start = match status {
        200 => 0, // el servidor ignora `Range` (o no se pidió): llega el archivo entero
        206 => {
            let from = response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes "))
                .and_then(|v| v.split('-').next())
                .and_then(|v| v.parse::<u64>().ok());
            if from != Some(existing) {
                let _ = std::fs::remove_file(part);
                return Err(Attempt::Retry(format!(
                    "el servidor respondió un rango inesperado ({from:?} en vez de {existing})"
                )));
            }
            existing
        }
        416 => {
            // El rango pedido no existe: lo que hay en disco no sirve.
            let _ = std::fs::remove_file(part);
            return Err(Attempt::Retry("rango no satisfacible (HTTP 416)".into()));
        }
        408 | 429 | 500..=599 => return Err(Attempt::Retry(format!("HTTP {status}"))),
        other => return Err(Attempt::Fatal(format!("HTTP {other}"))),
    };

    let mut hasher = Sha256::new();
    let mut file = if start == 0 {
        std::fs::File::create(part).map_err(|e| io(&part.display().to_string(), e))?
    } else {
        // Reanudación: el hash cubre todo el archivo, así que se reprocesa lo ya descargado.
        let mut prior =
            std::fs::File::open(part).map_err(|e| io(&part.display().to_string(), e))?;
        hash_into(&mut prior, &mut hasher).map_err(|e| io("leyendo la descarga parcial", e))?;
        std::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| io(&part.display().to_string(), e))?
    };

    let mut reader = response.body_mut().as_reader();
    let mut written = start;
    let mut buffer = vec![0u8; 1 << 20];
    report(written);
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = file.flush();
                return Err(Attempt::Retry(format!("lectura interrumpida: {e}")));
            }
        };
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .map_err(|e| io(&part.display().to_string(), e))?;
        written += read as u64;
        report(written);
    }
    file.flush()
        .map_err(|e| io(&part.display().to_string(), e))?;
    file.sync_all()
        .map_err(|e| io(&part.display().to_string(), e))?;

    if written < size {
        // Cortado limpiamente antes de tiempo: se conserva el parcial y se reanuda.
        return Err(Attempt::Retry(format!(
            "la descarga terminó en {written} de {size} bytes"
        )));
    }
    check_digest(hasher, written, size, sha256)
}

fn verify_file(path: &Path, size: u64, sha256: &str) -> Result<(), Attempt> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)
        .map_err(|e| Attempt::Fatal(format!("{}: {e}", path.display())))?;
    let written = hash_into(&mut file, &mut hasher)
        .map_err(|e| Attempt::Fatal(format!("{}: {e}", path.display())))?;
    check_digest(hasher, written, size, sha256)
}

fn hash_into(reader: &mut impl Read, hasher: &mut Sha256) -> std::io::Result<u64> {
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(total);
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
}

fn check_digest(hasher: Sha256, written: u64, size: u64, sha256: &str) -> Result<(), Attempt> {
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if written != size || digest != sha256 {
        return Err(Attempt::Fatal(format!(
            "MISMATCH: no coincide con la versión esperada (tamaño {written}/{size}, sha256 {digest})"
        )));
    }
    Ok(())
}

/// Bloqueo entre procesos (y entre hilos) para un archivo de la caché.
struct Lock {
    path: PathBuf,
}

impl Lock {
    /// `Ok(None)` si, mientras esperaba, otro proceso dejó el archivo completo.
    fn acquire(destination: &Path, size: u64) -> hivedb_index::Result<Option<Self>> {
        let path = suffixed(destination, "lock");
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => {
                    let lock = Self { path };
                    // El otro proceso pudo terminar justo antes de soltar el bloqueo.
                    if std::fs::metadata(destination).is_ok_and(|m| m.len() == size) {
                        return Ok(None);
                    }
                    return Ok(Some(lock));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::fs::metadata(destination).is_ok_and(|m| m.len() == size) {
                        return Ok(None);
                    }
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > LOCK_STALE_AFTER);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() > Duration::from_secs(30 * 60) {
                        return Err(fail(format!(
                            "otro proceso lleva más de 30 minutos descargando {}",
                            destination.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => return Err(fail(format!("{}: {e}", path.display()))),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    enum Behavior {
        /// Atiende `Range` con 206.
        Normal,
        /// Ignora `Range` y devuelve siempre el archivo entero (200).
        IgnoreRange,
        /// La primera petición promete el archivo entero pero corta tras `n` bytes.
        CutFirst(usize),
        /// Las primeras `n` peticiones fallan con HTTP 500.
        Fail500(usize),
        /// Siempre 404.
        NotFound,
    }

    struct Server {
        url: String,
        requests: Arc<AtomicUsize>,
        ranges: Arc<Mutex<Vec<Option<u64>>>>,
    }

    fn data(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        (0..len)
            .map(|_| {
                state ^= state >> 12;
                state ^= state << 25;
                state ^= state >> 27;
                (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8
            })
            .collect()
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn serve(body: Vec<u8>, behavior: Behavior) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let (counter, seen) = (requests.clone(), ranges.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse::<u64>().ok();
                    }
                }
                seen.lock().unwrap().push(range);
                let total = body.len();
                let (status, from) = match (behavior, range) {
                    (Behavior::NotFound, _) => ("404 Not Found", 0),
                    (Behavior::Fail500(n), _) if index < n => ("500 Internal Server Error", 0),
                    (Behavior::IgnoreRange, _) | (_, None) => ("200 OK", 0),
                    (_, Some(start)) => ("206 Partial Content", start as usize),
                };
                if status.starts_with('4') || status.starts_with('5') {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    continue;
                }
                let payload = &body[from..];
                let mut head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    payload.len()
                );
                if status.starts_with("206") {
                    head += &format!("Content-Range: bytes {from}-{}/{total}\r\n", total - 1);
                }
                head += "\r\n";
                let _ = stream.write_all(head.as_bytes());
                let send = match behavior {
                    Behavior::CutFirst(n) if index == 0 => &payload[..n.min(payload.len())],
                    _ => payload,
                };
                let _ = stream.write_all(send);
            }
        });
        Server {
            url,
            requests,
            ranges,
        }
    }

    const FAST: Retry = Retry {
        retries: 3,
        base_backoff: Duration::from_millis(5),
    };

    fn run(
        server: &Server,
        body: &[u8],
        dir: &Path,
        expected_sha: &str,
    ) -> hivedb_index::Result<Vec<u64>> {
        let mut seen = Vec::new();
        fetch(
            &server.url,
            body.len() as u64,
            expected_sha,
            &dir.join("model.bin"),
            FAST,
            &mut |downloaded| seen.push(downloaded),
        )?;
        Ok(seen)
    }

    #[test]
    fn descarga_completa_y_avisa_del_avance() {
        let body = data(3 << 20);
        let server = serve(body.clone(), Behavior::Normal);
        let dir = tempfile::tempdir().unwrap();
        let seen = run(&server, &body, dir.path(), &sha(&body)).unwrap();
        assert_eq!(std::fs::read(dir.path().join("model.bin")).unwrap(), body);
        assert_eq!(seen.first(), Some(&0));
        assert_eq!(seen.last(), Some(&(body.len() as u64)));
        assert!(seen.windows(2).all(|w| w[0] <= w[1]));
        assert!(!dir.path().join("model.bin.part").exists());
        assert!(!dir.path().join("model.bin.lock").exists());
    }

    #[test]
    fn un_corte_a_mitad_se_reanuda_sin_empezar_de_cero() {
        let body = data(3 << 20);
        let server = serve(body.clone(), Behavior::CutFirst(1 << 20));
        let dir = tempfile::tempdir().unwrap();
        run(&server, &body, dir.path(), &sha(&body)).unwrap();
        assert_eq!(std::fs::read(dir.path().join("model.bin")).unwrap(), body);
        // Segundo intento: pide desde donde se cortó, no desde el principio.
        let ranges = server.ranges.lock().unwrap().clone();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], None);
        assert_eq!(ranges[1], Some(1 << 20));
    }

    #[test]
    fn reintenta_ante_errores_del_servidor() {
        let body = data(1 << 20);
        let server = serve(body.clone(), Behavior::Fail500(2));
        let dir = tempfile::tempdir().unwrap();
        run(&server, &body, dir.path(), &sha(&body)).unwrap();
        assert_eq!(server.requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn un_404_no_se_reintenta() {
        let body = data(1 << 10);
        let server = serve(body.clone(), Behavior::NotFound);
        let dir = tempfile::tempdir().unwrap();
        let error = run(&server, &body, dir.path(), &sha(&body)).unwrap_err();
        assert!(error.to_string().contains("404"), "{error}");
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn un_contenido_distinto_se_descarta() {
        let body = data(1 << 20);
        let server = serve(body.clone(), Behavior::Normal);
        let dir = tempfile::tempdir().unwrap();
        let error = run(&server, &body, dir.path(), &sha(b"otra cosa")).unwrap_err();
        assert!(error.to_string().contains("no coincide"), "{error}");
        assert!(!dir.path().join("model.bin").exists());
        assert!(!dir.path().join("model.bin.part").exists());
    }

    #[test]
    fn si_el_servidor_ignora_range_se_empieza_de_cero() {
        let body = data(2 << 20);
        let server = serve(body.clone(), Behavior::IgnoreRange);
        let dir = tempfile::tempdir().unwrap();
        // Parcial previo (por ejemplo de otra ejecución): el servidor no sabe reanudar.
        std::fs::write(dir.path().join("model.bin.part"), &body[..(1 << 20)]).unwrap();
        run(&server, &body, dir.path(), &sha(&body)).unwrap();
        assert_eq!(std::fs::read(dir.path().join("model.bin")).unwrap(), body);
    }

    #[test]
    fn un_parcial_previo_se_reanuda() {
        let body = data(3 << 20);
        let server = serve(body.clone(), Behavior::Normal);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.bin.part"), &body[..(2 << 20)]).unwrap();
        let seen = run(&server, &body, dir.path(), &sha(&body)).unwrap();
        assert_eq!(std::fs::read(dir.path().join("model.bin")).unwrap(), body);
        assert_eq!(seen.first(), Some(&(2 << 20)));
        assert_eq!(server.ranges.lock().unwrap()[0], Some(2 << 20));
    }

    #[test]
    fn un_parcial_corrupto_se_descarta_y_se_reintenta_desde_cero() {
        let body = data(2 << 20);
        let server = serve(body.clone(), Behavior::Normal);
        let dir = tempfile::tempdir().unwrap();
        let mut corrupt = body[..(1 << 20)].to_vec();
        corrupt[10] ^= 0xFF;
        std::fs::write(dir.path().join("model.bin.part"), corrupt).unwrap();
        run(&server, &body, dir.path(), &sha(&body)).unwrap();
        assert_eq!(std::fs::read(dir.path().join("model.bin")).unwrap(), body);
    }

    #[test]
    fn un_bloqueo_ajeno_hace_esperar_hasta_que_termine() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("model.bin");
        let lock_path = suffixed(&destination, "lock");
        std::fs::write(&lock_path, b"").unwrap();
        let releaser = {
            let (lock_path, destination) = (lock_path.clone(), destination.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                // El otro proceso termina: deja el archivo completo y suelta el bloqueo.
                std::fs::write(&destination, b"completo").unwrap();
                std::fs::remove_file(&lock_path).unwrap();
            })
        };
        // Si esperara de más o descargara de nuevo, la URL inválida haría fallar el test.
        let result = Lock::acquire(&destination, b"completo".len() as u64).unwrap();
        releaser.join().unwrap();
        assert!(
            result.is_none(),
            "debía detectar que el otro proceso terminó"
        );
    }
}
