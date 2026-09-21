#!/usr/bin/env python3
"""Independent oracle for the linestat 1.0.0 SPEC.md contract.

This module is transcribed directly from SPEC.md
(`.../snapshot-nvn21yva/source/SPEC.md`). It deliberately does NOT import,
execute, read, or otherwise consult the author's Rust source
(`src/main.rs`) or the author's test suite (`tests/cli.rs`) as a source of
truth. The author's artefacts are only ever used as the *subject under test*.

SPEC.md byte-level counting rules (quoted in the constants below) differ from
`wc(1)` for `words`; `lines` happens to coincide with GNU `wc -l` semantics but
is implemented here from the SPEC text, not from `wc`.

CLI grammar (SPEC.md 1.0.0):

    linestat [--version] [FILE...]

* `--version` -> stdout exactly ``linestat 1.0.0\\n``, exit 0, reads no file.
* per FILE, in argument order: ``<lines> <words> <bytes> <path>\\n`` where the
  four fields are separated by a single space and ``<path>`` is echoed verbatim
  (raw bytes, no normalisation).
* total line ``<lines> <words> <bytes> total`` iff more than one FILE was given.
* unreadable FILE -> one stderr line ``linestat: <path>: <description>``,
  processing continues, final exit 1.
* unknown option -> stderr ``linestat: usage: linestat [--version] [FILE...]``,
  exit 2.
* no FILE and no --version -> same usage error, exit 2.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import List, Optional, Tuple

# SPEC.md: whitespace bytes are exactly space(0x20), \t(0x09), \n(0x0A),
# \r(0x0D), \v(0x0B), \f(0x0C).  Judged per byte, no Unicode segmentation.
WHITESPACE = frozenset((0x20, 0x09, 0x0A, 0x0D, 0x0B, 0x0C))

VERSION_STDOUT = b"linestat 1.0.0\n"
USAGE_STDERR = b"linestat: usage: linestat [--version] [FILE...]\n"


@dataclass(frozen=True)
class Counts:
    lines: int
    words: int
    bytes: int

    def __add__(self, other: "Counts") -> "Counts":
        return Counts(
            self.lines + other.lines,
            self.words + other.words,
            self.bytes + other.bytes,
        )


ZERO = Counts(0, 0, 0)


def count_bytes(data: bytes) -> Counts:
    """SPEC.md counting rules, implemented over the raw byte stream."""
    n = len(data)
    # lines: number of 0x0A occurrences; +1 if non-empty and not ending in 0x0A.
    lines = data.count(0x0A)
    if n > 0 and data[-1] != 0x0A:
        lines += 1
    # words: number of maximal runs of non-whitespace bytes.
    words = 0
    in_word = False
    for b in data:
        if b in WHITESPACE:
            in_word = False
        elif not in_word:
            in_word = True
            words += 1
    return Counts(lines, words, n)


def count_path(path: bytes) -> Tuple[Optional[Counts], Optional[OSError]]:
    """Read a file (raw bytes) and count. Directory/unreadable -> OSError."""
    try:
        with open(path, "rb") as fh:
            data = fh.read()
    except OSError as exc:  # missing, not-a-file(IsADirectory), permission...
        return None, exc
    return count_bytes(data), None


def report_line(counts: Counts, name: bytes) -> bytes:
    return b"%d %d %d " % (counts.lines, counts.words, counts.bytes) + name + b"\n"


@dataclass
class Expectation:
    stdout: bytes = b""
    stderr_exact: Optional[bytes] = None       # when the exact bytes are defined
    stderr_error_paths: List[bytes] = field(default_factory=list)  # error lines
    exit_code: int = 0
    note: str = ""


def is_option(arg: bytes) -> bool:
    return arg.startswith(b"-")


def expected(argv: List[bytes]) -> Expectation:
    """Canonical SPEC reading of a single invocation.

    Precedence: unknown option (SPEC defines this unconditionally) >
    --version > no-FILE usage error > normal processing.  The SPEC does not
    define the combined case "unknown option AND --version"; callers must treat
    that combination as underdetermined rather than scored.
    """
    version = any(a == b"--version" for a in argv)
    unknown = any(is_option(a) and a != b"--version" for a in argv)
    files = [a for a in argv if not is_option(a)]

    if unknown:
        return Expectation(stdout=b"", stderr_exact=USAGE_STDERR, exit_code=2,
                           note="unknown option -> usage error")
    if version:
        return Expectation(stdout=VERSION_STDOUT, stderr_exact=b"", exit_code=0,
                           note="--version reads no files")
    if not files:
        return Expectation(stdout=b"", stderr_exact=USAGE_STDERR, exit_code=2,
                           note="no FILE -> usage error")

    out = b""
    total = ZERO
    errors: List[bytes] = []
    failed = False
    for path in files:
        counts, err = count_path(path)
        if counts is None:
            failed = True
            errors.append(path)
            continue
        total = total + counts
        out += report_line(counts, path)

    if len(files) > 1:
        out += report_line(total, b"total")

    return Expectation(
        stdout=out,
        stderr_exact=None,
        stderr_error_paths=errors,
        exit_code=1 if failed else 0,
        note="normal path",
    )


if __name__ == "__main__":
    import sys

    for probe in (b"", b"a", b"a\n", b"a\nb", b" \t\r\n", b"\xff\xfe A"):
        print(repr(probe), "->", count_bytes(probe))
    sys.exit(0)
