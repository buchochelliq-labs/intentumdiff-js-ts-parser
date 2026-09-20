# Partial-signature review evidence

Issue #4: `function f(` → `function g(` previously became two empty program trees.
The binding and native live routes agreed, but were both wrong: the changed name
was hidden. A new Rust regression failed on the original parser output.

The Rust parser now retains ERROR nodes and unnamed missing tokens (represented
as ERROR at their exact zero-width position). Ordinary unnamed punctuation stays
excluded. Valid-source child ordering and path IDs remain stable. This uses the
existing SemanticNode contract; no binding-side parsing or comparison was added.
Core must include the Rust source fallback from core PR #46.

Verification on Rust 1.95.0:

- `cargo test`: 14 passed, including JS/TS/TSX incomplete syntax, valid recovery,
  trivia, determinism, missing-parenthesis position and unique IDs.
- `cargo build --release --target wasm32-wasip2`: passed.
- Actual rebuilt Wasm plus native core/public Python routes: 71 acceptance and
  existing incomplete-source/rename/body-edit regressions passed.
- Rebuilt wheel installed, acceptance run from `/tmp` outside the checkouts: 9 passed.
- Independent agent re-ran all 9 actual-Wasm acceptance cases and reviewed the
  Rust implementation. No blocking findings.

Reproduce the consumer contract with `tests/acceptance_incomplete_source.py` using
an installed Python candidate containing this component and core PR #46. The test
covers default/diagnostics JS, TS and TSX, identical incomplete input, recovery to
valid syntax, and the native live protocol. It deliberately is an acceptance
script rather than part of the parser's standalone Rust unit command.

`cli.txt` is actual stdout from the installed CLI, comparing the adjacent `old.js`
and `new.js`. `cli.svg` and `cli.png` render that recorded output through Rich and
CairoSVG; the terminal frame is generated, not an OS or VS Code screenshot.

Component SHA256:
`ebaec67d0a96354605cc89687fb99cd02896b76c1c5582fbc65bf868e68da1ab`.

Linux development wheel SHA256:
`1f93fab509f8bbc2fade6348f55555a6aaaf5abc36c5281e57e27d79c1ae3248`.
Built with Python 3.12, maturin dev profile, Python plus rebuilt JS/TS components.
This is correctness evidence, not a release artifact or VSIX/platform certification.

The checked-in lockfile was stale against Cargo.toml; it now records the existing
`intentumdiff-plugin-sdk` v0.0.2-beta.1 manifest pin, rather than v0.1.0. No dependency
policy was changed. Registry adoption must pin a reviewed commit plus its verified
component checksum; do not bypass the registry checksum gate. No merge or release.
