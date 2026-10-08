use std::collections::HashMap;

/// Reciprocal Rank Fusion.
///
/// For every document present in one or more rankings, computes
/// `score(d) = sum_i 1 / (k + rank_i(d))`, where `rank_i(d)` is the 1-based
/// position of `d` in ranking `i`. Results are returned sorted by score
/// descending.
pub fn rrf(rankings: &[Vec<(String, usize)>], k: usize) -> Vec<(String, f32)> {
    // (puntuación, mejor puesto en cualquiera de las listas)
    let mut scores: HashMap<String, (f32, usize)> = HashMap::new();

    for ranking in rankings {
        for (id, rank) in ranking {
            let contribution = 1.0 / (k as f32 + *rank as f32);
            let entry = scores.entry(id.clone()).or_insert((0.0, usize::MAX));
            entry.0 += contribution;
            entry.1 = entry.1.min(*rank);
        }
    }

    let mut results: Vec<(String, (f32, usize))> = scores.into_iter().collect();
    // Orden total y reproducible: con la misma entrada, siempre la misma salida. Los
    // empates (frecuentes en RRF, que solo mira posiciones) se resuelven por el mejor
    // puesto que tenga el documento en alguna lista y, al final, por id.
    results.sort_by(|a, b| {
        b.1.0
            .total_cmp(&a.1.0)
            .then(a.1.1.cmp(&b.1.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    results
        .into_iter()
        .map(|(id, (score, _))| (id, score))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_fusion_matches_reference() {
        let bm25 = vec![
            ("a".to_string(), 1),
            ("b".to_string(), 2),
            ("c".to_string(), 3),
        ];
        let ann = vec![
            ("b".to_string(), 1),
            ("c".to_string(), 2),
            ("a".to_string(), 3),
        ];
        let fused = rrf(&[bm25, ann], 60);
        assert_eq!(fused[0].0, "b");
    }

    #[test]
    fn los_empates_se_resuelven_siempre_igual() {
        // "x" es primero en texto y "y" primero en vector: misma puntuación exacta.
        let text = vec![("x".to_string(), 1)];
        let vector = vec![("y".to_string(), 1)];
        for _ in 0..200 {
            let fused = rrf(&[text.clone(), vector.clone()], 60);
            assert_eq!(fused[0].1, fused[1].1);
            assert_eq!(fused[0].0, "x");
            assert_eq!(fused[1].0, "y");
        }
    }

    #[test]
    fn a_igual_puntuacion_gana_el_mejor_puesto() {
        // "p" suma 1/62 + 1/62 (puestos 2 y 2); "q" suma 1/61 + 1/63 (puestos 1 y 3): casi
        // igual, pero distinto; con dos documentos de puntuación idéntica el de mejor puesto gana.
        let a = vec![("m".to_string(), 1), ("n".to_string(), 3)];
        let b = vec![("n".to_string(), 1), ("m".to_string(), 3)];
        let fused = rrf(&[a, b], 60);
        assert_eq!(fused[0].1, fused[1].1);
        assert_eq!(fused[0].0, "m");
    }
}
