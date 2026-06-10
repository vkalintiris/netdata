# nlogql fuzz harness

Two targets backed by `cargo-fuzz` / `libfuzzer-sys`:

- `parse_no_panic` — property: any `&str` either parses or returns
  `Err`. No panics, no unbounded recursion, no infinite loops.
- `roundtrip` — property: for any input that parses, the
  `Display`-rendered canonical form re-parses to an AST whose
  `Display` is byte-equal to the canonical form.

## Setup

```sh
rustup install nightly                  # cargo-fuzz needs nightly
cargo install cargo-fuzz                # one-off
```

## Run

```sh
cd src/crates/nlogql/fuzz
cargo +nightly fuzz run parse_no_panic
# or
cargo +nightly fuzz run roundtrip
```

`cargo-fuzz` runs until you Ctrl-C or it finds a panic. Crash
artifacts land in `artifacts/<target>/`.

## Notes

The `fuzz/` directory is **not** a member of the parent Cargo
workspace — `fuzz/Cargo.toml` sets its own empty `[workspace]` so
that `libfuzzer-sys` and friends don't pollute the main build.
