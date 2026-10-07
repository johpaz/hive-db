//! HNSW con almacenamiento plano.
//!
//! Sustituye a `hnsw_rs`. Los motivos, medidos con 100k vectores de 384
//! dimensiones:
//!
//! - **Arranque:** `hnsw_rs` reconstruye un objeto por vecino al cargar
//!   (~0,9 s). Aquí el grafo son arrays `u32` que se leen en bloque y los
//!   vectores se mapean en memoria (`mmap`): la carga cuesta milisegundos.
//! - **Disco:** una sola copia de los vectores, contiguos, y el grafo en
//!   `u32` (~20 MiB en lugar de ~80 MiB).
//! - **Latencia y p99:** vectores contiguos y listas de vecinos en arrays,
//!   sin `Arc`/`RwLock` por vecino, y búsquedas concurrentes sin bloqueo.
//! - **Robustez:** sin `panic!` ante ficheros corruptos (se validan al
//!   cargar) y sin fugas intencionadas de memoria.
//!
//! # Construcción en paralelo, sin locks y determinista
//!
//! Los vectores nuevos se añaden por tandas. Para cada tanda, primero se
//! calculan en paralelo (solo lectura) los vecinos de cada nodo nuevo sobre el
//! grafo ya enlazado; después se aplican las aristas directas y las inversas
//! (agrupadas por destino, también en paralelo y de solo lectura hasta aplicar).
//! Como ningún hilo escribe mientras otros leen, no hay locks y el resultado no
//! depende del número de hilos ni de su planificación: la misma entrada da el
//! mismo grafo. El tamaño de tanda crece con el grafo (≈ 1/32 de lo ya
//! enlazado) para que los nodos de una misma tanda no se pierdan de vista.
//!
//! Los vectores se guardan normalizados (L2); la distancia es `1 - a·b`.

use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::Write;
use std::path::Path;

const NO_NODE: u32 = u32::MAX;
const MAX_LEVEL: usize = 15;
/// Una tanda enlaza como mucho `enlazados / CHUNK_DIVISOR` nodos nuevos.
const CHUNK_DIVISOR: usize = 32;
const MAX_CHUNK: usize = 4_096;
/// Por debajo de este tamaño el reparto entre hilos cuesta más que lo que ahorra.
const PARALLEL_MIN: usize = 16;
/// Al podar una lista de vecinos, rellenar con los descartados hasta la capacidad.
const KEEP_PRUNED: bool = true;

const GRAPH_MAGIC: &[u8; 8] = b"HIVEGRPH";
const GRAPH_FORMAT: u32 = 1;

/// Producto punto en `f32` con 16 acumuladores independientes: el compilador lo
/// vectoriza (SSE/AVX/NEON) sin `unsafe` ni dependencias.
pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
    const LANES: usize = 16;
    let mut acc = [0.0f32; LANES];
    let a_chunks = a.chunks_exact(LANES);
    let b_chunks = b.chunks_exact(LANES);
    let (a_tail, b_tail) = (a_chunks.remainder(), b_chunks.remainder());
    for (x, y) in a_chunks.zip(b_chunks) {
        for i in 0..LANES {
            acc[i] += x[i] * y[i];
        }
    }
    let tail: f32 = a_tail.iter().zip(b_tail).map(|(x, y)| x * y).sum();
    acc.iter().sum::<f32>() + tail
}

/// Distancia coseno entre vectores ya normalizados.
fn dist(a: &[f32], b: &[f32]) -> f32 {
    (1.0 - dot(a, b)).max(0.0)
}

/// Distancia como entero ordenable (los `f32` no negativos conservan el orden
/// de sus bits), para usarla en `BinaryHeap` sin envoltorios con `partial_cmp`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Dist(u32);

impl Dist {
    fn new(distance: f32) -> Self {
        Dist(distance.max(0.0).to_bits())
    }
    fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// Conjunto de nodos visitados en una búsqueda.
struct Visited(Vec<u64>);

impl Visited {
    fn new(nodes: usize) -> Self {
        Visited(vec![0; nodes.div_ceil(64)])
    }
    /// `true` si el nodo no estaba visitado.
    fn insert(&mut self, node: u32) -> bool {
        let (word, bit) = (node as usize / 64, node as usize % 64);
        let seen = (self.0[word] >> bit) & 1 == 1;
        self.0[word] |= 1 << bit;
        !seen
    }
}

/// Vectores del grafo: un bloque restaurado (mapeado o en memoria) y una cola
/// con los añadidos desde la carga.
enum Base {
    None,
    #[cfg(not(windows))]
    Mapped(memmap2::Mmap),
    #[cfg_attr(not(windows), allow(dead_code))]
    Owned(Vec<f32>),
}

struct Vectors {
    dim: usize,
    base: Base,
    base_len: usize,
    tail: Vec<f32>,
}

impl Vectors {
    fn get(&self, id: u32) -> &[f32] {
        let i = id as usize;
        if i < self.base_len {
            match &self.base {
                #[cfg(not(windows))]
                Base::Mapped(map) => {
                    let bytes = &map[i * self.dim * 4..(i + 1) * self.dim * 4];
                    bytemuck::cast_slice(bytes)
                }
                Base::Owned(all) => &all[i * self.dim..(i + 1) * self.dim],
                Base::None => unreachable!("base_len > 0 sin base"),
            }
        } else {
            let j = i - self.base_len;
            &self.tail[j * self.dim..(j + 1) * self.dim]
        }
    }

    /// Escribe todos los vectores, en orden, como `f32` little-endian.
    fn write_to(&self, writer: &mut impl Write) -> std::io::Result<()> {
        match &self.base {
            #[cfg(not(windows))]
            Base::Mapped(map) => writer.write_all(&map[..self.base_len * self.dim * 4])?,
            Base::Owned(all) => write_f32s(writer, all)?,
            Base::None => {}
        }
        write_f32s(writer, &self.tail)
    }
}

fn write_f32s(writer: &mut impl Write, values: &[f32]) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(64 * 1024);
    for chunk in values.chunks(16 * 1024) {
        buffer.clear();
        for value in chunk {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        writer.write_all(&buffer)?;
    }
    Ok(())
}

/// Capa superior (≥ 1): solo contiene a los nodos cuyo nivel la alcanza.
struct Upper {
    /// Fila → nodo.
    nodes: Vec<u32>,
    /// Nodo → fila (`NO_NODE` si el nodo no está en esta capa). Una entrada por nodo.
    row_of: Vec<u32>,
    adj: Vec<u32>,
    len: Vec<u8>,
}

pub(crate) struct Graph {
    dim: usize,
    /// Vecinos máximos en capas superiores; la capa 0 admite `2 * m`.
    m: usize,
    m0: usize,
    ef_construction: usize,
    vectors: Vectors,
    levels: Vec<u8>,
    l0: Vec<u32>,
    l0_len: Vec<u8>,
    upper: Vec<Upper>,
    entry: u32,
    max_level: usize,
    /// `true` si hay vectores que no están en el fichero de datos restaurado.
    vectors_dirty: bool,
}

/// Nivel de un nodo: geométrico con parámetro `1/ln(m)`, derivado del id para
/// que la construcción sea reproducible.
fn level_for(id: u32, m: usize) -> usize {
    let mut z = u64::from(id).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let u = ((z >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
    (((-u.ln()) / (m as f64).ln()).floor() as usize).min(MAX_LEVEL)
}

impl Graph {
    pub(crate) fn new(dim: usize, m: usize, ef_construction: usize) -> Self {
        assert!(m >= 2 && 2 * m <= usize::from(u8::MAX));
        Self {
            dim,
            m,
            m0: 2 * m,
            ef_construction,
            vectors: Vectors {
                dim,
                base: Base::None,
                base_len: 0,
                tail: Vec::new(),
            },
            levels: Vec::new(),
            l0: Vec::new(),
            l0_len: Vec::new(),
            upper: Vec::new(),
            entry: NO_NODE,
            max_level: 0,
            vectors_dirty: true,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.levels.len()
    }

    /// `true` si el fichero de datos en disco ya contiene todos los vectores.
    pub(crate) fn vectors_clean(&self) -> bool {
        !self.vectors_dirty
    }

    /// Anota que el fichero de datos en disco ya tiene todos los vectores.
    pub(crate) fn mark_vectors_persisted(&mut self) {
        self.vectors_dirty = false;
    }

    fn neighbors(&self, layer: usize, node: u32) -> &[u32] {
        if layer == 0 {
            let start = node as usize * self.m0;
            &self.l0[start..start + usize::from(self.l0_len[node as usize])]
        } else {
            let upper = &self.upper[layer - 1];
            let row = upper.row_of[node as usize] as usize;
            let start = row * self.m;
            &upper.adj[start..start + usize::from(upper.len[row])]
        }
    }

    fn set_neighbors(&mut self, layer: usize, node: u32, list: &[u32]) {
        if layer == 0 {
            let start = node as usize * self.m0;
            self.l0[start..start + list.len()].copy_from_slice(list);
            self.l0_len[node as usize] = list.len() as u8;
        } else {
            let m = self.m;
            let upper = &mut self.upper[layer - 1];
            let row = upper.row_of[node as usize] as usize;
            let start = row * m;
            upper.adj[start..start + list.len()].copy_from_slice(list);
            upper.len[row] = list.len() as u8;
        }
    }

    /// Añade un vector (lo normaliza) sin enlazarlo. Devuelve su id.
    fn append(&mut self, vector: &[f32]) -> u32 {
        debug_assert_eq!(vector.len(), self.dim);
        let id = self.len() as u32;
        let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            self.vectors.tail.extend(vector.iter().map(|x| x / norm));
        } else {
            self.vectors.tail.extend_from_slice(vector);
        }
        self.vectors_dirty = true;

        let level = level_for(id, self.m);
        self.levels.push(level as u8);
        self.l0.resize(self.l0.len() + self.m0, 0);
        self.l0_len.push(0);
        for upper in &mut self.upper {
            upper.row_of.push(NO_NODE);
        }
        while self.upper.len() < level {
            self.upper.push(Upper {
                nodes: Vec::new(),
                row_of: vec![NO_NODE; id as usize + 1],
                adj: Vec::new(),
                len: Vec::new(),
            });
        }
        for layer in 1..=level {
            let upper = &mut self.upper[layer - 1];
            upper.row_of[id as usize] = upper.nodes.len() as u32;
            upper.nodes.push(id);
            upper.adj.resize(upper.adj.len() + self.m, 0);
            upper.len.push(0);
        }
        id
    }

    /// Añade y enlaza todos los vectores. Devuelve el id del primero.
    pub(crate) fn insert_batch(&mut self, vectors: &[&[f32]]) -> u32 {
        let first = self.len() as u32;
        for vector in vectors {
            self.append(vector);
        }
        let end = self.len() as u32;
        let mut next = first;
        while next < end {
            let size = if self.entry == NO_NODE {
                1
            } else {
                (next as usize / CHUNK_DIVISOR).clamp(1, MAX_CHUNK)
            };
            let stop = (next + size as u32).min(end);
            self.link_chunk(next, stop);
            next = stop;
        }
        first
    }

    fn greedy(&self, query: &[f32], mut best: (f32, u32), layer: usize) -> (f32, u32) {
        loop {
            let mut improved = false;
            for &neighbor in self.neighbors(layer, best.1) {
                let d = dist(query, self.vectors.get(neighbor));
                if d < best.0 {
                    best = (d, neighbor);
                    improved = true;
                }
            }
            if !improved {
                return best;
            }
        }
    }

    /// Mejores `ef` candidatos de una capa, de menor a mayor distancia.
    fn search_layer(
        &self,
        query: &[f32],
        entries: &[(f32, u32)],
        ef: usize,
        layer: usize,
    ) -> Vec<(f32, u32)> {
        let mut visited = Visited::new(self.len());
        let mut candidates: BinaryHeap<Reverse<(Dist, u32)>> = BinaryHeap::new();
        let mut results: BinaryHeap<(Dist, u32)> = BinaryHeap::new();
        for &(d, node) in entries {
            if visited.insert(node) {
                candidates.push(Reverse((Dist::new(d), node)));
                results.push((Dist::new(d), node));
                if results.len() > ef {
                    results.pop();
                }
            }
        }
        while let Some(Reverse((d, current))) = candidates.pop() {
            if results.len() >= ef && results.peek().is_some_and(|worst| d > worst.0) {
                break;
            }
            for &neighbor in self.neighbors(layer, current) {
                if !visited.insert(neighbor) {
                    continue;
                }
                let dn = Dist::new(dist(query, self.vectors.get(neighbor)));
                if results.len() < ef || results.peek().is_some_and(|worst| dn < worst.0) {
                    candidates.push(Reverse((dn, neighbor)));
                    results.push((dn, neighbor));
                    if results.len() > ef {
                        results.pop();
                    }
                }
            }
        }
        let mut out: Vec<(f32, u32)> = results.into_iter().map(|(d, n)| (d.get(), n)).collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        out
    }

    /// Heurística de diversidad (algoritmo 4 del paper): un candidato entra si
    /// está más cerca del nodo que de cualquiera de los ya elegidos. Con
    /// `keep_pruned` se rellena con los descartados hasta `cap`.
    fn select(&self, candidates: &[(f32, u32)], cap: usize, keep_pruned: bool) -> Vec<u32> {
        let mut selected: Vec<(f32, u32)> = Vec::with_capacity(cap);
        let mut pruned: Vec<u32> = Vec::new();
        for &(d, node) in candidates {
            if selected.len() >= cap {
                break;
            }
            let vector = self.vectors.get(node);
            let diverse = selected
                .iter()
                .all(|&(_, other)| dist(vector, self.vectors.get(other)) > d);
            if diverse {
                selected.push((d, node));
            } else if keep_pruned {
                pruned.push(node);
            }
        }
        let mut out: Vec<u32> = selected.into_iter().map(|(_, node)| node).collect();
        if keep_pruned {
            for node in pruned {
                if out.len() >= cap {
                    break;
                }
                out.push(node);
            }
        }
        out
    }

    /// Vecinos elegidos para un nodo nuevo, por capa (índice = capa).
    fn plan(&self, node: u32) -> Vec<Vec<u32>> {
        let query = self.vectors.get(node);
        let level = usize::from(self.levels[node as usize]);
        let top = level.min(self.max_level);
        let mut entry = (dist(query, self.vectors.get(self.entry)), self.entry);
        for layer in (top + 1..=self.max_level).rev() {
            entry = self.greedy(query, entry, layer);
        }
        let mut plan = vec![Vec::new(); top + 1];
        let mut entries = vec![entry];
        for layer in (0..=top).rev() {
            let found = self.search_layer(query, &entries, self.ef_construction, layer);
            plan[layer] = self.select(&found, self.m, KEEP_PRUNED);
            entries = found;
        }
        plan
    }

    /// Lista de vecinos de `node` en `layer` tras añadir las aristas inversas.
    fn merged(&self, layer: usize, node: u32, incoming: &[u32]) -> Vec<u32> {
        let cap = if layer == 0 { self.m0 } else { self.m };
        let mut all = self.neighbors(layer, node).to_vec();
        for &source in incoming {
            if !all.contains(&source) {
                all.push(source);
            }
        }
        if all.len() <= cap {
            return all;
        }
        let vector = self.vectors.get(node);
        let mut scored: Vec<(f32, u32)> = all
            .iter()
            .map(|&other| (dist(vector, self.vectors.get(other)), other))
            .collect();
        scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        self.select(&scored, cap, KEEP_PRUNED)
    }

    /// Enlaza los nodos `start..end` (ya añadidos) al grafo.
    fn link_chunk(&mut self, start: u32, end: u32) {
        if self.entry == NO_NODE {
            self.entry = start;
            self.max_level = usize::from(self.levels[start as usize]);
            // Una tanda sobre un grafo vacío siempre es de un único nodo.
            debug_assert_eq!(end, start + 1);
            return;
        }

        let nodes: Vec<u32> = (start..end).collect();
        let plans: Vec<Vec<Vec<u32>>> = if nodes.len() >= PARALLEL_MIN {
            nodes.par_iter().map(|&node| self.plan(node)).collect()
        } else {
            nodes.iter().map(|&node| self.plan(node)).collect()
        };

        // Aristas directas y recopilación de las inversas.
        let mut reverse: Vec<(usize, u32, u32)> = Vec::new();
        for (&node, plan) in nodes.iter().zip(&plans) {
            for (layer, list) in plan.iter().enumerate() {
                self.set_neighbors(layer, node, list);
                for &target in list {
                    reverse.push((layer, target, node));
                }
            }
        }
        reverse.sort_unstable();

        // Aristas inversas, agrupadas por destino. El cálculo (poda incluida)
        // es de solo lectura y paralelo; la escritura, secuencial.
        let mut groups: Vec<(usize, u32, std::ops::Range<usize>)> = Vec::new();
        let mut from = 0;
        while from < reverse.len() {
            let (layer, target, _) = reverse[from];
            let mut to = from + 1;
            while to < reverse.len() && reverse[to].0 == layer && reverse[to].1 == target {
                to += 1;
            }
            groups.push((layer, target, from..to));
            from = to;
        }
        let compute = |(layer, target, range): &(usize, u32, std::ops::Range<usize>)| {
            let incoming: Vec<u32> = reverse[range.clone()].iter().map(|r| r.2).collect();
            (*layer, *target, self.merged(*layer, *target, &incoming))
        };
        let updates: Vec<(usize, u32, Vec<u32>)> = if groups.len() >= PARALLEL_MIN {
            groups.par_iter().map(compute).collect()
        } else {
            groups.iter().map(compute).collect()
        };
        for (layer, target, list) in updates {
            self.set_neighbors(layer, target, &list);
        }

        for &node in &nodes {
            let level = usize::from(self.levels[node as usize]);
            if level > self.max_level {
                self.max_level = level;
                self.entry = node;
            }
        }
    }

    /// Los `limit` vecinos más cercanos de `query` (se normaliza aquí), de
    /// menor a mayor distancia: `(id, distancia coseno)`.
    pub(crate) fn search(&self, query: &[f32], ef: usize, limit: usize) -> Vec<(u32, f32)> {
        if self.entry == NO_NODE || limit == 0 {
            return Vec::new();
        }
        let norm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        let query: Vec<f32> = if norm > 0.0 {
            query.iter().map(|x| x / norm).collect()
        } else {
            query.to_vec()
        };
        let mut entry = (dist(&query, self.vectors.get(self.entry)), self.entry);
        for layer in (1..=self.max_level).rev() {
            entry = self.greedy(&query, entry, layer);
        }
        let mut found = self.search_layer(&query, &[entry], ef.max(limit), 0);
        found.truncate(limit);
        found.into_iter().map(|(d, node)| (node, d)).collect()
    }

    // ---- Persistencia ----------------------------------------------------

    /// Escribe el grafo (sin los vectores) en `path`.
    pub(crate) fn write_graph(&self, path: &Path) -> std::io::Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        out.write_all(GRAPH_MAGIC)?;
        for value in [GRAPH_FORMAT, self.dim as u32, self.m as u32] {
            out.write_all(&value.to_le_bytes())?;
        }
        out.write_all(&(self.ef_construction as u32).to_le_bytes())?;
        out.write_all(&(self.len() as u64).to_le_bytes())?;
        out.write_all(&self.entry.to_le_bytes())?;
        out.write_all(&(self.max_level as u32).to_le_bytes())?;
        out.write_all(&self.levels)?;
        out.write_all(&self.l0_len)?;
        write_u32s(&mut out, &self.l0)?;
        for upper in &self.upper {
            out.write_all(&(upper.nodes.len() as u64).to_le_bytes())?;
            write_u32s(&mut out, &upper.nodes)?;
            out.write_all(&upper.len)?;
            write_u32s(&mut out, &upper.adj)?;
        }
        out.flush()
    }

    /// Escribe todos los vectores (normalizados) en `path`.
    pub(crate) fn write_vectors(&self, path: &Path) -> std::io::Result<()> {
        let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
        self.vectors.write_to(&mut out)?;
        out.flush()
    }

    /// Carga un grafo. Devuelve `None` ante cualquier incoherencia: el llamador
    /// reconstruye desde los documentos, que son la fuente de verdad.
    pub(crate) fn read(graph_path: &Path, data_path: &Path, dim: usize) -> Option<Self> {
        let raw = std::fs::read(graph_path).ok()?;
        let mut cursor = Cursor { bytes: &raw, at: 0 };
        if cursor.take(8)? != GRAPH_MAGIC || cursor.u32()? != GRAPH_FORMAT {
            return None;
        }
        if cursor.u32()? as usize != dim {
            return None;
        }
        let m = cursor.u32()? as usize;
        let ef_construction = cursor.u32()? as usize;
        if m < 2 || 2 * m > usize::from(u8::MAX) {
            return None;
        }
        let n = usize::try_from(cursor.u64()?).ok()?;
        let entry = cursor.u32()?;
        let max_level = cursor.u32()? as usize;
        if max_level > MAX_LEVEL || n >= NO_NODE as usize {
            return None;
        }
        if (n == 0) != (entry == NO_NODE) || (entry != NO_NODE && entry as usize >= n) {
            return None;
        }
        let m0 = 2 * m;
        let levels = cursor.take(n)?.to_vec();
        let l0_len = cursor.take(n)?.to_vec();
        let l0 = cursor.u32s(n.checked_mul(m0)?)?;
        if levels.iter().any(|&l| usize::from(l) > MAX_LEVEL)
            || l0_len.iter().any(|&l| usize::from(l) > m0)
        {
            return None;
        }
        for (node, &len) in l0_len.iter().enumerate() {
            let list = &l0[node * m0..node * m0 + usize::from(len)];
            if list.iter().any(|&x| x as usize >= n) {
                return None;
            }
        }
        let layers = levels.iter().map(|&l| usize::from(l)).max().unwrap_or(0);
        // El nivel máximo declarado debe ser el del grafo.
        if layers != max_level {
            return None;
        }
        let mut upper = Vec::with_capacity(layers);
        for layer in 1..=layers {
            let rows = usize::try_from(cursor.u64()?).ok()?;
            let nodes = cursor.u32s(rows)?;
            let len = cursor.take(rows)?.to_vec();
            let adj = cursor.u32s(rows.checked_mul(m)?)?;
            let expected = levels.iter().filter(|&&l| usize::from(l) >= layer).count();
            if rows != expected {
                return None;
            }
            let mut row_of = vec![NO_NODE; n];
            for (row, &node) in nodes.iter().enumerate() {
                if node as usize >= n
                    || usize::from(levels[node as usize]) < layer
                    || row_of[node as usize] != NO_NODE
                {
                    return None;
                }
                row_of[node as usize] = row as u32;
            }
            for (row, &len) in len.iter().enumerate() {
                if usize::from(len) > m {
                    return None;
                }
                let list = &adj[row * m..row * m + usize::from(len)];
                if list
                    .iter()
                    .any(|&x| x as usize >= n || usize::from(levels[x as usize]) < layer)
                {
                    return None;
                }
            }
            upper.push(Upper {
                nodes,
                row_of,
                adj,
                len,
            });
        }
        if cursor.at != raw.len() {
            return None;
        }
        // Las listas de capa 0 solo apuntan a nodos existentes (validado). Las
        // de capas superiores, además, a nodos que llegan a esa capa.
        if n > 0 && usize::from(levels[entry as usize]) != max_level {
            return None;
        }

        let file = std::fs::File::open(data_path).ok()?;
        let expected_bytes = n.checked_mul(dim)?.checked_mul(4)?;
        if usize::try_from(file.metadata().ok()?.len()).ok()? != expected_bytes {
            return None;
        }
        let base = if n == 0 {
            Base::None
        } else {
            map_vectors(&file, expected_bytes)?
        };

        Some(Self {
            dim,
            m,
            m0,
            ef_construction,
            vectors: Vectors {
                dim,
                base,
                base_len: n,
                tail: Vec::new(),
            },
            levels,
            l0,
            l0_len,
            upper,
            entry,
            max_level,
            vectors_dirty: false,
        })
    }
}

#[cfg(not(windows))]
fn map_vectors(file: &std::fs::File, expected_bytes: usize) -> Option<Base> {
    // SAFETY: el fichero se mapea en solo lectura. Si otro proceso lo
    // truncara mientras está mapeado, el acceso fallaría (SIGBUS); HiveDB es el
    // único escritor de este directorio y lo reemplaza por rename, nunca in situ.
    let map = unsafe { memmap2::Mmap::map(file) }.ok()?;
    if map.len() != expected_bytes {
        return None;
    }
    Some(Base::Mapped(map))
}

/// En Windows no se puede renombrar sobre un fichero mapeado, y el siguiente
/// volcado lo hace: allí se lee a memoria.
#[cfg(windows)]
fn map_vectors(file: &std::fs::File, expected_bytes: usize) -> Option<Base> {
    use std::io::Read;
    let mut bytes = Vec::with_capacity(expected_bytes);
    let mut reader = file;
    reader.read_to_end(&mut bytes).ok()?;
    if bytes.len() != expected_bytes {
        return None;
    }
    let values = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Some(Base::Owned(values))
}

fn write_u32s(out: &mut impl Write, values: &[u32]) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(64 * 1024);
    for chunk in values.chunks(16 * 1024) {
        buffer.clear();
        for value in chunk {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        out.write_all(&buffer)?;
    }
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(len)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn u32s(&mut self, count: usize) -> Option<Vec<u32>> {
        let bytes = self.take(count.checked_mul(4)?)?;
        Some(
            bytes
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIM: usize = 24;

    fn rng_vectors(count: usize, clusters: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut state = seed;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        };
        let centers: Vec<Vec<f32>> = (0..clusters)
            .map(|_| (0..DIM).map(|_| next()).collect())
            .collect();
        (0..count)
            .map(|i| {
                centers[i % clusters]
                    .iter()
                    .map(|c| c + 0.4 * next())
                    .collect()
            })
            .collect()
    }

    fn build(vectors: &[Vec<f32>]) -> Graph {
        let mut graph = Graph::new(DIM, 12, 80);
        let refs: Vec<&[f32]> = vectors.iter().map(Vec::as_slice).collect();
        graph.insert_batch(&refs);
        graph
    }

    fn exact(vectors: &[Vec<f32>], query: &[f32], k: usize) -> Vec<u32> {
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let q: Vec<f32> = query.iter().map(|x| x / norm(query)).collect();
        let mut all: Vec<(f32, u32)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let n: Vec<f32> = v.iter().map(|x| x / norm(v)).collect();
                (dist(&q, &n), i as u32)
            })
            .collect();
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        all.into_iter().take(k).map(|(_, i)| i).collect()
    }

    fn recall(graph: &Graph, vectors: &[Vec<f32>], queries: &[Vec<f32>], ef: usize) -> f64 {
        let mut hits = 0;
        for query in queries {
            let truth = exact(vectors, query, 10);
            let found = graph.search(query, ef, 10);
            hits += found.iter().filter(|(id, _)| truth.contains(id)).count();
        }
        hits as f64 / (queries.len() * 10) as f64
    }

    #[test]
    fn alcanza_buen_recall_frente_a_fuerza_bruta() {
        let vectors = rng_vectors(4_000, 20, 7);
        let queries = rng_vectors(60, 20, 99);
        let graph = build(&vectors);
        assert_eq!(graph.len(), 4_000);
        let r = recall(&graph, &vectors, &queries, 500);
        assert!(r >= 0.97, "recall {r}");
    }

    #[test]
    fn la_construccion_es_determinista() {
        let vectors = rng_vectors(1_500, 10, 3);
        let a = build(&vectors);
        let b = build(&vectors);
        let dir = tempfile::tempdir().unwrap();
        a.write_graph(&dir.path().join("a")).unwrap();
        b.write_graph(&dir.path().join("b")).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("a")).unwrap(),
            std::fs::read(dir.path().join("b")).unwrap()
        );
    }

    #[test]
    fn insertar_uno_a_uno_tambien_funciona() {
        let vectors = rng_vectors(800, 8, 5);
        let queries = rng_vectors(40, 8, 11);
        let mut graph = Graph::new(DIM, 12, 80);
        for vector in &vectors {
            graph.insert_batch(&[vector.as_slice()]);
        }
        let r = recall(&graph, &vectors, &queries, 100);
        assert!(r >= 0.95, "recall {r}");
    }

    #[test]
    fn ida_y_vuelta_a_disco_conserva_las_busquedas() {
        let vectors = rng_vectors(1_200, 10, 21);
        let queries = rng_vectors(20, 10, 33);
        let graph = build(&vectors);
        let dir = tempfile::tempdir().unwrap();
        let (gp, dp) = (dir.path().join("g"), dir.path().join("d"));
        graph.write_graph(&gp).unwrap();
        graph.write_vectors(&dp).unwrap();
        let loaded = Graph::read(&gp, &dp, DIM).expect("carga");
        assert!(loaded.vectors_clean());
        for query in &queries {
            assert_eq!(graph.search(query, 80, 10), loaded.search(query, 80, 10));
        }
        // Se puede seguir insertando sobre un grafo cargado.
        let mut loaded = loaded;
        let more = rng_vectors(100, 10, 77);
        let refs: Vec<&[f32]> = more.iter().map(Vec::as_slice).collect();
        loaded.insert_batch(&refs);
        assert_eq!(loaded.len(), 1_300);
        assert!(!loaded.vectors_clean());
        let hit = loaded.search(&more[5], 80, 1);
        assert_eq!(hit[0].0, 1_205);
    }

    #[test]
    fn ficheros_corruptos_no_provocan_panico() {
        let vectors = rng_vectors(600, 6, 41);
        let graph = build(&vectors);
        let dir = tempfile::tempdir().unwrap();
        let (gp, dp) = (dir.path().join("g"), dir.path().join("d"));
        graph.write_graph(&gp).unwrap();
        graph.write_vectors(&dp).unwrap();
        let good = std::fs::read(&gp).unwrap();

        // Truncado y bytes alterados en varias posiciones.
        std::fs::write(&gp, &good[..good.len() / 2]).unwrap();
        assert!(Graph::read(&gp, &dp, DIM).is_none());
        for position in (0..good.len()).step_by(good.len() / 97 + 1) {
            let mut bad = good.clone();
            bad[position] ^= 0xFF;
            std::fs::write(&gp, &bad).unwrap();
            if let Some(loaded) = Graph::read(&gp, &dp, DIM) {
                // Si pasa la validación, buscar no debe entrar en pánico.
                let _ = loaded.search(&vectors[0], 50, 10);
            }
        }
        std::fs::write(&gp, &good).unwrap();
        assert!(Graph::read(&gp, &dp, DIM + 1).is_none());
        std::fs::write(&dp, b"corto").unwrap();
        assert!(Graph::read(&gp, &dp, DIM).is_none());
    }
}
