# Full poe-code Markdown suite

All four Markdown test files are copied byte-for-byte from
`poe-code/packages/toolcraft-design/src/terminal-markdown`, including their
parser, terminal, HTML, plaintext, and highlighting assertions: 346 tests.
The 31 supporting source/fixture/license files let the original tests run
independently of a poe-code checkout. `upstream-manifest.json` records the
revision and SHA-256 of every copied file. The two-line package entrypoint and
frontmatter package adapter only resolve imports; they do not replace renderers.

```sh
cd tools/poe-markdown-tests
npm ci --ignore-scripts
npm run corpus
```

This runs every original assertion against the copied reference implementation
and exports all observed inputs. Import instrumentation does not alter the
original files or assertions. The checked-in corpus records every test name,
309 unique Markdown inputs, 31 code inputs, and 46 direct renderer inputs.
Type-only, invalid-option, invalid-frontmatter, and subprocess theme tests retain
their original assertions even where they have no native Markdown input.

The native bridge runs the corpus through hey-boss itself:

```sh
mkdir -p out
HEY_BOSS_NATIVE_CORPUS_OUTPUT="$PWD/out/poe-native-corpus.json" \
  cargo test --locked -p hey-boss --test markdown_poe_suite
xcrun swiftc -g -Onone -parse-as-library -D HEY_BOSS_AUDIT \
  hey_boss_daemon.swift test_hey_boss.swift -o out/hey-boss-test
HEY_BOSS_AUDIT_POE_CORPUS="$PWD/out/poe-native-corpus.json" out/hey-boss-test
```

The Rust checks compare native structure/content with the independent upstream
parser, validate source lines, token/source preservation, and safe destinations.
The AppKit audit renders all 355 document/direct-node inputs, verifies literal
text order and native plain/RTF selection copying. Syntax families not supported
by the lightweight native lexer remain readable code; upstream's complete token
classification assertions still run in the reference suite. ANSI cell widths,
terminal colors, and plaintext announcement options are reference-only behavior,
not assertions about AppKit pixels.

`tests/fixtures/poe-markdown-dialects.json` contains 15 reviewed, exact native
expectations for intentional dialect differences, each with a reason. These
include CommonMark indented code/reference links/code-span spaces, HTTPS defaults
and early URL sanitization. No input is skipped; stale or unused overrides fail.
A dialect override must never be generated automatically to accept a regression.

To refresh the complete upstream copy, run `node import.mjs /path/to/poe-code`,
then run the full suite and review the corpus/dialect changes. `compare.mjs` is a
read-only diagnostic comparison against `HEY_BOSS_CLI_PATH`; it writes mismatches
to `out/poe-mismatches.json`, never updates accepted expectations.

The dedicated Markdown CI workflow runs both suites and rejects stale generated
fixtures. Cargo tests also verify every imported file hash and test inventory.
