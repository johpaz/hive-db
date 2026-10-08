/** PCA a 3D con iteración de potencias + deflación. Base fija tras `fit`. */
export class Pca3 {
  mean: Float64Array;
  comps: Float64Array[] = [];
  scale = 1;

  constructor(private dim: number) {
    this.mean = new Float64Array(dim);
  }

  fit(vectors: Float32Array[], extent = 12): void {
    const n = vectors.length;
    const d = this.dim;
    this.mean.fill(0);
    for (const v of vectors) for (let i = 0; i < d; i++) this.mean[i]! += v[i]! / n;
    const X = vectors.map((v) => {
      const x = new Float64Array(d);
      for (let i = 0; i < d; i++) x[i] = v[i]! - this.mean[i]!;
      return x;
    });
    for (let c = 0; c < 3; c++) {
      let w = new Float64Array(d);
      for (let i = 0; i < d; i++) w[i] = Math.sin(i * 12.9898 + c * 78.233);
      for (let it = 0; it < 60; it++) {
        const nw = new Float64Array(d);
        for (const x of X) {
          let dot = 0;
          for (let i = 0; i < d; i++) dot += x[i]! * w[i]!;
          for (let i = 0; i < d; i++) nw[i]! += x[i]! * dot;
        }
        let norm = 0;
        for (let i = 0; i < d; i++) norm += nw[i]! * nw[i]!;
        norm = Math.sqrt(norm) || 1;
        for (let i = 0; i < d; i++) nw[i]! /= norm;
        w = nw;
      }
      this.comps.push(w);
      for (const x of X) {
        let dot = 0;
        for (let i = 0; i < d; i++) dot += x[i]! * w[i]!;
        for (let i = 0; i < d; i++) x[i]! -= dot * w[i]!;
      }
    }
    let maxAbs = 1e-9;
    for (const v of vectors) for (const p of this.raw(v)) maxAbs = Math.max(maxAbs, Math.abs(p));
    this.scale = extent / maxAbs;
  }

  private raw(v: Float32Array): [number, number, number] {
    const out: [number, number, number] = [0, 0, 0];
    for (let c = 0; c < 3; c++) {
      const w = this.comps[c]!;
      let s = 0;
      for (let i = 0; i < this.dim; i++) s += (v[i]! - this.mean[i]!) * w[i]!;
      out[c] = s;
    }
    return out;
  }

  project(v: Float32Array): [number, number, number] {
    const [x, y, z] = this.raw(v);
    return [x * this.scale, y * this.scale, z * this.scale];
  }
}
