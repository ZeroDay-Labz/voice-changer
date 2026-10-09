//! Print the inputs and outputs of ONNX models.
//! `cargo run -p vc-core --features ai --example inspect_onnx -- model.onnx ...`
fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        let session = ort::session::Session::builder()?.commit_from_file(&path)?;
        println!("== {path}");
        for (kind, outlets) in [("input", session.inputs()), ("output", session.outputs())] {
            for o in outlets {
                println!("  {kind:6} {:<16} {:?}", o.name(), o.dtype());
            }
        }
    }
    Ok(())
}
