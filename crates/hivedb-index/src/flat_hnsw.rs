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

use crate::vector_file::View;
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

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
    let (a_chunks, a_tail) = a.as_chunks::<LANES>();
    let (b_chunks, b_tail) = b.as_chunks::<LANES>();
    for (x, y) in a_chunks.iter().zip(b_chunks) {
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

/// Capa superior (≥ 1): solo contiene a los nodos cuyo nivel la alcanza.
struct Upper {
    /// Fila → nodo.
    nodes: Vec<u32>,
    /// Nodo → fila (`NO_NODE` si el nodo no está en esta capa). Una entrada por nodo.
    row_of: Vec<u32>,
    adj: Vec<u32>,
    len: Vec<u8>,
}

/// Observador de la búsqueda. `NoTrace` es un no-op que desaparece al
/// monomorfizar, así que la ruta normal no paga nada por la traza.
pub(crate) trait Recorder {
    /// `node` entró al frente de búsqueda de `layer`, alcanzado desde `from`
    /// (`NO_NODE` para el punto de entrada).
    fn step(&mut self, layer: usize, from: u32, node: u32, distance: f32);
}

pub(crate) struct NoTrace;

impl Recorder for NoTrace {
    #[inline(always)]
    fn step(&mut self, _: usize, _: u32, _: u32, _: f32) {}
}

/// Pasos máximos que guarda una traza (acota memoria con `ef` muy altos).
pub(crate) const MAX_TRACE_STEPS: usize = 1024;

#[derive(Default)]
pub(crate) struct Collect(pub(crate) Vec<(usize, u32, u32, f32)>);

impl Recorder for Collect {
    fn step(&mut self, layer: usize, from: u32, node: u32, distance: f32) {
        if self.0.len() < MAX_TRACE_STEPS {
            self.0.push((layer, from, node, distance));
        }
    }
}

pub(crate) struct Graph {
    dim: usize,
    /// Vecinos máximos en capas superiores; la capa 0 admite `2 * m`.
    m: usize,
    m0: usize,
    ef_construction: usize,
    /// Vectores normalizados, en el fichero plano compartido con el almacén.
    /// El id de cada nodo es su número de ranura.
    view: Arc<View>,
    levels: Vec<u8>,
    l0: Vec<u32>,
    l0_len: Vec<u8>,
    upper: Vec<Upper>,
    entry: u32,
    max_level: usize,
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
    pub(crate) fn new(dim: usize, m: usize, ef_construction: usize, view: Arc<View>) -> Self {
        assert!(m >= 2 && 2 * m <= usize::from(u8::MAX));
        Self {
            dim,
            m,
            m0: 2 * m,
            ef_construction,
            view,
            levels: Vec::new(),
            l0: Vec::new(),
            l0_len: Vec::new(),
            upper: Vec::new(),
            entry: NO_NODE,
            max_level: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.levels.len()
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

    /// Registra un nodo nuevo (su vector ya está en la vista) sin enlazarlo.
    fn add_node(&mut self) -> u32 {
        let id = self.len() as u32;
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

    /// Sigue al fichero de vectores: crea y enlaza un nodo por cada ranura
    /// nueva de `view` (que debe incluir todas las anteriores).
    pub(crate) fn extend(&mut self, view: Arc<View>) {
        let first = self.len() as u32;
        debug_assert!(view.slots() >= self.len());
        self.view = view;
        while self.len() < self.view.slots() {
            self.add_node();
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
    }

    fn greedy(&self, query: &[f32], best: (f32, u32), layer: usize) -> (f32, u32) {
        self.greedy_rec(query, best, layer, &mut NoTrace)
    }

    fn greedy_rec<R: Recorder>(
        &self,
        query: &[f32],
        mut best: (f32, u32),
        layer: usize,
        rec: &mut R,
    ) -> (f32, u32) {
        loop {
            let mut improved = false;
            for &neighbor in self.neighbors(layer, best.1) {
                let d = dist(query, self.view.get(neighbor));
                if d < best.0 {
                    rec.step(layer, best.1, neighbor, d);
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
        self.search_layer_rec(query, entries, ef, layer, &mut NoTrace)
    }

    fn search_layer_rec<R: Recorder>(
        &self,
        query: &[f32],
        entries: &[(f32, u32)],
        ef: usize,
        layer: usize,
        rec: &mut R,
    ) -> Vec<(f32, u32)> {
        let mut visited = Visited::new(self.len());
        let mut candidates: BinaryHeap<Reverse<(Dist, u32)>> = BinaryHeap::new();
        let mut results: BinaryHeap<(Dist, u32)> = BinaryHeap::new();
        for &(d, node) in entries {
            if visited.insert(node) {
                rec.step(layer, NO_NODE, node, d);
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
                let dn = Dist::new(dist(query, self.view.get(neighbor)));
                if results.len() < ef || results.peek().is_some_and(|worst| dn < worst.0) {
                    rec.step(layer, current, neighbor, dn.get());
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
            let vector = self.view.get(node);
            let diverse = selected
                .iter()
                .all(|&(_, other)| dist(vector, self.view.get(other)) > d);
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
        let query = self.view.get(node);
        let level = usize::from(self.levels[node as usize]);
        let top = level.min(self.max_level);
        let mut entry = (dist(query, self.view.get(self.entry)), self.entry);
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
        let vector = self.view.get(node);
        let mut scored: Vec<(f32, u32)> = all
            .iter()
            .map(|&other| (dist(vector, self.view.get(other)), other))
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
    #[cfg(test)]
    pub(crate) fn search(&self, query: &[f32], ef: usize, limit: usize) -> Vec<(u32, f32)> {
        self.search_rec(query, ef, limit, &mut NoTrace)
    }

    pub(crate) fn search_rec<R: Recorder>(
        &self,
        query: &[f32],
        ef: usize,
        limit: usize,
        rec: &mut R,
    ) -> Vec<(u32, f32)> {
        if self.entry == NO_NODE || limit == 0 {
            return Vec::new();
        }
        let norm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        let query: Vec<f32> = if norm > 0.0 {
            query.iter().map(|x| x / norm).collect()
        } else {
            query.to_vec()
        };
        let mut entry = (dist(&query, self.view.get(self.entry)), self.entry);
        rec.step(self.max_level, NO_NODE, entry.1, entry.0);
        for layer in (1..=self.max_level).rev() {
            entry = self.greedy_rec(&query, entry, layer, rec);
        }
        let mut found = self.search_layer_rec(&query, &[entry], ef.max(limit), 0, rec);
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

    /// Carga un grafo. Devuelve `None` ante cualquier incoherencia: el llamador
    /// reconstruye desde los documentos, que son la fuente de verdad.
    pub(crate) fn read(graph_path: &Path, view: Arc<View>, dim: usize) -> Option<Self> {
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

        if view.slots() != n {
            return None;
        }

        Some(Self {
            dim,
            m,
            m0,
            ef_construction,
            view,
            levels,
            l0,
            l0_len,
            upper,
            entry,
            max_level,
        })
    }
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
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector_file::{VectorFile, normalized};

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

    /// Añade los vectores al fichero (normalizados) y publica.
    fn append(file: &VectorFile, vectors: &[Vec<f32>]) {
        let first = file.slots();
        let rows: Vec<Vec<f32>> = vectors.iter().map(|v| normalized(v)).collect();
        let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();
        file.write_rows(first, &refs).unwrap();
        file.publish(first + rows.len()).unwrap();
    }

    fn build(dir: &Path, vectors: &[Vec<f32>]) -> (Graph, VectorFile) {
        let file = VectorFile::open(&dir.join("v.dat"), DIM).unwrap();
        append(&file, vectors);
        let mut graph = Graph::new(DIM, 12, 80, file.view());
        graph.extend(file.view());
        (graph, file)
    }

    fn exact(vectors: &[Vec<f32>], query: &[f32], k: usize) -> Vec<u32> {
        let q = normalized(query);
        let mut all: Vec<(f32, u32)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (dist(&q, &normalized(v)), i as u32))
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
    fn la_traza_no_cambia_el_resultado_y_describe_la_ruta() {
        let dir = tempfile::tempdir().unwrap();
        let vectors = rng_vectors(1_500, 12, 3);
        let (graph, _file) = build(dir.path(), &vectors);
        for query in rng_vectors(20, 12, 5) {
            let plain = graph.search(&query, 120, 10);
            let mut collect = Collect::default();
            let traced = graph.search_rec(&query, 120, 10, &mut collect);
            assert_eq!(plain, traced);
            let steps = collect.0;
            assert!(!steps.is_empty());
            // El primer paso es el punto de entrada, en la capa más alta.
            assert_eq!((steps[0].0, steps[0].1), (graph.max_level, NO_NODE));
            // Las capas nunca suben, y la capa 0 contiene todos los resultados.
            assert!(steps.windows(2).all(|w| w[1].0 <= w[0].0));
            for (node, _) in &traced {
                assert!(steps.iter().any(|s| s.0 == 0 && s.2 == *node));
            }
        }
    }

    #[test]
    fn alcanza_buen_recall_frente_a_fuerza_bruta() {
        let dir = tempfile::tempdir().unwrap();
        let vectors = rng_vectors(4_000, 20, 7);
        let queries = rng_vectors(60, 20, 99);
        let (graph, _file) = build(dir.path(), &vectors);
        assert_eq!(graph.len(), 4_000);
        let r = recall(&graph, &vectors, &queries, 500);
        assert!(r >= 0.97, "recall {r}");
    }

    #[test]
    fn la_construccion_es_determinista() {
        let vectors = rng_vectors(1_500, 10, 3);
        let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, _fa) = build(dir_a.path(), &vectors);
        let (b, _fb) = build(dir_b.path(), &vectors);
        a.write_graph(&dir_a.path().join("a.graph")).unwrap();
        b.write_graph(&dir_b.path().join("b.graph")).unwrap();
        assert_eq!(
            std::fs::read(dir_a.path().join("a.graph")).unwrap(),
            std::fs::read(dir_b.path().join("b.graph")).unwrap()
        );
    }

    #[test]
    fn insertar_uno_a_uno_tambien_funciona() {
        let dir = tempfile::tempdir().unwrap();
        let vectors = rng_vectors(800, 8, 5);
        let queries = rng_vectors(40, 8, 11);
        let file = VectorFile::open(&dir.path().join("v.dat"), DIM).unwrap();
        let mut graph = Graph::new(DIM, 12, 80, file.view());
        for vector in &vectors {
            append(&file, std::slice::from_ref(vector));
            graph.extend(file.view());
        }
        let r = recall(&graph, &vectors, &queries, 500);
        assert!(r >= 0.97, "recall {r}");
    }

    #[test]
    fn ida_y_vuelta_a_disco_conserva_las_busquedas() {
        let dir = tempfile::tempdir().unwrap();
        let vectors = rng_vectors(1_200, 10, 21);
        let queries = rng_vectors(20, 10, 33);
        let (graph, file) = build(dir.path(), &vectors);
        let gp = dir.path().join("g");
        graph.write_graph(&gp).unwrap();
        let mut loaded = Graph::read(&gp, file.view(), DIM).expect("carga");
        for query in &queries {
            assert_eq!(graph.search(query, 80, 10), loaded.search(query, 80, 10));
        }
        // Si el fichero tiene otro número de ranuras, no es válido.
        let short = VectorFile::open(&dir.path().join("corto.dat"), DIM).unwrap();
        assert!(Graph::read(&gp, short.view(), DIM).is_none());

        // Se puede seguir insertando sobre un grafo cargado.
        let more = rng_vectors(100, 10, 77);
        append(&file, &more);
        loaded.extend(file.view());
        assert_eq!(loaded.len(), 1_300);
        let hit = loaded.search(&more[5], 80, 1);
        assert_eq!(hit[0].0, 1_205);
    }

    #[test]
    fn ficheros_corruptos_no_provocan_panico() {
        let dir = tempfile::tempdir().unwrap();
        let vectors = rng_vectors(600, 6, 41);
        let (graph, file) = build(dir.path(), &vectors);
        let gp = dir.path().join("g");
        graph.write_graph(&gp).unwrap();
        let good = std::fs::read(&gp).unwrap();

        // Truncado y bytes alterados en varias posiciones.
        std::fs::write(&gp, &good[..good.len() / 2]).unwrap();
        assert!(Graph::read(&gp, file.view(), DIM).is_none());
        for position in (0..good.len()).step_by(good.len() / 97 + 1) {
            let mut bad = good.clone();
            bad[position] ^= 0xFF;
            std::fs::write(&gp, &bad).unwrap();
            if let Some(loaded) = Graph::read(&gp, file.view(), DIM) {
                // Si pasa la validación, buscar no debe entrar en pánico.
                let _ = loaded.search(&vectors[0], 50, 10);
            }
        }
        std::fs::write(&gp, &good).unwrap();
        assert!(Graph::read(&gp, file.view(), DIM + 1).is_none());
    }
}
