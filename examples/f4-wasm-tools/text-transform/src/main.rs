//! F4-wasm (doc 28 §F4) example governed tool #1: a text transform.
//!
//! Reads stdin, applies one transform selected by argv, writes the result to stdout. Deliberately
//! touches the two ambient sources this lane virtualizes (`wasi:clocks/wall-clock` via
//! `std::time::SystemTime::now()`, and `wasi:random/*` via `std::collections::hash_map::RandomState`,
//! which std wasm32-wasip2 seeds from WASI randomness) so a `--show-clock`/`--show-hash` run is a
//! real demonstration that kriya-run-wasm's virtualization makes even THOSE calls reproducible —
//! not just the plain stdin/stdout transform, which would be deterministic on its own regardless.
//!
//! Usage (argv[1] selects the transform; unrecognized/absent defaults to `--upper`):
//!   --upper        uppercase stdin
//!   --lower        lowercase stdin
//!   --reverse      reverse stdin byte-for-byte
//!   --show-clock   print the wall-clock instant `SystemTime::now()` reports (ms since epoch)
//!   --show-hash    print a RandomState-keyed hash of stdin (proves the RNG stream is virtualized)

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "--upper".to_string());

    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .expect("reading stdin");

    let output: Vec<u8> = match mode.as_str() {
        "--upper" => input.iter().map(|b| b.to_ascii_uppercase()).collect(),
        "--lower" => input.iter().map(|b| b.to_ascii_lowercase()).collect(),
        "--reverse" => input.iter().rev().copied().collect(),
        "--show-clock" => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("wall clock before epoch");
            format!("{}\n", now.as_millis()).into_bytes()
        }
        "--show-hash" => {
            let state = RandomState::new();
            let mut hasher = state.build_hasher();
            hasher.write(&input);
            format!("{:x}\n", hasher.finish()).into_bytes()
        }
        other => {
            eprintln!("text-transform: unknown mode {other:?}");
            std::process::exit(1);
        }
    };

    io::stdout().write_all(&output).expect("writing stdout");
}
