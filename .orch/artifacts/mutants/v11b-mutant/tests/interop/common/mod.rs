//! 与真实 `git` 做差分测试的公共工具。
//!
//! **CONTROLLER-OWNED / verifier-only**：本目录只用来做「独立验收」，
//! 不允许被实现型 agent 修改（见 `ORCHESTRATION.md` C1/C3）。
//!
//! 设计要点：
//! * 每个测试用独立的临时目录，绝不碰用户仓库。
//! * 所有 git 调用都禁用全局/系统配置并固定时间戳，保证可复现。
//! * 断言失败时打印双方原始输出，方便 controller 事后复核（证据留痕）。

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// cargo 在集成测试里注入的 mg 二进制路径。
pub const MG: &str = env!("CARGO_BIN_EXE_mg");

pub const FIXED_DATE: &str = "1700000000 +0800";

/// 一个隔离的临时工作目录。
pub struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    pub fn new() -> Scratch {
        Scratch {
            dir: tempfile::tempdir().expect("failed to create temp dir"),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// 用 mg 建仓库。
    pub fn init_with_mg(&self) -> String {
        let out = self.mg(&["init", "."]);
        assert!(
            out.status.success(),
            "mg init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// 用真实 git 建仓库。
    pub fn init_with_git(&self) -> String {
        self.git_ok(&["init", "--quiet", "."])
    }

    /// 写出一个文件（自动创建父目录）。
    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    pub fn write_bytes(&self, rel: &str, contents: &[u8]) {
        let path = self.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.join(rel)).unwrap()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.join(rel).symlink_metadata().is_ok()
    }

    /// 真实 git，返回 `Output`（不检查退出码）。
    pub fn git(&self, args: &[&str]) -> Output {
        run(git_command(self.dir.path(), args))
    }

    /// 真实 git，成功则返回 trim 过的 stdout。
    pub fn git_ok(&self, args: &[&str]) -> String {
        let out = self.git(args);
        assert_success(&format!("git {}", args.join(" ")), &out);
        trim(&out.stdout)
    }

    /// 真实 git，允许失败，返回 `(success, stdout, stderr)`。
    pub fn git_raw(&self, args: &[&str]) -> (bool, String, String) {
        let out = self.git(args);
        (out.status.success(), trim(&out.stdout), trim(&out.stderr))
    }

    /// mg，返回 `Output`（不检查退出码）。
    pub fn mg(&self, args: &[&str]) -> Output {
        run(mg_command(self.dir.path(), args))
    }

    /// mg，成功则返回 trim 过的 stdout。
    pub fn mg_ok(&self, args: &[&str]) -> String {
        let out = self.mg(args);
        assert_success(&format!("mg {}", args.join(" ")), &out);
        trim(&out.stdout)
    }

    pub fn mg_raw(&self, args: &[&str]) -> (bool, String, String) {
        let out = self.mg(args);
        (out.status.success(), trim(&out.stdout), trim(&out.stderr))
    }

    /// 让真实 git 产生一个提交（测试准备数据用）。
    pub fn git_commit_all(&self, message: &str) -> String {
        self.git_ok(&["add", "-A"]);
        self.git_ok(&["commit", "--quiet", "-m", message]);
        self.git_ok(&["rev-parse", "HEAD"])
    }
}

/// 构造一个被完全隔离的 git 命令。
pub fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", FIXED_DATE)
        .env("GIT_COMMITTER_DATE", FIXED_DATE)
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    for global in [
        "-c",
        "init.defaultBranch=main",
        "-c",
        "core.autocrlf=false",
        "-c",
        "core.filemode=true",
        "-c",
        "gc.auto=0",
        "-c",
        "advice.detachedHead=false",
        "-c",
        "protocol.file.allow=always",
        "-c",
        "commit.gpgsign=false",
    ] {
        cmd.arg(global);
    }
    cmd.args(args);
    cmd
}

/// 构造一个 mg 命令（与 git 使用同样的隔离环境）。
pub fn mg_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(MG);
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    cmd.args(args);
    cmd
}

pub fn run(mut cmd: Command) -> Output {
    cmd.output().expect("failed to spawn process")
}

pub fn trim(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

pub fn assert_success(what: &str, out: &Output) {
    assert!(
        out.status.success(),
        "{what} failed (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// 差分断言：失败时把双方输出都打出来（这是「独立验证」的证据格式）。
pub fn assert_same(what: &str, mg_output: &str, git_output: &str) {
    if mg_output == git_output {
        return;
    }
    panic!(
        "{what}: mg output differs from real git\n\
         --- mg ---\n{mg_output}\n--- git ---\n{git_output}\n\
         --- byte diff ---\n{}",
        byte_diff(mg_output, git_output)
    );
}

fn byte_diff(a: &str, b: &str) -> String {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut out = String::new();
    for idx in 0..a.len().max(b.len()) {
        let ca = a.get(idx).copied();
        let cb = b.get(idx).copied();
        if ca != cb {
            out.push_str(&format!(
                "  first difference at char {idx}: mg={ca:?} git={cb:?}\n"
            ));
            break;
        }
    }
    out.push_str(&format!(
        "  mg  len={} bytes\n  git len={} bytes\n",
        a.len(),
        b.len()
    ));
    out
}
