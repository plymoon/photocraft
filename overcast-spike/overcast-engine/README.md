# overcast-engine (spike)

Overcast's non-destructive layer stack (fill / paint / Hue-Sat / Levels layers, stroke-built masks,
strokes stored as UV-space data) on PhotoCraft's `raster`, `paint`, `doc` and `compose` crates,
depended on by path (`../../crates/*`). See [`../REPORT.md`](../REPORT.md) for the evaluation.

```sh
cargo test                                             # 13 tests
cargo run --release --example demo -- out              # 2048² stack -> out/stack_cpu{,16}.png
cargo run --release --features gpu --example demo -- out   # + GPU compositor when an adapter exists
cargo check --target wasm32-unknown-unknown -p overcast-engine

cd py && python -m venv .venv && . .venv/bin/activate && pip install maturin
maturin develop --release && python demo.py out        # composite() / stroke() / Stack from Python
```

Stack JSON (`StackSpec`), UV units by default:

```json
{"depth": "u16", "layers": [
  {"type": "fill", "color": [0.85, 0.55, 0.35, 1]},
  {"type": "paint", "strokes": [
    {"size": 0.09, "hardness": 0, "color": [0.1, 0.25, 0.75, 1], "points": [[0.1, 0.3, 1], [0.9, 0.3, 1]]},
    {"size": 0.06, "tip": {"shape": "square"}, "angle": 30, "spacing": 1.4, "points": [[0.1, 0.6, 1], [0.9, 0.9, 1]]}]},
  {"type": "hue_saturation", "hue": 150, "saturation": 40,
   "mask": [{"size": 0.3, "hardness": 0.2, "color": [1, 1, 1, 1], "points": [[0.7, 0.1, 1], [0.7, 0.9, 1]]}]}]}
```
