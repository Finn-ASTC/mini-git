# linestat 1.0.0 — Independent Verification Report

| | |
|---|---|
| Contract | `SPEC.md` inside snapshot `snapshot-nvn21yva` (sha256 `20903f07…98244`) |
| SNAPSHOT/source | `/home/user/Projects/mini-git/.orch/waves/W5/orch-run-2lpx81iy/rounds/agent-orchestrator-ggyoe35m/deliveries/snapshot-nvn21yva/source` |
| cwd / attempt | `/tmp/w5-verify` |
| Work copy | `work/linestat` (copied from snapshot; built here; author `target/` NOT reused) |
| Oracle (truth) | `oracle/linestat_oracle.py` — transcribed from SPEC only; never consults `src/main.rs`/`tests/cli.rs` |
| Differential harness | `harness/differential.py` — 36 cases |

## 1. Verdicts (kept separate)

**Product verdict: PASS (with documented underdetermined areas).**
The built `target/release/linestat` behaviour matched the independent oracle on **36/36** differential cases; dev + release `cargo test` and release build all exited 0; `cargo fmt --check` and `clippy -D warnings` are clean.

**Protocol status: success.** This is a read-only verification; result written to the contract `result_path`. No SPEC self-contradiction or git/wc conflict was found, so `blocked` is not used.

## 2. Environment

| item | value |
|---|---|
| host / user | linux, uid 1001 (non-root: EACCES tests are real) |
| cargo / rustc | 1.97.1 / 1.97.1 |
| python | 3.14.7 |
| network | never used (`--offline`; crate has no third-party deps) |

## 3. Config matrix (per-config, not a single total)

| config | command | exit | result |
|---|---|---|---|
| dev | `cargo test --offline` | 0 | 11 integration tests pass (author suite — supplement only) |
| release build | `cargo build --release --offline` | 0 | `target/release/linestat` produced |
| release test | `cargo test --release --offline` | 0 | 11 integration tests pass (author suite — supplement only) |
| fmt (dev) | `cargo fmt --all -- --check` | 0 | clean |
| clippy (dev) | `cargo clippy --offline --all-targets -- -D warnings` | 0 | clean |
| release artifact behaviour | `harness/differential.py` on `target/release/linestat` | 0 | 36/36 cases match oracle |
| release artifact (supplement) | same harness on `target/debug/linestat` | 0 | 36/36 cases match oracle |

Actual commands + captured exit codes (raw logs): `evidence/cargo-test-dev.log`, `evidence/cargo-build-release.log`, `evidence/cargo-test-release.log`, `evidence/cargo-fmt.log`, `evidence/cargo-clippy.log`.

**Release artifact (actual deliverable):**
`/tmp/w5-verify/work/linestat/target/release/linestat`
SHA-256 `759534809958403271ec37bba8962439ab51938999bdd214214b6bff3ea44f5e`

## 4. Differential fixture matrix (oracle vs actual)

Every case compares raw stdout bytes, stderr, and exit code. Full expected/actual stdout+stderr+exit per case: `evidence/differential-report.json` and `evidence/differential-report.txt`.

| # | case | class | exit exp/act | stdout | stderr | verdict |
|---|---|---|---|---|---|---|
| 1 | `version_alone` | --version | 0/0 | == | ok | PASS |
| 2 | `version_with_missing_file` | --version | 0/0 | == | ok | PASS |
| 3 | `single_file` | counting | 0/0 | == | ok | PASS |
| 4 | `single_no_trailing_newline` | counting | 0/0 | == | ok | PASS |
| 5 | `empty_file` | counting | 0/0 | == | ok | PASS |
| 6 | `multi_files_total` | multi-file/total | 0/0 | == | ok | PASS |
| 7 | `multi_files_total_reverse` | multi-file/total | 0/0 | == | ok | PASS |
| 8 | `cr_tab_vt_ff` | counting | 0/0 | == | ok | PASS |
| 9 | `non_utf8_content` | counting | 0/0 | == | ok | PASS |
| 10 | `missing_file` | error path | 1/1 | == | ok | PASS |
| 11 | `missing_then_good` | error path | 1/1 | == | ok | PASS |
| 12 | `good_then_missing` | multi-file/total | 1/1 | == | ok | PASS |
| 13 | `unknown_long_option` | usage errors | 2/2 | == | ok | PASS |
| 14 | `unknown_short_option` | usage errors | 2/2 | == | ok | PASS |
| 15 | `unknown_option_with_file` | usage errors | 2/2 | == | ok | PASS |
| 16 | `no_args` | usage errors | 2/2 | == | ok | PASS |
| 17 | `directory_operand` | error path | 1/1 | == | ok | PASS |
| 18 | `non_utf8_path` | counting | 0/0 | == | ok | PASS |
| 19 | `only_newlines` | counting | 0/0 | == | ok | PASS |
| 20 | `only_cr_no_newline` | counting | 0/0 | == | ok | PASS |
| 21 | `whitespace_only` | counting | 0/0 | == | ok | PASS |
| 22 | `nul_bytes` | counting | 0/0 | == | ok | PASS |
| 23 | `permission_denied` | error path | 1/1 | == | ok | PASS |
| 24 | `permission_denied_then_good` | error path | 1/1 | == | ok | PASS |
| 25 | `unicode_ideographic_space` | counting | 0/0 | == | ok | PASS |
| 26 | `unicode_nbsp` | counting | 0/0 | == | ok | PASS |
| 27 | `mixed_unicode_and_ascii_ws` | counting | 0/0 | == | ok | PASS |
| 28 | `multi_non_utf8_total` | multi-file/total | 0/0 | == | ok | PASS |
| 29 | `path_echo_no_normalisation` | counting | 0/0 | == | ok | PASS |
| 30 | `many_bytes_lines` | counting | 0/0 | == | ok | PASS |
| 31 | `boundary_word_split` | 64KiB boundary | 0/0 | == | ok | PASS |
| 32 | `boundary_sep_then_word` | 64KiB boundary | 0/0 | == | ok | PASS |
| 33 | `boundary_newline_then_word` | 64KiB boundary | 0/0 | == | ok | PASS |
| 34 | `boundary_word_then_sep` | 64KiB boundary | 0/0 | == | ok | PASS |
| 35 | `boundary_two_chunks` | 64KiB boundary | 0/0 | == | ok | PASS |
| 36 | `boundary_all_ws_then_word` | 64KiB boundary | 0/0 | == | ok | PASS |

### 4.1 Required coverage — explicit per-item evidence

| required item | case | actual stdout | actual exit | stderr |
|---|---|---|---|---|
| version_alone | `version_alone` | `'linestat 1.0.0\n'` | 0 | `''` |
| single_file | `single_file` | `'1 2 12 /tmp/w5-verify/evidence/fixtures-release/single_file/one.txt\n'` | 0 | `''` |
| multi_files_total | `multi_files_total` | `'1 1 6 /tmp/w5-verify/evidence/fixtures-release/multi_files_total/b.txt\n1 2 8 /tmp/w5-verify/evidence/fixtures-release/multi_files_total/a.txt\n2 3 14 total\n'` | 0 | `''` |
| empty_file | `empty_file` | `'0 0 0 /tmp/w5-verify/evidence/fixtures-release/empty_file/empty.txt\n'` | 0 | `''` |
| single_no_trailing_newline | `single_no_trailing_newline` | `'3 3 5 /tmp/w5-verify/evidence/fixtures-release/single_no_trailing_newline/nonl.txt\n'` | 0 | `''` |
| cr_tab_vt_ff | `cr_tab_vt_ff` | `'1 4 9 /tmp/w5-verify/evidence/fixtures-release/cr_tab_vt_ff/ws.txt\n'` | 0 | `''` |
| non_utf8_content | `non_utf8_content` | `'1 2 4 /tmp/w5-verify/evidence/fixtures-release/non_utf8_content/binary.bin\n'` | 0 | `''` |
| boundary_word_split | `boundary_word_split` | `'1 1 65537 /tmp/w5-verify/evidence/fixtures-release/boundary_word_split/boundary_word_split.bin\n'` | 0 | `''` |
| missing_file | `missing_file` | `''` | 1 | `'linestat: /tmp/w5-verify/evidence/fixtures-release/missing_file/missing.txt: No such file or directory (os error 2)\n'` |
| missing_then_good | `missing_then_good` | `'1 1 4 /tmp/w5-verify/evidence/fixtures-release/missing_then_good/ok.txt\n1 1 4 total\n'` | 1 | `'linestat: /tmp/w5-verify/evidence/fixtures-release/missing_then_good/missing.txt: No such file or directory (os error 2)\n'` |
| unknown_long_option | `unknown_long_option` | `''` | 2 | `'linestat: usage: linestat [--version] [FILE...]\n'` |
| no_args | `no_args` | `''` | 2 | `'linestat: usage: linestat [--version] [FILE...]\n'` |

Additional boundary cases: `boundary_word_split`, `boundary_sep_then_word`, `boundary_newline_then_word`, `boundary_word_then_sep`, `boundary_two_chunks`, `boundary_all_ws_then_word` — all PASS (word/line counting persists across the 64 KiB read buffer).

### 4.2 SPEC != wc (oracle is not wc)

`evidence/probes/probes.txt`: file `abc` (no trailing newline) → linestat reports `1 1 3` while `wc -l` reports `0`. The oracle follows SPEC (`+1` for a non-empty file not ending in `\n`), not `wc`.

## 5. Underdetermined by SPEC (recorded, NOT scored as pass/fail)

| area | SPEC silence | observed behaviour |
|---|---|---|
| `--version` combined with an unknown option (`--version -x`) | no precedence rule | usage error, exit 2 (`evidence/probes/probes.txt`) |
| Error `<description>` text | placeholder only | Rust `io::Error` text, e.g. `No such file or directory (os error 2)`, `Permission denied (os error 13)` |
| `total` when some operands fail | not stated what is summed | sums readable operands only; still emits `total` when >1 FILE given |
| empty-string arg / lone `-` / name starting with `-` | undefined | `""`→open error exit 1; `-`→usage exit 2; `--x`→usage exit 2 |

These are ambiguities of the specification, not implementation defects; they are excluded from the verdict rather than silently scored.

## 6. Not covered (explicit) + reason

- stdin / TTY / `-`-as-stdin: SPEC defines no stdin input path (out of contract).
- Files larger than ~600 KiB / multi-GiB streams: streaming is O(1) memory and boundary logic is covered; only up to 200 000 lines (≈600 KB) and 2×64 KiB boundary blocks exercised.
- Concurrent truncation/rename of an operand between open and read (TOCTOU): not in SPEC; not tested.
- SIGPIPE / closed-stdout-pipe behaviour and stdout/stderr interleaving order: not specified.
- Signals, other errors (EIO/ENOSPC): not reachable deterministically in this sandbox.
- Non-Linux platforms / Windows path semantics: SPEC is byte-level; harness is Linux-only.
- Locale/Unicode segmentation beyond the whitespace byte set: SPEC forbids Unicode segmentation; spot-checked U+3000 and U+00A0 bytes count as non-whitespace (cases 25–27).

## 7. Snapshot integrity note

`source/` files re-hashed after all work and still equal `snapshot.json` (`evidence/snapshot-rehash.txt`); I made no writes under SNAPSHOT. Observation: an external actor created `snapshot-nvn21yva/verifications/` at 17:47:43 (+08:00), after the 17:45:37 seal — this is **not** my change and did not affect the byte-identical `source/` I verified.

## 8. Evidence index

- `evidence/cargo-build-release.log`
- `evidence/cargo-clippy.log`
- `evidence/cargo-fmt.log`
- `evidence/cargo-test-dev.log`
- `evidence/cargo-test-release.log`
- `evidence/differential-report-debug.json`
- `evidence/differential-report-debug.txt`
- `evidence/differential-report.json`
- `evidence/differential-report.txt`
- `evidence/probes`
- `evidence/release-binary.sha256`
- `evidence/snapshot-rehash.txt`
- `oracle/linestat_oracle.py`
- `harness/differential.py`
- `work/linestat/` (writable copy; `target/` is transient build output, excluded)
