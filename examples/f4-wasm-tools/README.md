# F4-wasm example governed tools (doc 28 §F4)

Two minimal WASI-p2 components that exist to prove out `kriya-run-wasm`'s **Deterministic
Execution** lane — the rail is the product here, not tool coverage. See
`crates/kriya/src/wasmexec/mod.rs`'s module doc for the full rationale and the naming law (this is
re-**EXECUTION**, never "replay" — kriya-console's B1 "Verified Replay" is a different, unrelated
claim about re-deriving a session timeline from receipts).

## Tools

- **`text-transform/`** — reads stdin, applies one transform (`--upper` / `--lower` / `--reverse`),
  plus `--show-clock` / `--show-hash` which deliberately touch `wasi:clocks/wall-clock` and
  `wasi:random/*` (via `std::time::SystemTime::now()` and `RandomState`) to demonstrate that this
  lane's clock/RNG virtualization makes even THOSE calls reproducible, not just plain stdin→stdout
  transforms which would be deterministic on their own regardless.
- **`json-filter/`** — reads a JSON array of objects from stdin, keeps only the elements whose
  `--field` equals `--equals`, writes the filtered array to stdout.

## Build

```sh
cd text-transform && cargo build --release --target wasm32-wasip2
cd ../json-filter  && cargo build --release --target wasm32-wasip2
```

Requires the `wasm32-wasip2` Rust target (`rustup target add wasm32-wasip2`). Produces real
WASI-p2 **components** directly — no `cargo-component`/`wasm-tools` needed, since `rustc`'s
`wasm32-wasip2` target has emitted components natively since it stabilized.

## Run under the deterministic-execution lane

```sh
# from the repo root, after `cargo build --features wasm-exec -p kriya --bin kriya-run-wasm`
BIN=crates/kriya/target/debug/kriya-run-wasm

echo "hello deterministic world" > /tmp/stdin.txt
$BIN run examples/f4-wasm-tools/text-transform/target/wasm32-wasip2/release/text-transform.wasm \
  --arg --upper --stdin-file /tmp/stdin.txt --seed 42 --epoch-ms 1700000000000 \
  --out /tmp/bundle.json --tool-name text-transform

$BIN --verify /tmp/bundle.json
```

`run` records a `kriya-exec-bundle/1` JSON bundle + signs a `kriya.exec.deterministic` receipt.
`--verify` re-executes the SAME recorded inputs and hash-compares stdout/stderr/fuel — exit 0 on a
match, 1 on any divergence (a moved/edited module, a tampered bundle, or a genuinely different
re-execution), with the specific field and reason printed.

## What this does NOT prove

Two runs of the SAME bundle on THIS machine are byte-identical — that's the claim, checked in
`crates/kriya/tests/wasmexec_determinism.rs`. It is **not** a cross-Wasmtime-version guarantee (the
bundle records the exact `wasmtime_version` + a config digest, and `--verify` reports a version
mismatch honestly rather than silently comparing across versions), and it says nothing about any
tool call that did not run through this lane. See `docs/TRUST.md` in the kriya-console repo for the
buyer-facing wording.
