//! Behavioural tests: every case drives the built `linestat` binary and
//! asserts stdout, stderr and exit status.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_linestat");

fn os(s: &str) -> &OsStr {
    OsStr::new(s)
}

fn run(args: &[&OsStr]) -> Output {
    Command::new(BIN).args(args).output().expect("run linestat")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("stderr is UTF-8")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit status, not a signal")
}

/// Unique scratch directory under the cargo target dir; removed on drop.
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "cli-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create sandbox");
        Sandbox { dir }
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.dir.join(name);
        fs::write(&path, content).expect("write fixture");
        path
    }

    fn raw_file(&self, name: &[u8], content: &[u8]) -> PathBuf {
        let path = self.dir.join(OsString::from_vec(name.to_vec()));
        fs::write(&path, content).expect("write fixture");
        path
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn version_prints_exactly_one_line_and_reads_no_files() {
    let out = run(&[os("--version"), os("definitely-missing.txt")]);
    assert_eq!(stdout(&out), "linestat 1.0.0\n");
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

#[test]
fn single_file_reports_counts_and_echoes_path_verbatim() {
    let sb = Sandbox::new();
    let p = sb.file("one.txt", b"hello world\n");
    let out = run(&[p.as_os_str()]);
    assert_eq!(
        stdout(&out),
        format!("1 2 12 {}\n", p.display()),
        "no total line for a single file"
    );
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

#[test]
fn multiple_files_report_each_in_argument_order_then_total() {
    let sb = Sandbox::new();
    let a = sb.file("a.txt", b"one two\n");
    let b = sb.file("b.txt", b"three\n");
    let out = run(&[b.as_os_str(), a.as_os_str()]);
    assert_eq!(
        stdout(&out),
        format!(
            "1 1 6 {}\n1 2 8 {}\n2 3 14 total\n",
            b.display(),
            a.display()
        )
    );
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

#[test]
fn empty_file_counts_zero() {
    let sb = Sandbox::new();
    let p = sb.file("empty.txt", b"");
    let out = run(&[p.as_os_str()]);
    assert_eq!(stdout(&out), format!("0 0 0 {}\n", p.display()));
    assert_eq!(code(&out), 0);
}

#[test]
fn missing_trailing_newline_adds_one_line() {
    let sb = Sandbox::new();
    let p = sb.file("nonl.txt", b"a\nb\nc");
    let out = run(&[p.as_os_str()]);
    assert_eq!(stdout(&out), format!("3 3 5 {}\n", p.display()));
    assert_eq!(code(&out), 0);
}

#[test]
fn cr_tab_vt_ff_and_space_separate_words_but_not_lines() {
    let sb = Sandbox::new();
    // "x\r\ty\vz\fw\n": x|y|z|w separated by \r \t \v \f, one trailing \n.
    let p = sb.file("ws.txt", b"x\r\ty\x0bz\x0cw\n");
    let out = run(&[p.as_os_str()]);
    assert_eq!(stdout(&out), format!("1 4 9 {}\n", p.display()));
    assert_eq!(code(&out), 0);
}

#[test]
fn non_utf8_content_is_counted_as_bytes() {
    let sb = Sandbox::new();
    let p = sb.file("binary.bin", &[0xff, 0xfe, 0x20, 0x41]);
    let out = run(&[p.as_os_str()]);
    assert_eq!(stdout(&out), format!("1 2 4 {}\n", p.display()));
    assert_eq!(code(&out), 0);
}

#[test]
fn non_utf8_path_is_echoed_byte_for_byte() {
    let sb = Sandbox::new();
    let p = sb.raw_file(b"weird-\xff-name.txt", b"ab\n");
    let out = run(&[p.as_os_str()]);
    let mut expected = b"1 1 3 ".to_vec();
    expected.extend_from_slice(p.as_os_str().as_bytes());
    expected.push(b'\n');
    assert_eq!(out.stdout, expected);
    assert_eq!(code(&out), 0);
}

#[test]
fn unreadable_file_reports_error_on_stderr_and_continues_with_exit_1() {
    let sb = Sandbox::new();
    let missing = sb.path("missing.txt");
    let ok = sb.file("ok.txt", b"abc\n");
    let out = run(&[missing.as_os_str(), ok.as_os_str()]);

    assert_eq!(
        stdout(&out),
        format!("1 1 4 {}\n1 1 4 total\n", ok.display())
    );
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!("linestat: {}: ", missing.display())),
        "unexpected stderr: {err:?}"
    );
    assert_eq!(err.lines().count(), 1, "one diagnostic per failing file");
    assert_eq!(code(&out), 1);
}

#[test]
fn directory_operand_is_an_error() {
    let sb = Sandbox::new();
    let out = run(&[sb.dir.as_os_str()]);
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).starts_with(&format!("linestat: {}: ", sb.dir.display())),
        "unexpected stderr: {:?}",
        stderr(&out)
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn unknown_option_and_missing_arguments_are_usage_errors() {
    let sb = Sandbox::new();
    let existing = sb.file("one.txt", b"hello world\n");

    for args in [
        vec![os("--help")],
        vec![os("-x")],
        vec![os("--help"), existing.as_os_str()],
        vec![os("-x"), os("--version")],
        vec![],
    ] {
        let out = run(&args);
        assert_eq!(stdout(&out), "", "no stdout for {args:?}");
        assert_eq!(
            stderr(&out),
            "linestat: usage: linestat [--version] [FILE...]\n",
            "wrong stderr for {args:?}"
        );
        assert_eq!(code(&out), 2, "wrong exit code for {args:?}");
    }
}
