#!/usr/bin/env python3
"""Differential acceptance harness for linestat 1.0.0.

Runs the *built release binary* against a fixture matrix and compares, per case,
the actual stdout bytes / stderr bytes / exit code with the expectations
computed by the independent oracle `oracle/linestat_oracle.py`.

The author's own tests are NOT used here; this is the primary evidence.

Usage:
    python3 harness/differential.py <path-to-linestat-binary> <fixtures-root> \
        <report-prefix>
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "oracle"))
import linestat_oracle as oracle  # noqa: E402

BIG = 64 * 1024


def file_fixtures():
    """Return (case_name, args, fixtures) triples.

    args items are tagged:
        ("opt", b"--version")   literal argument byte string
        ("file", b"name")       argument resolved to <case_dir>/name
    fixtures items:
        ("f", b"name", b"content")  write a file with raw-byte name/content
        ("d", b"name")              create a directory
    """
    cases = []

    def C(name, args, fixtures, note=""):
        cases.append({"name": name, "args": args, "fixtures": fixtures, "note": note})

    # ---- required matrix -------------------------------------------------
    C("version_alone", [("opt", b"--version")], [])
    C("version_with_missing_file", [("opt", b"--version"), ("file", b"missing.txt")],
      [], "SPEC: --version reads no file")
    C("single_file", [("file", b"one.txt")], [("f", b"one.txt", b"hello world\n")])
    C("single_no_trailing_newline", [("file", b"nonl.txt")],
      [("f", b"nonl.txt", b"a\nb\nc")])
    C("empty_file", [("file", b"empty.txt")], [("f", b"empty.txt", b"")])
    C("multi_files_total", [("file", b"b.txt"), ("file", b"a.txt")],
      [("f", b"a.txt", b"one two\n"), ("f", b"b.txt", b"three\n")],
      "argument order preserved; total sums")
    C("multi_files_total_reverse", [("file", b"a.txt"), ("file", b"b.txt")],
      [("f", b"a.txt", b"one two\n"), ("f", b"b.txt", b"three\n")])
    C("cr_tab_vt_ff", [("file", b"ws.txt")],
      [("f", b"ws.txt", b"x\r\ty\x0bz\x0cw\n")])
    C("non_utf8_content", [("file", b"binary.bin")],
      [("f", b"binary.bin", b"\xff\xfe\x20\x41")])
    C("missing_file", [("file", b"missing.txt")], [])
    C("missing_then_good", [("file", b"missing.txt"), ("file", b"ok.txt")],
      [("f", b"ok.txt", b"abc\n")], "continue after error; total from readable")
    C("good_then_missing", [("file", b"ok.txt"), ("file", b"missing.txt")],
      [("f", b"ok.txt", b"abc\n")])
    C("unknown_long_option", [("opt", b"--help")], [])
    C("unknown_short_option", [("opt", b"-x")], [])
    C("unknown_option_with_file", [("opt", b"--help"), ("file", b"one.txt")],
      [("f", b"one.txt", b"hello world\n")])
    C("no_args", [], [])

    # ---- extended boundary / equivalence classes ------------------------
    C("directory_operand", [("file", b"thedir")], [("d", b"thedir")])
    C("non_utf8_path", [("file", b"weird-\xff-name.txt")],
      [("f", b"weird-\xff-name.txt", b"ab\n")])
    C("only_newlines", [("file", b"nl.txt")], [("f", b"nl.txt", b"\n\n\n")])
    C("only_cr_no_newline", [("file", b"cr.txt")], [("f", b"cr.txt", b"\r")])
    C("whitespace_only", [("file", b"w.txt")], [("f", b"w.txt", b" \t\r\n\x0b\x0c")])
    C("nul_bytes", [("file", b"nul.bin")], [("f", b"nul.bin", b"a\x00b\n")])
    C("permission_denied", [("file", b"noperm.txt")],
      [("f", b"noperm.txt", b"secret\n", 0o000)],
      "SPEC error path: unreadable file (EACCES)")
    C("permission_denied_then_good",
      [("file", b"noperm.txt"), ("file", b"ok.txt")],
      [("f", b"noperm.txt", b"secret\n", 0o000), ("f", b"ok.txt", b"abc\n")],
      "EACCES then continue; total from readable")
    C("unicode_ideographic_space", [("file", b"u3000.txt")],
      [("f", b"u3000.txt", b"\xe3\x80\x80")],
      "U+3000 bytes are NOT SPEC whitespace -> one word")
    C("unicode_nbsp", [("file", b"nbsp.txt")],
      [("f", b"nbsp.txt", b"\xc2\xa0")],
      "U+00A0 bytes are NOT SPEC whitespace -> one word")
    C("mixed_unicode_and_ascii_ws", [("file", b"mix.txt")],
      [("f", b"mix.txt", b"a\xe3\x80\x80b c\n")])
    C("multi_non_utf8_total",
      [("file", b"b1.bin"), ("file", b"b2.bin")],
      [("f", b"b1.bin", b"\xff\xfe"), ("f", b"b2.bin", b"\x80\x81\n")])
    C("path_echo_no_normalisation", [("file", b"./one.txt")],
      [("f", b"one.txt", b"hi\n")], "argument echoed verbatim incl ./")
    C("many_bytes_lines", [("file", b"big.txt")],
      [("f", b"big.txt", (b"x y\n" * 100000))])

    # word crossing / touching the 64 KiB read boundary
    word_boundary = [
        ("boundary_word_split", b"a" * BIG + b"b", 1),
        ("boundary_sep_then_word", b"a" * (BIG - 1) + b" " + b"b" * 5, 2),
        ("boundary_newline_then_word", b"a" * (BIG - 1) + b"\n" + b"b" * 10, 2),
        ("boundary_word_then_sep", b"a" * BIG + b" b", 2),
        ("boundary_two_chunks", b"a" * (2 * BIG) + b" " + b"b", 2),
        ("boundary_all_ws_then_word", b" " * BIG + b"word", 1),
    ]
    for name, data, _w in word_boundary:
        C(name, [("file", (name + ".bin").encode())],
          [("f", (name + ".bin").encode(), data)],
          "word count must persist across the 64 KiB read boundary")

    return cases


def materialise(case_dir: bytes, fixtures):
    for fx in fixtures:
        if fx[0] == "d":
            os.mkdir(os.path.join(case_dir, fx[1]))
            continue
        path = os.path.join(case_dir, fx[1])
        with open(path, "wb") as fh:
            fh.write(fx[2])
        if len(fx) >= 4:
            os.chmod(path, fx[3])


def resolve_args(case_dir: bytes, args):
    argv = []
    for tag, val in args:
        if tag == "opt":
            argv.append(val)
        else:
            argv.append(os.path.join(case_dir, val))
    return argv


def check_stderr(actual: bytes, exp: oracle.Expectation):
    if exp.stderr_exact is not None:
        if actual == exp.stderr_exact:
            return True, ""
        return False, "stderr exact mismatch: expected %r got %r" % (
            exp.stderr_exact, actual)
    lines = actual.split(b"\n")
    if lines and lines[-1] == b"":
        lines = lines[:-1]
    if len(lines) != len(exp.stderr_error_paths):
        return False, "stderr line count %d != expected %d; actual=%r" % (
            len(lines), len(exp.stderr_error_paths), actual)
    for line, path in zip(lines, exp.stderr_error_paths):
        prefix = b"linestat: " + path + b": "
        if not line.startswith(prefix):
            return False, "stderr line %r lacks prefix %r" % (line, prefix)
        if len(line) <= len(prefix):
            return False, "stderr line %r has empty <description>" % line
    return True, ""


def main():
    binpath = os.path.abspath(sys.argv[1]).encode()
    root = os.path.abspath(sys.argv[2]).encode()
    prefix = sys.argv[3]

    if os.path.exists(root):
        shutil.rmtree(root)
    os.makedirs(root)

    bin_sha = hashlib.sha256(open(binpath, "rb").read()).hexdigest()

    results = []
    for case in file_fixtures():
        case_dir = os.path.join(root, case["name"].encode())
        os.makedirs(case_dir)
        materialise(case_dir, case["fixtures"])
        argv = resolve_args(case_dir, case["args"])
        exp = oracle.expected(argv)

        proc = subprocess.run([binpath] + argv, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, cwd=case_dir, timeout=120)
        act_out, act_err, act_code = proc.stdout, proc.stderr, proc.returncode

        failures = []
        if act_out != exp.stdout:
            failures.append("stdout mismatch: expected %r got %r" % (exp.stdout, act_out))
        if act_code != exp.exit_code:
            failures.append("exit mismatch: expected %d got %d" % (exp.exit_code, act_code))
        ok, msg = check_stderr(act_err, exp)
        if not ok:
            failures.append(msg)

        results.append({
            "case": case["name"],
            "note": case["note"],
            "argv": [a.decode("utf-8", "backslashreplace") for a in argv],
            "expected_stdout": exp.stdout.decode("utf-8", "backslashreplace"),
            "actual_stdout": act_out.decode("utf-8", "backslashreplace"),
            "expected_exit": exp.exit_code,
            "actual_exit": act_code,
            "expected_stderr": (exp.stderr_exact.decode("utf-8", "backslashreplace")
                                if exp.stderr_exact is not None
                                else ["<error line for %s>" % p.decode("utf-8", "backslashreplace")
                                      for p in exp.stderr_error_paths]),
            "actual_stderr": act_err.decode("utf-8", "backslashreplace"),
            "pass": not failures,
            "failures": failures,
        })

    passed = sum(1 for r in results if r["pass"])
    total = len(results)

    report = {
        "binary": binpath.decode(),
        "binary_sha256": bin_sha,
        "fixtures_root": root.decode(),
        "cases_total": total,
        "cases_passed": passed,
        "cases_failed": total - passed,
        "results": results,
    }
    with open(prefix + ".json", "w", encoding="utf-8") as fh:
        json.dump(report, fh, indent=2, ensure_ascii=False)

    with open(prefix + ".txt", "w", encoding="utf-8") as fh:
        fh.write("binary: %s\nsha256: %s\ncases: %d/%d passed\n\n"
                 % (binpath.decode(), bin_sha, passed, total))
        for r in results:
            fh.write("%s %s\n" % ("PASS" if r["pass"] else "FAIL", r["case"]))
            fh.write("  argv: %s\n" % (r["argv"],))
            fh.write("  exit: expected %d actual %d\n" % (r["expected_exit"], r["actual_exit"]))
            fh.write("  stdout expected: %s\n" % repr(r["expected_stdout"]))
            if not r["pass"]:
                fh.write("  stdout actual  : %s\n" % repr(r["actual_stdout"]))
                fh.write("  stderr actual  : %s\n" % repr(r["actual_stderr"]))
                for f in r["failures"]:
                    fh.write("  FAILURE: %s\n" % f)
            fh.write("\n")

    print("cases: %d/%d passed; binary sha256=%s" % (passed, total, bin_sha))
    for r in results:
        print("%s %s" % ("PASS" if r["pass"] else "FAIL", r["case"]))
    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main())
