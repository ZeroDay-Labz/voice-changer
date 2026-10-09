#!/usr/bin/env python3
"""Export an RVC (v1/v2) .pth voice model to the ONNX layout this app loads.

Uses the official RVC WebUI model code (MIT), fetched into the tools folder,
so weights load exactly. Inputs: phone, phone_lengths, pitch, pitchf, ds, rnd.
"""
import argparse, os, subprocess, sys, json

TOOLS = os.environ.get("VC_TOOLS", os.path.expanduser("~/.local/share/voice-changer/tools"))
RVC_DIR = os.path.join(TOOLS, "rvc-webui")
RVC_REPO = "https://github.com/RVC-Project/Retrieval-based-Voice-Conversion-WebUI"


def ensure_rvc():
    if not os.path.isfile(os.path.join(RVC_DIR, "infer", "module", "models.py")):
        print(">> fetching RVC model code", flush=True)
        subprocess.check_call(["git", "clone", "--depth", "1", RVC_REPO, RVC_DIR])
    sys.path.insert(0, RVC_DIR)


def fix_mixed_types(path):
    """The dynamo exporter occasionally emits float*int64 arithmetic that
    ONNX Runtime rejects; insert the missing casts."""
    import onnx
    from onnx import TensorProto, helper, shape_inference

    m = shape_inference.infer_shapes(onnx.load(path), data_prop=True)
    g = m.graph
    types = {}
    for vi in list(g.value_info) + list(g.input) + list(g.output):
        if vi.type.HasField("tensor_type"):
            types[vi.name] = vi.type.tensor_type.elem_type
    for init in g.initializer:
        types[init.name] = init.data_type
    fixed = 0
    nodes = []
    for node in g.node:
        if node.op_type in ("Mul", "Add", "Sub", "Div") and len(node.input) == 2:
            ta, tb = types.get(node.input[0]), types.get(node.input[1])
            if ta and tb and ta != tb and TensorProto.FLOAT in (ta, tb):
                for i, ty in ((0, ta), (1, tb)):
                    if ty != TensorProto.FLOAT:
                        out = node.input[i] + "_as_float"
                        nodes.append(helper.make_node("Cast", [node.input[i]], [out], to=TensorProto.FLOAT, name=f"{node.name}_cast{i}"))
                        node.input[i] = out
                        fixed += 1
        nodes.append(node)
    del g.node[:]
    g.node.extend(nodes)
    onnx.save(m, path)
    if fixed:
        print(f">> patched {fixed} mixed-type op(s)", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pth")
    ap.add_argument("onnx")
    args = ap.parse_args()
    ensure_rvc()
    import torch
    from torch import nn
    from infer.module.models import SynthesizerTrnMs256NSFsid, SynthesizerTrnMs768NSFsid

    cpt = torch.load(args.pth, map_location="cpu", weights_only=False)
    cpt["config"][-3] = cpt["weight"]["emb_g.weight"].shape[0]  # n_spk
    version = cpt.get("version", "v1")
    sr = cpt["config"][-1]
    if isinstance(sr, str):
        sr = int(sr.rstrip("k")) * 1000
    vec_channels = 256 if version == "v1" else 768
    print(f">> {os.path.basename(args.pth)}: {version}, {sr} Hz, {cpt['config'][-3]} speaker(s)", flush=True)

    cls = SynthesizerTrnMs768NSFsid if version == "v2" else SynthesizerTrnMs256NSFsid
    net_g = cls(*cpt["config"], is_half=False)
    missing, unexpected = net_g.load_state_dict(cpt["weight"], strict=False)
    # enc_q (posterior encoder) is training-only and absent from the ONNX graph.
    bad = [k for k in missing if not k.startswith("enc_q.")]
    if bad or unexpected:
        print(f"!! weight mismatch: missing={bad[:5]} unexpected={list(unexpected)[:5]}", file=sys.stderr)
        sys.exit(2)
    net_g.eval()

    class Export(nn.Module):
        """The inference graph with the noise as an explicit input (the
        layout the classic RVC ONNX export used and this app loads)."""

        def __init__(self, net):
            super().__init__()
            self.net = net

        def forward(self, phone, phone_lengths, pitch, nsff0, sid, rnd):
            net = self.net
            g = net.emb_g(sid).unsqueeze(-1)
            m_p, logs_p, x_mask = net.enc_p(phone, pitch, phone_lengths)
            z_p = (m_p + torch.exp(logs_p) * rnd * 0.66666) * x_mask
            z = net.flow(z_p, x_mask, g=g, reverse=True)
            return net.dec(z * x_mask, nsff0, g=g)

    model = Export(net_g).eval()

    # The upstream relative-attention helpers call int() on the sequence
    # length, which pins the exported graph to the trace length. Swap in
    # shape-symbolic versions for the export.
    from torch.nn import functional as F
    from infer.module import attentions

    def rel_to_abs(self, x):
        batch, heads, length, _ = x.size()
        x = F.pad(x, [0, 1, 0, 0, 0, 0, 0, 0])
        x_flat = x.view([batch, heads, length * 2 * length])
        x_flat = F.pad(x_flat, [0, length - 1, 0, 0, 0, 0])
        x_final = x_flat.view([batch, heads, length + 1, 2 * length - 1])[:, :, :length, length - 1 :]
        return x_final

    def abs_to_rel(self, x):
        batch, heads, length, _ = x.size()
        x = F.pad(x, [0, length - 1, 0, 0, 0, 0, 0, 0])
        x_flat = x.view([batch, heads, length * length + length * (length - 1)])
        x_flat = F.pad(x_flat, [length, 0, 0, 0, 0, 0])
        x_final = x_flat.view([batch, heads, length, 2 * length])[:, :, :, 1:]
        return x_final

    attentions.MultiHeadAttention._relative_position_to_absolute_position = rel_to_abs
    attentions.MultiHeadAttention._absolute_position_to_relative_position = abs_to_rel

    T = 200
    inputs = (
        torch.rand(1, T, vec_channels),
        torch.tensor([T]).long(),
        torch.randint(5, 255, (1, T)),
        torch.rand(1, T),
        torch.LongTensor([0]),
        torch.rand(1, 192, T),
    )
    names = ["phone", "phone_lengths", "pitch", "pitchf", "ds", "rnd"]
    t = torch.export.Dim("t", min=16, max=6000)
    dyn = {"phone": {1: t}, "phone_lengths": None, "pitch": {1: t}, "nsff0": {1: t}, "sid": None, "rnd": {2: t}}
    with torch.no_grad():
        prog = torch.onnx.export(
            model,
            inputs,
            dynamo=True,
            dynamic_shapes=dyn,
            input_names=names,
            output_names=["audio"],
            report=False,
        )
    prog.save(args.onnx)
    fix_mixed_types(args.onnx)
    # Record the sample rate alongside (the app also detects it from the output).
    with open(os.path.splitext(args.onnx)[0] + ".json", "w") as f:
        json.dump({"sample_rate": sr, "version": version, "source": os.path.basename(args.pth)}, f)
    print(f">> wrote {args.onnx} ({os.path.getsize(args.onnx) // 1_000_000} MB)", flush=True)


if __name__ == "__main__":
    main()
