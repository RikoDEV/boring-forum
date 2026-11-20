# Fuzz targets

Continuous fuzzing of the parsers that take untrusted input.

Each target asserts a security property, not just the absence of a crash — an escaped XSS payload
or a leaked `[hide]` block fails the run as loudly as a panic does.

| Target | What it covers | Property |
| --- | --- | --- |
| `cook` | the whole Markdown → HTML pipeline | output is inert, stable under re-sanitize, deterministic; no executable links or markup in mentions |
| `sanitize` | the ammonia sanitizer alone | output is inert and idempotent for arbitrary markup, not just comrak's |
| `bbcode` | BBCode → Markdown | converted-then-cooked output is inert; conversion is deterministic |
| `search_query` | the advanced search parser | no panic from a URL; dates and tags are validated, not passed through |
| `hidden` | `[hide]` blocks | hidden text never reaches the public HTML; `strip_hidden` agrees with `split_hidden` |
| `redact` | log redaction | no sensitive query value survives; the path is preserved; idempotent |

## Running them

cargo-fuzz needs a nightly toolchain:

```sh
cargo install cargo-fuzz
rustup toolchain install nightly

# Run from the repository root. One target, until you stop it.
cargo +nightly fuzz run --fuzz-dir fuzz cook

# A fixed budget, as CI runs it.
cargo +nightly fuzz run --fuzz-dir fuzz cook -- -max_total_time=300

# Re-check a crash that was found.
cargo +nightly fuzz run --fuzz-dir fuzz cook fuzz/artifacts/cook/crash-<hash>
```

This directory is its own workspace, so the sanitizer and nightly flags cargo-fuzz needs do not
apply to the main build.

## When a target finds something

1. The input lands in `fuzz/artifacts/<target>/`. Keep it.
2. Add it to the corpus test it belongs to — usually
   `crates/bf-cook/tests/xss_corpus.rs` — so the case is checked on every change from then on,
   not only when the fuzzer happens to regenerate it.
3. Fix the parser or the sanitizer. Never relax the assertion in the target: if the property is
   genuinely wrong, that is a change to what the pipeline guarantees and belongs in the docs
   and in a reviewed pull request.

## Corpus

Seeds are not committed. A useful starting corpus is the payload list in
`crates/bf-cook/tests/xss_corpus.rs` plus a sample of real post bodies from a development
database. `cargo fuzz run` will build its own corpus under `fuzz/corpus/<target>/` as it goes;
keeping that directory between CI runs (as a cache) makes each run start where the last stopped
instead of from nothing.
