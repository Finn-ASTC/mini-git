//! linestat 1.0.0 — byte-level line/word/byte counter.
//!
//! Contract: `SPEC.md`. Counting is defined on raw bytes and deliberately
//! differs from `wc(1)`:
//!
//! * `lines` — `\n` (0x0A) count, plus one when the file is non-empty and does
//!   not end in `\n`.
//! * `words` — number of maximal runs of non-whitespace bytes, where
//!   whitespace is exactly `0x20 0x09 0x0A 0x0D 0x0B 0x0C`.
//! * `bytes` — file size in bytes.
//!
//! Both counts are computed in a single streaming pass over a fixed 64 KiB
//! buffer, so memory use is independent of file size.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

const VERSION_LINE: &str = "linestat 1.0.0";
const USAGE_LINE: &str = "linestat: usage: linestat [--version] [FILE...]";

/// Whitespace bytes for the `words` definition (SPEC.md).
fn is_word_separator(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

#[derive(Default, Clone, Copy)]
struct Counts {
    lines: u64,
    words: u64,
    bytes: u64,
}

impl Counts {
    fn add(&mut self, other: Counts) {
        self.lines += other.lines;
        self.words += other.words;
        self.bytes += other.bytes;
    }
}

/// Streams `path` once and returns its counts.
fn measure(path: &OsString) -> io::Result<Counts> {
    let mut file = File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    let mut counts = Counts::default();
    let mut in_word = false;
    let mut last: Option<u8> = None;

    loop {
        let read = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        counts.bytes += read as u64;
        for &b in &buf[..read] {
            if b == b'\n' {
                counts.lines += 1;
            }
            if is_word_separator(b) {
                in_word = false;
            } else if !in_word {
                in_word = true;
                counts.words += 1;
            }
        }
        last = Some(buf[read - 1]);
    }

    if counts.bytes > 0 && last != Some(b'\n') {
        counts.lines += 1;
    }
    Ok(counts)
}

/// Writes `<lines> <words> <bytes> <name>\n`; `name` is echoed verbatim.
fn write_report(out: &mut impl Write, counts: Counts, name: &[u8]) -> io::Result<()> {
    write!(out, "{} {} {} ", counts.lines, counts.words, counts.bytes)?;
    out.write_all(name)?;
    out.write_all(b"\n")
}

fn write_error(err_out: &mut impl Write, name: &[u8], err: &io::Error) -> io::Result<()> {
    err_out.write_all(b"linestat: ")?;
    err_out.write_all(name)?;
    err_out.write_all(b": ")?;
    err_out.write_all(err.to_string().as_bytes())?;
    err_out.write_all(b"\n")
}

fn write_usage(err_out: &mut impl Write) -> io::Result<()> {
    err_out.write_all(USAGE_LINE.as_bytes())?;
    err_out.write_all(b"\n")
}

fn run() -> io::Result<ExitCode> {
    let mut files: Vec<OsString> = Vec::new();
    let mut version = false;

    for arg in env::args_os().skip(1) {
        let bytes = arg.as_bytes();
        if bytes == b"--version" {
            version = true;
        } else if bytes.first() == Some(&b'-') {
            // Unknown option. SPEC.md has no stdin handling, so a file whose
            // name begins with `-` must be given as `./-name`.
            write_usage(&mut io::stderr().lock())?;
            return Ok(ExitCode::from(2));
        } else {
            files.push(arg);
        }
    }

    // `--version` wins over file arguments and reads nothing.
    if version {
        let mut out = io::stdout().lock();
        out.write_all(VERSION_LINE.as_bytes())?;
        out.write_all(b"\n")?;
        out.flush()?;
        return Ok(ExitCode::SUCCESS);
    }

    if files.is_empty() {
        write_usage(&mut io::stderr().lock())?;
        return Ok(ExitCode::from(2));
    }

    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut err_out = stderr.lock();
    let mut total = Counts::default();
    let mut failed = false;

    for path in &files {
        match measure(path) {
            Ok(counts) => {
                total.add(counts);
                write_report(&mut out, counts, path.as_bytes())?;
            }
            Err(e) => {
                failed = true;
                out.flush()?; // keep stdout/stderr in argument order
                write_error(&mut err_out, path.as_bytes(), &e)?;
            }
        }
    }

    // Emitted whenever more than one FILE was given; sums the files that were
    // readable, mirroring `wc` on a failed operand.
    if files.len() > 1 {
        write_report(&mut out, total, b"total")?;
    }

    out.flush()?;
    err_out.flush()?;
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            // Report write failures (e.g. closed stdout pipe) instead of panicking.
            let _ = writeln!(io::stderr().lock(), "linestat: {e}");
            ExitCode::from(1)
        }
    }
}
