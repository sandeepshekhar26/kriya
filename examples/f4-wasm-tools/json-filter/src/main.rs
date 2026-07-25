//! F4-wasm (doc 28 §F4) example governed tool #2: a JSON filter.
//!
//! Reads a JSON array of objects from stdin, keeps only the elements whose `--field` equals
//! `--equals`, and writes the filtered array (compact JSON, keys in each object's original order)
//! to stdout. A malformed input or a field/equals mismatch is reported on stderr and exits non-zero
//! — the same "success" flag `kriya-run-wasm` records covers this tool's real failure path, not
//! just its happy path.
//!
//! Usage: json-filter --field <key> --equals <value>   (stdin: a JSON array of objects)

use std::io::{self, Read, Write};

fn parse_args() -> (String, String) {
    let mut field = None;
    let mut equals = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--field" => field = args.next(),
            "--equals" => equals = args.next(),
            _ => {}
        }
    }
    match (field, equals) {
        (Some(f), Some(e)) => (f, e),
        _ => {
            eprintln!("json-filter: usage: json-filter --field <key> --equals <value>");
            std::process::exit(2);
        }
    }
}

fn main() {
    let (field, equals) = parse_args();

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("reading stdin");

    let value: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("json-filter: stdin is not valid JSON: {e}");
            std::process::exit(1);
        }
    };

    let Some(array) = value.as_array() else {
        eprintln!("json-filter: stdin must be a JSON array of objects");
        std::process::exit(1);
    };

    let filtered: Vec<&serde_json::Value> = array
        .iter()
        .filter(|item| {
            item.get(&field)
                .map(|v| match v {
                    serde_json::Value::String(s) => s == &equals,
                    other => other.to_string() == equals,
                })
                .unwrap_or(false)
        })
        .collect();

    let out = serde_json::to_string(&filtered).expect("serializing filtered result");
    io::stdout()
        .write_all(out.as_bytes())
        .expect("writing stdout");
}
