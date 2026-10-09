//! RVC feature retrieval: a pure-Rust reader for FAISS `IndexIVFFlat` files
//! (the `.index` that ships with every RVC voice) and the k-NN blending RVC
//! applies to ContentVec features before synthesis. Loading all vectors in
//! memory mirrors RVC's own `big_npy`; search goes through the IVF lists so
//! it costs about a megaflop per frame.

use anyhow::{Context as _, Result, bail};
use std::io::{Cursor, Read};
use std::path::Path;

const FOURCC_IVF_FLAT: &[u8; 4] = b"IwFl";
const FOURCC_FLAT_L2: &[u8; 4] = b"IxF2";
const FOURCC_FLAT_IP: &[u8; 4] = b"IxFI";
const FOURCC_ARRAY_LISTS: &[u8; 4] = b"ilar";
const FOURCC_FULL: &[u8; 4] = b"full";
const FOURCC_SPARSE: &[u8; 4] = b"sprs";

struct Reader<R: Read> {
    r: R,
}

impl<R: Read> Reader<R> {
    fn fourcc(&mut self) -> Result<[u8; 4]> {
        let mut b = [0u8; 4];
        self.r.read_exact(&mut b)?;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8> {
        let mut b = [0u8; 1];
        self.r.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn i32(&mut self) -> Result<i32> {
        let mut b = [0u8; 4];
        self.r.read_exact(&mut b)?;
        Ok(i32::from_le_bytes(b))
    }
    fn f32(&mut self) -> Result<f32> {
        let mut b = [0u8; 4];
        self.r.read_exact(&mut b)?;
        Ok(f32::from_le_bytes(b))
    }
    fn i64(&mut self) -> Result<i64> {
        let mut b = [0u8; 8];
        self.r.read_exact(&mut b)?;
        Ok(i64::from_le_bytes(b))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(self.i64()? as u64)
    }
    /// FAISS `READVECTOR`: u64 count then raw elements.
    #[allow(dead_code)]
    fn vec_u8(&mut self, limit: u64) -> Result<Vec<u8>> {
        let n = self.u64()?;
        if n > limit {
            bail!("vector of {n} bytes exceeds limit {limit}");
        }
        let mut v = vec![0u8; n as usize];
        self.r.read_exact(&mut v)?;
        Ok(v)
    }
    fn vec_u64(&mut self, limit: u64) -> Result<Vec<u64>> {
        let n = self.u64()?;
        if n > limit {
            bail!("vector of {n} elements exceeds limit {limit}");
        }
        let mut v = Vec::with_capacity(n as usize);
        for _ in 0..n {
            v.push(self.u64()?);
        }
        Ok(v)
    }
    fn exact(&mut self, n: usize) -> Result<Vec<u8>> {
        let mut v = vec![0u8; n];
        self.r.read_exact(&mut v)?;
        Ok(v)
    }
}

struct Header {
    d: usize,
    ntotal: usize,
}

fn read_header<R: Read>(r: &mut Reader<R>) -> Result<Header> {
    let d = r.i32()?;
    let ntotal = r.i64()?;
    let _dummy1 = r.i64()?;
    let _dummy2 = r.i64()?;
    let _is_trained = r.u8()?;
    let metric = r.i32()?;
    if metric > 1 {
        let _metric_arg = r.f32()?;
    }
    if !(1..=4096).contains(&d) || ntotal < 0 {
        bail!("implausible index header (d={d}, ntotal={ntotal})");
    }
    Ok(Header {
        d: d as usize,
        ntotal: ntotal as usize,
    })
}

fn bytes_to_f32(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// An RVC retrieval index, fully resident in memory.
pub struct RetrievalIndex {
    pub d: usize,
    pub ntotal: usize,
    nlist: usize,
    nprobe_default: usize,
    /// nlist × d
    centroids: Vec<f32>,
    /// per list: n_i × d
    lists: Vec<Vec<f32>>,
}

impl RetrievalIndex {
    /// Parse a FAISS `IndexIVFFlat` file. Returns an error for any other
    /// index type so the caller can fall back to "no index".
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&data)
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader {
            r: Cursor::new(data),
        };
        let cc = r.fourcc()?;
        if &cc != FOURCC_IVF_FLAT {
            bail!(
                "not an IndexIVFFlat (fourcc {:?})",
                String::from_utf8_lossy(&cc)
            );
        }
        let h = read_header(&mut r)?;
        let nlist = r.u64()? as usize;
        let nprobe = r.u64()? as usize;
        if nlist == 0 || nlist > 1 << 20 {
            bail!("implausible nlist {nlist}");
        }
        // Nested coarse quantizer: a flat index of the centroids.
        let qcc = r.fourcc()?;
        if &qcc != FOURCC_FLAT_L2 && &qcc != FOURCC_FLAT_IP {
            bail!(
                "unexpected quantizer type {:?}",
                String::from_utf8_lossy(&qcc)
            );
        }
        let qh = read_header(&mut r)?;
        // Older FAISS stored the flat vectors as vector<float> (count in
        // floats); newer versions as vector<uint8> codes (count in bytes).
        let count = r.u64()? as usize;
        let want = qh.ntotal * h.d;
        let codes = if count == want * 4 {
            r.exact(count)?
        } else if count == want {
            r.exact(count * 4)?
        } else {
            bail!(
                "quantizer shape mismatch ({count} elements for {} centroids of d={})",
                qh.ntotal,
                h.d
            );
        };
        if qh.d != h.d || qh.ntotal != nlist {
            bail!(
                "quantizer mismatch (d {} vs {}, {} centroids for nlist {nlist})",
                qh.d,
                h.d,
                qh.ntotal
            );
        }
        let centroids = bytes_to_f32(&codes);
        // Direct map.
        let dm_type = r.u8()?;
        let _array = r.vec_u64(1 << 32)?;
        if dm_type == 2 {
            let n = r.u64()?;
            let _ = r.exact((n as usize) * 16)?;
        }
        // Inverted lists.
        let lcc = r.fourcc()?;
        if &lcc != FOURCC_ARRAY_LISTS {
            bail!(
                "unsupported inverted list storage {:?}",
                String::from_utf8_lossy(&lcc)
            );
        }
        let l_nlist = r.u64()? as usize;
        let code_size = r.u64()? as usize;
        if l_nlist != nlist || code_size != h.d * 4 {
            bail!("inverted list header mismatch (nlist {l_nlist}, code_size {code_size})");
        }
        let kind = r.fourcc()?;
        let sizes: Vec<u64> = if &kind == FOURCC_FULL {
            r.vec_u64(1 << 24)?
        } else if &kind == FOURCC_SPARSE {
            let pairs = r.vec_u64(1 << 25)?;
            let mut s = vec![0u64; nlist];
            for p in pairs.as_chunks::<2>().0 {
                if (p[0] as usize) < nlist {
                    s[p[0] as usize] = p[1];
                }
            }
            s
        } else {
            bail!("unknown list size encoding");
        };
        if sizes.len() != nlist {
            bail!("sizes vector has {} entries for {nlist} lists", sizes.len());
        }
        let mut lists = Vec::with_capacity(nlist);
        let mut total = 0usize;
        for &n in &sizes {
            let n = n as usize;
            if n == 0 {
                lists.push(Vec::new());
                continue;
            }
            let codes = r.exact(n * code_size)?;
            let _ids = r.exact(n * 8)?;
            lists.push(bytes_to_f32(&codes));
            total += n;
        }
        if total != h.ntotal {
            bail!("lists hold {total} vectors but header says {}", h.ntotal);
        }
        Ok(Self {
            d: h.d,
            ntotal: h.ntotal,
            nlist,
            nprobe_default: nprobe.max(1),
            centroids,
            lists,
        })
    }

    /// The `i`-th stored vector (list order), for diagnostics.
    pub fn sample_vector(&self, mut i: usize) -> Option<Vec<f32>> {
        for l in &self.lists {
            let n = l.len() / self.d.max(1);
            if i < n {
                return Some(l[i * self.d..(i + 1) * self.d].to_vec());
            }
            i -= n;
        }
        None
    }

    pub fn memory_bytes(&self) -> usize {
        (self.centroids.len() + self.lists.iter().map(|l| l.len()).sum::<usize>()) * 4
    }

    /// Blend `feats` (frames × d, row-major) toward the index: for each
    /// frame, the k nearest stored vectors weighted by 1/d², mixed in with
    /// `rate` (RVC's `index_rate`). `k` = 8 and `nprobe` = 1 are RVC's defaults.
    pub fn blend(&self, feats: &mut [f32], rate: f32, nprobe: usize, k: usize) {
        if rate <= 0.0 || feats.is_empty() || self.ntotal == 0 {
            return;
        }
        let d = self.d;
        let nprobe = nprobe
            .max(1)
            .min(self.nlist)
            .max(self.nprobe_default.min(self.nlist));
        let mut probe: Vec<(f32, usize)> = Vec::with_capacity(self.nlist);
        let mut best: Vec<(f32, usize, usize)> = Vec::with_capacity(k + 1);
        let mut blend = vec![0.0f32; d];
        for frame in feats.chunks_exact_mut(d) {
            // Coarse quantizer: nearest centroids.
            probe.clear();
            for (ci, c) in self.centroids.chunks_exact(d).enumerate() {
                probe.push((l2(frame, c), ci));
            }
            probe.sort_unstable_by(|a, b| {
                a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            // k nearest within the probed lists.
            best.clear();
            for &(_, li) in probe.iter().take(nprobe) {
                for (vi, v) in self.lists[li].chunks_exact(d).enumerate() {
                    let dist = l2(frame, v);
                    if best.len() < k {
                        best.push((dist, li, vi));
                        best.sort_unstable_by(|a, b| {
                            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                        });
                    } else if dist < best[k - 1].0 {
                        best[k - 1] = (dist, li, vi);
                        best.sort_unstable_by(|a, b| {
                            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                }
            }
            if best.is_empty() {
                continue;
            }
            // RVC: weight = (1/score)^2 with score = squared L2 distance.
            let mut wsum = 0.0f32;
            blend.fill(0.0);
            for &(dist, li, vi) in &best {
                let w = 1.0 / (dist.max(1e-6) * dist.max(1e-6));
                wsum += w;
                let v = &self.lists[li][vi * d..(vi + 1) * d];
                for (b, x) in blend.iter_mut().zip(v) {
                    *b += w * x;
                }
            }
            if wsum > 0.0 {
                for (f, b) in frame.iter_mut().zip(&blend) {
                    *f = rate * (b / wsum) + (1.0 - rate) * *f;
                }
            }
        }
    }
}

#[inline]
fn l2(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        let e = x - y;
        acc += e * e;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a tiny IndexIVFFlat in FAISS's layout.
    fn write_index(d: usize, centroids: &[Vec<f32>], lists: &[Vec<Vec<f32>>]) -> Vec<u8> {
        let mut out = Vec::new();
        let ntotal: usize = lists.iter().map(|l| l.len()).sum();
        let header = |out: &mut Vec<u8>, ntotal: usize| {
            out.extend_from_slice(&(d as i32).to_le_bytes());
            out.extend_from_slice(&(ntotal as i64).to_le_bytes());
            out.extend_from_slice(&0i64.to_le_bytes());
            out.extend_from_slice(&0i64.to_le_bytes());
            out.push(1);
            out.extend_from_slice(&1i32.to_le_bytes());
        };
        out.extend_from_slice(b"IwFl");
        header(&mut out, ntotal);
        out.extend_from_slice(&(centroids.len() as u64).to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(b"IxF2");
        header(&mut out, centroids.len());
        out.extend_from_slice(&((centroids.len() * d * 4) as u64).to_le_bytes());
        for c in centroids {
            for v in c {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.push(0); // direct map: none
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(b"ilar");
        out.extend_from_slice(&(centroids.len() as u64).to_le_bytes());
        out.extend_from_slice(&((d * 4) as u64).to_le_bytes());
        out.extend_from_slice(b"full");
        out.extend_from_slice(&(lists.len() as u64).to_le_bytes());
        for l in lists {
            out.extend_from_slice(&(l.len() as u64).to_le_bytes());
        }
        let mut id = 0i64;
        for l in lists {
            for v in l {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            for _ in l {
                out.extend_from_slice(&id.to_le_bytes());
                id += 1;
            }
        }
        out
    }

    #[test]
    fn parses_and_blends() {
        let d = 4;
        let centroids = vec![vec![0.0, 0.0, 0.0, 0.0], vec![10.0, 10.0, 10.0, 10.0]];
        let lists = vec![
            vec![
                vec![0.1, 0.0, 0.0, 0.0],
                vec![0.0, 0.2, 0.0, 0.0],
                vec![0.0, 0.0, 0.3, 0.0],
            ],
            vec![vec![10.1, 10.0, 10.0, 10.0], vec![9.9, 10.0, 10.0, 10.0]],
        ];
        let bytes = write_index(d, &centroids, &lists);
        let idx = RetrievalIndex::parse(&bytes).expect("parse");
        assert_eq!(idx.d, 4);
        assert_eq!(idx.ntotal, 5);
        assert_eq!(idx.nlist, 2);

        // A vector sitting on a stored one is pulled onto it fully at rate 1.
        let mut feats = vec![0.1, 0.0, 0.0, 0.0, 10.05, 10.0, 10.0, 10.0];
        idx.blend(&mut feats, 1.0, 1, 8);
        assert!(
            (feats[0] - 0.1).abs() < 1e-3 && feats[1].abs() < 1e-3,
            "{:?}",
            &feats[..4]
        );
        // Second frame: between the two far vectors, stays near 10.
        assert!((feats[4] - 10.0).abs() < 0.2, "{:?}", &feats[4..]);
        // Rate 0 leaves features untouched.
        let mut f2 = vec![5.0; 4];
        idx.blend(&mut f2, 0.0, 1, 8);
        assert_eq!(f2, vec![5.0; 4]);
    }

    #[test]
    fn rejects_other_index_types() {
        let bytes = b"IxF2\0\0\0\0";
        assert!(RetrievalIndex::parse(bytes).is_err());
    }
}
