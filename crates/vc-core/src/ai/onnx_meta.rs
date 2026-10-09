//! Tiny ONNX (protobuf) scanner: enough to find the speaker-embedding
//! table of an RVC voice model without loading the whole file into
//! memory. Everything that is not needed is skipped by length.

use anyhow::{Context as _, Result, bail};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

// ModelProto.graph = 7, GraphProto.initializer = 5,
// TensorProto.dims = 1, TensorProto.name = 8.
const MODEL_GRAPH: u64 = 7;
const GRAPH_INITIALIZER: u64 = 5;
const TENSOR_DIMS: u64 = 1;
const TENSOR_NAME: u64 = 8;

struct Reader<R> {
    inner: R,
    pos: u64,
}

impl<R: Read + Seek> Reader<R> {
    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let mut b = [0u8; 1];
            self.inner.read_exact(&mut b).context("truncated varint")?;
            self.pos += 1;
            v |= ((b[0] & 0x7f) as u64) << shift;
            if b[0] & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 63 {
                bail!("varint too long");
            }
        }
    }

    fn skip(&mut self, n: u64) -> Result<()> {
        self.inner.seek(SeekFrom::Current(n as i64))?;
        self.pos += n;
        Ok(())
    }

    /// Skip a field's payload given its wire type.
    fn skip_field(&mut self, wire: u64) -> Result<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.skip(8)?,
            2 => {
                let n = self.varint()?;
                self.skip(n)?;
            }
            5 => self.skip(4)?,
            w => bail!("unsupported wire type {w}"),
        }
        Ok(())
    }
}

/// Dimensions of the first initializer whose name ends with `suffix`
/// (e.g. `emb_g.weight`), or `None` if the model has none.
pub fn initializer_dims(path: &Path, suffix: &str) -> Result<Option<Vec<i64>>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let len = file.metadata()?.len();
    let mut r = Reader {
        inner: BufReader::with_capacity(1 << 16, file),
        pos: 0,
    };
    while r.pos < len {
        let key = r.varint()?;
        let (field, wire) = (key >> 3, key & 7);
        if field == MODEL_GRAPH && wire == 2 {
            let n = r.varint()?;
            let end = r.pos + n;
            while r.pos < end {
                let key = r.varint()?;
                let (field, wire) = (key >> 3, key & 7);
                if field == GRAPH_INITIALIZER && wire == 2 {
                    let n = r.varint()?;
                    let tend = r.pos + n;
                    let mut dims = Vec::new();
                    while r.pos < tend {
                        let key = r.varint()?;
                        let (field, wire) = (key >> 3, key & 7);
                        match (field, wire) {
                            (TENSOR_DIMS, 0) => dims.push(r.varint()? as i64),
                            (TENSOR_DIMS, 2) => {
                                let n = r.varint()?;
                                let pend = r.pos + n;
                                while r.pos < pend {
                                    dims.push(r.varint()? as i64);
                                }
                            }
                            (TENSOR_NAME, 2) => {
                                let n = r.varint()? as usize;
                                let mut buf = vec![0u8; n];
                                r.inner.read_exact(&mut buf)?;
                                r.pos += n as u64;
                                let name = String::from_utf8_lossy(&buf);
                                if name.ends_with(suffix) {
                                    return Ok(Some(dims));
                                }
                            }
                            (_, w) => r.skip_field(w)?,
                        }
                    }
                    // Name came after the dims but did not match: keep scanning.
                    if r.pos != tend {
                        r.skip(tend.saturating_sub(r.pos))?;
                    }
                } else {
                    r.skip_field(wire)?;
                }
            }
            return Ok(None);
        } else {
            r.skip_field(wire)?;
        }
    }
    Ok(None)
}

/// Speakers a voice model can address, from the rows of its `emb_g`
/// embedding table. Stock RVC reserves 109 rows whether or not they are
/// used, so only small, deliberately sized tables are trusted; everything
/// else reports `1` and the user can raise it on the Voices page.
pub const MAX_SPEAKERS: u32 = 16;

pub fn speaker_count(path: &Path) -> u32 {
    match initializer_dims(path, "emb_g.weight") {
        Ok(Some(dims)) if !dims.is_empty() && dims[0] > 1 && dims[0] <= MAX_SPEAKERS as i64 => {
            dims[0] as u32
        }
        Ok(_) => 1,
        Err(e) => {
            log::debug!("speaker probe failed for {}: {e:#}", path.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn field_len(field: u64, payload: &[u8], out: &mut Vec<u8>) {
        varint((field << 3) | 2, out);
        varint(payload.len() as u64, out);
        out.extend_from_slice(payload);
    }
    fn tensor(name: &str, dims: &[i64], raw: &[u8]) -> Vec<u8> {
        let mut t = Vec::new();
        for d in dims {
            varint(TENSOR_DIMS << 3, &mut t);
            varint(*d as u64, &mut t);
        }
        varint(2 << 3, &mut t); // data_type = 1 (float)
        varint(1, &mut t);
        field_len(TENSOR_NAME, name.as_bytes(), &mut t);
        field_len(9, raw, &mut t); // raw_data
        t
    }

    #[test]
    fn finds_the_embedding_table() {
        let mut graph = Vec::new();
        field_len(1, b"node", &mut graph); // some other field first
        field_len(
            GRAPH_INITIALIZER,
            &tensor("dec.conv.weight", &[512, 3], &[0u8; 64]),
            &mut graph,
        );
        field_len(
            GRAPH_INITIALIZER,
            &tensor("model.emb_g.weight", &[2, 256], &[0u8; 2048]),
            &mut graph,
        );
        let mut model = Vec::new();
        varint(1 << 3, &mut model); // ir_version
        varint(9, &mut model);
        field_len(MODEL_GRAPH, &graph, &mut model);
        let tmp = std::env::temp_dir().join(format!("vc_onnx_meta_{}.onnx", std::process::id()));
        std::fs::write(&tmp, &model).unwrap();
        assert_eq!(
            initializer_dims(&tmp, "emb_g.weight").unwrap(),
            Some(vec![2, 256])
        );
        assert_eq!(speaker_count(&tmp), 2);
        assert_eq!(initializer_dims(&tmp, "missing").unwrap(), None);
        let _ = std::fs::remove_file(&tmp);
    }
}
