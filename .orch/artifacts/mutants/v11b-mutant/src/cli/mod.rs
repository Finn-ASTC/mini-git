//! 命令行表面。**CONTROLLER-OWNED**（`Cli` / `Command` 的声明区冻结）。
//!
//! 每个子命令一个文件，属于该命令的负责 agent（见 `ORCHESTRATION.md` §4）。
//! `cli/*.rs` 只做「解析好的参数 → 调用下层 API → 打印输出」，
//! 不允许直接读写仓库文件（那是 `repo` / `index` / `refs` / `worktree` 的职责）。

pub mod add;
pub mod branch;
pub mod cat_file;
pub mod checkout;
pub mod clone;
pub mod commit;
pub mod diff;
pub mod fetch;
pub mod fsck;
pub mod gc;
pub mod hash_object;
pub mod init;
pub mod log;
pub mod merge;
pub mod pull;
pub mod push;
pub mod reset;
pub mod rm;
pub mod status;
pub mod switch;
pub mod tag;

use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

use crate::error::{Error, Result};
use crate::repo::Repo;

/// 所有需要仓库的命令的统一入口（保证「不在仓库里」的报错一致）。
pub fn open_repo() -> Result<Repo> {
    Repo::discover(std::path::Path::new("."))
}

#[derive(Debug, Parser)]
#[command(
    name = "mg",
    version,
    about = "mini-git: a git subset that interoperates with real git",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Run as if mg was started in <DIR>.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    pub cd: Option<PathBuf>,

    /// Print extra detail (repeatable).
    #[arg(short = 'v', long = "verbose", global = true, action = ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create an empty repository.
    Init {
        path: Option<PathBuf>,
        #[arg(long = "initial-branch", default_value = "main")]
        initial_branch: String,
        #[arg(short = 'q', long = "quiet")]
        quiet: bool,
    },

    /// Compute the object id of a file, optionally writing it to the object database.
    HashObject {
        file: Option<PathBuf>,
        #[arg(long = "stdin")]
        stdin: bool,
        #[arg(short = 'w')]
        write: bool,
        #[arg(short = 't', long = "type", default_value = "blob")]
        kind: String,
    },

    /// Show the type, size or contents of an object.
    CatFile {
        #[arg(short = 't')]
        show_type: bool,
        #[arg(short = 's')]
        show_size: bool,
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,
        #[arg(short = 'e')]
        exists: bool,
        object: String,
    },

    /// Add file contents to the index.
    Add {
        #[arg(required = true)]
        pathspec: Vec<PathBuf>,
    },

    /// Remove files from the index and/or the working tree.
    Rm {
        #[arg(required = true)]
        pathspec: Vec<PathBuf>,
        #[arg(long = "cached")]
        cached: bool,
        #[arg(short = 'f', long = "force")]
        force: bool,
    },

    /// Show the working tree status.
    Status {
        #[arg(long = "porcelain", num_args = 0..=1, default_missing_value = "v1", require_equals = true, value_name = "FORMAT")]
        porcelain: Option<String>,
        #[arg(short = 's', long = "short")]
        short: bool,
    },

    /// Record changes to the repository.
    Commit {
        #[arg(short = 'm', long = "message", value_name = "MSG")]
        message: Option<String>,
        #[arg(short = 'a', long = "all")]
        all: bool,
        #[arg(long = "amend")]
        amend: bool,
        #[arg(long = "allow-empty")]
        allow_empty: bool,
        #[arg(long = "author", value_name = "NAME <EMAIL>")]
        author: Option<String>,
    },

    /// Show commit history.
    Log {
        #[arg(long = "oneline")]
        oneline: bool,
        #[arg(short = 'n', long = "max-count", value_name = "N")]
        max_count: Option<usize>,
        rev: Option<String>,
    },

    /// Show changes between commits, the index and the working tree.
    Diff {
        #[arg(long = "staged", alias = "cached")]
        staged: bool,
        #[arg(short = 'U', long = "unified", default_value_t = 3, value_name = "N")]
        context: usize,
        rev: Option<String>,
        #[arg(last = true)]
        paths: Vec<PathBuf>,
    },

    /// List, create, delete or rename branches.
    Branch {
        #[arg(short = 'd', long = "delete")]
        delete: bool,
        #[arg(short = 'm', long = "move")]
        rename: bool,
        #[arg(short = 'l', long = "list")]
        list: bool,
        name: Option<String>,
        start: Option<String>,
    },

    /// Switch branches.
    Switch {
        #[arg(short = 'c', long = "create", value_name = "NAME")]
        create: Option<String>,
        #[arg(short = 'f', long = "force")]
        force: bool,
        name: String,
    },

    /// Check out a commit, branch or paths into the working tree.
    Checkout {
        #[arg(short = 'f', long = "force")]
        force: bool,
        #[arg(long = "detach")]
        detach: bool,
        rev: String,
        #[arg(last = true)]
        paths: Vec<PathBuf>,
    },

    /// Join development histories together.
    Merge {
        rev: String,
        #[arg(short = 'm', long = "message", value_name = "MSG")]
        message: Option<String>,
        #[arg(long = "no-ff")]
        no_ff: bool,
    },

    /// Create, list or delete tags.
    Tag {
        #[arg(short = 'a', long = "annotate")]
        annotate: bool,
        #[arg(short = 'm', long = "message", value_name = "MSG")]
        message: Option<String>,
        #[arg(short = 'l', long = "list")]
        list: bool,
        name: Option<String>,
        rev: Option<String>,
    },

    /// Reset current HEAD to the specified state.
    Reset {
        #[arg(long = "soft", group = "mode")]
        soft: bool,
        #[arg(long = "mixed", group = "mode")]
        mixed: bool,
        #[arg(long = "hard", group = "mode")]
        hard: bool,
        rev: String,
    },

    /// Clone a repository into a new directory.
    Clone { url: String, dir: Option<PathBuf> },

    /// Download objects and refs from another repository.
    Fetch {
        remote: Option<String>,
        refspec: Option<String>,
    },

    /// Fetch from and integrate with another repository or a local branch.
    Pull { remote: Option<String> },

    /// Update remote refs along with associated objects.
    Push {
        remote: Option<String>,
        refspec: Option<String>,
        #[arg(short = 'u', long = "set-upstream")]
        set_upstream: bool,
        #[arg(short = 'f', long = "force")]
        force: bool,
    },

    /// Verify the connectivity and validity of objects in the database.
    Fsck {
        #[arg(long = "full")]
        full: bool,
    },

    /// Clean up unnecessary files and optimize the local repository.
    Gc, // v1 无参数
}

/// 统一入口：`-C` 生效后分发到各子命令。
pub fn dispatch(cli: Cli) -> Result<()> {
    if let Some(dir) = cli.cd.as_deref() {
        std::env::set_current_dir(dir)
            .map_err(|err| Error::Other(format!("cannot change to '{}': {err}", dir.display())))?;
    }

    match cli.command {
        Command::Init {
            path,
            initial_branch,
            quiet,
        } => init::run(path.as_deref(), &initial_branch, quiet),
        Command::HashObject {
            file,
            stdin,
            write,
            kind,
        } => hash_object::run(file.as_deref(), stdin, write, &kind),
        Command::CatFile {
            show_type,
            show_size,
            pretty,
            exists,
            object,
        } => cat_file::run(show_type, show_size, pretty, exists, &object),
        // `verbose` 走全局 repeatable `-v`（`Cli::verbose`）；子命令不再单独声明，
        // 否则 clap 会因「同名参数两种动作类型」在 parse 阶段 panic（controller 修复，见 C-11）。
        Command::Add { pathspec } => add::run(&pathspec, cli.verbose > 0),
        Command::Rm {
            pathspec,
            cached,
            force,
        } => rm::run(&pathspec, cached, force),
        Command::Status { porcelain, short } => status::run(porcelain.as_deref(), short),
        Command::Commit {
            message,
            all,
            amend,
            allow_empty,
            author,
        } => commit::run(
            message.as_deref(),
            all,
            amend,
            allow_empty,
            author.as_deref(),
        ),
        Command::Log {
            oneline,
            max_count,
            rev,
        } => log::run(oneline, max_count, rev.as_deref()),
        Command::Diff {
            staged,
            context,
            rev,
            paths,
        } => diff::run(staged, context, rev.as_deref(), &paths),
        Command::Branch {
            delete,
            rename,
            list,
            name,
            start,
        } => branch::run(delete, rename, list, name.as_deref(), start.as_deref()),
        Command::Switch {
            create,
            force,
            name,
        } => switch::run(create.as_deref(), &name, force),
        Command::Checkout {
            force,
            detach,
            rev,
            paths,
        } => checkout::run(&rev, &paths, force, detach),
        Command::Merge {
            rev,
            message,
            no_ff,
        } => merge::run(&rev, message.as_deref(), no_ff),
        Command::Tag {
            annotate,
            message,
            list,
            name,
            rev,
        } => tag::run(
            annotate,
            message.as_deref(),
            list,
            name.as_deref(),
            rev.as_deref(),
        ),
        Command::Reset {
            soft,
            mixed,
            hard,
            rev,
        } => {
            let mode = if soft {
                ResetMode::Soft
            } else if hard {
                ResetMode::Hard
            } else {
                let _ = mixed;
                ResetMode::Mixed
            };
            reset::run(mode, &rev)
        }
        Command::Clone { url, dir } => clone::run(&url, dir.as_deref()),
        Command::Fetch { remote, refspec } => fetch::run(remote.as_deref(), refspec.as_deref()),
        Command::Pull { remote } => pull::run(remote.as_deref()),
        Command::Push {
            remote,
            refspec,
            set_upstream,
            force,
        } => push::run(remote.as_deref(), refspec.as_deref(), set_upstream, force),
        Command::Fsck { full } => fsck::run(full),
        Command::Gc => gc::run(),
    }
}
