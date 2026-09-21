//! 仓库发现、`.git` 布局与最小 config 解析。CONTROLLER-OWNED —— 冻结，实现已完成。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub const DEFAULT_INITIAL_BRANCH: &str = "main";

#[derive(Debug, Clone)]
pub struct Repo {
    workdir: Option<PathBuf>,
    git_dir: PathBuf,
}

impl Repo {
    /// 从 `start` 向上查找 `.git`（目录或 gitdir 文件）；`start` 自身是 bare 仓库也可。
    pub fn discover(start: &Path) -> Result<Repo> {
        let start = if start.as_os_str().is_empty() {
            Path::new(".")
        } else {
            start
        };
        let start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
        let mut cur: Option<&Path> = Some(&start);
        while let Some(dir) = cur {
            let dot_git = dir.join(".git");
            if dot_git.is_dir() {
                return Repo::open_git_dir(dir.to_path_buf(), dot_git);
            }
            if dot_git.is_file() {
                let text = std::fs::read_to_string(&dot_git)?;
                let target = text
                    .strip_prefix("gitdir:")
                    .ok_or_else(|| {
                        Error::corrupt(dot_git.display().to_string(), "unrecognized .git file")
                    })?
                    .trim();
                let git_dir = dot_git.parent().unwrap_or(Path::new(".")).join(target);
                return Repo::open_git_dir(dir.to_path_buf(), git_dir);
            }
            if is_git_dir(dir) {
                return Repo::open_git_dir(dir.to_path_buf(), dir.to_path_buf());
            }
            cur = dir.parent();
        }
        Err(Error::NotARepo(start))
    }

    /// 打开一个已知的 git 目录（`git_dir` 通常但不总是 `<workdir>/.git`）。
    pub fn open_git_dir(workdir: PathBuf, git_dir: PathBuf) -> Result<Repo> {
        if !is_git_dir(&git_dir) {
            return Err(Error::NotARepo(git_dir));
        }
        let repo = Repo {
            workdir: Some(workdir),
            git_dir,
        };
        let version = repo
            .config()
            .get("core.repositoryformatversion")
            .map(str::to_string);
        if let Some(version) = version {
            if version.trim() != "0" {
                return Err(Error::UnsupportedRepoFormat(version));
            }
        }
        Ok(repo)
    }

    /// 打开 bare 仓库（无工作区）。
    pub fn open_bare(git_dir: PathBuf) -> Result<Repo> {
        if !is_git_dir(&git_dir) {
            return Err(Error::NotARepo(git_dir));
        }
        Ok(Repo {
            workdir: None,
            git_dir,
        })
    }

    /// `mg init`：创建真实 git 也能直接使用的仓库骨架。
    pub fn init(path: &Path, initial_branch: &str) -> Result<Repo> {
        std::fs::create_dir_all(path)?;
        let workdir = std::fs::canonicalize(path)?;
        let git_dir = workdir.join(".git");
        if git_dir.exists() {
            return Err(Error::Other(format!(
                "{} already exists: refusing to reinit",
                git_dir.display()
            )));
        }
        for sub in [
            "objects/info",
            "objects/pack",
            "refs/heads",
            "refs/tags",
            "info",
        ] {
            std::fs::create_dir_all(git_dir.join(sub))?;
        }
        std::fs::write(
            git_dir.join("HEAD"),
            format!("ref: refs/heads/{initial_branch}\n"),
        )?;
        std::fs::write(git_dir.join("description"), "mini-git repository\n")?;
        std::fs::write(git_dir.join("info/exclude"), "# mg init: no excludes\n")?;
        std::fs::write(git_dir.join("config"), default_config())?;
        Ok(Repo {
            workdir: Some(workdir),
            git_dir,
        })
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// 工作区根目录；bare 仓库为 `None`。
    pub fn workdir(&self) -> Option<&Path> {
        self.workdir.as_deref()
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.git_dir.join("objects")
    }

    pub fn index_path(&self) -> PathBuf {
        self.git_dir.join("index")
    }

    pub fn head_path(&self) -> PathBuf {
        self.git_dir.join("HEAD")
    }

    pub fn config(&self) -> Config {
        Config::load(&self.git_dir.join("config"))
    }

    /// 把仓库相对路径（`/` 分隔）解析为工作区绝对路径，并拒绝逃逸出工作区的路径。
    pub fn work_path(&self, rel: &[u8]) -> Result<PathBuf> {
        let workdir = self
            .workdir
            .as_ref()
            .ok_or(Error::Unsupported("bare repository has no working tree"))?;
        let rel = std::str::from_utf8(rel)
            .map_err(|_| Error::Other("non-UTF8 paths are not supported in v1".into()))?;
        if rel
            .split('/')
            .any(|seg| seg == ".." || seg == "." || seg.is_empty())
        {
            return Err(Error::Other(format!("unsafe path in index: {rel:?}")));
        }
        Ok(workdir.join(rel))
    }
}

fn is_git_dir(path: &Path) -> bool {
    path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir()
}

fn default_config() -> String {
    [
        "[core]",
        "\trepositoryformatversion = 0",
        "\tfilemode = true",
        "\tbare = false",
        "\tlogallrefupdates = true",
        "",
    ]
    .join("\n")
}

/// 极简 git config 解析：只支持 `get("section.key")` 与 `section.sub.key` 形式。
/// 不做 include、不做多值、不做类型转换（v1 明确范围）。
#[derive(Debug, Default, Clone)]
pub struct Config {
    entries: Vec<(String, String)>,
}

impl Config {
    pub fn load(path: &Path) -> Config {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Config::default();
        };
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Config {
        let mut entries = Vec::new();
        let mut section = String::new();
        for raw in text.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim();
                section = if let Some((head, sub)) = name.split_once('"') {
                    let head = head.trim_end_matches('.').trim();
                    let sub = sub.trim_end_matches('"');
                    format!("{head}.{sub}")
                } else {
                    name.to_string()
                };
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let full = if section.is_empty() {
                key
            } else {
                format!("{}.{key}", section.to_ascii_lowercase())
            };
            entries.push((full, value.trim().to_string()));
        }
        Config { entries }
    }

    /// `key` 大小写不敏感，形如 `"core.bare"`。
    pub fn get(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1"
            )
        })
    }

    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' | ';' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config() {
        let cfg = Config::parse(
            "[core]\n\tbare = false\nfilemode = TRUE\n[remote \"origin\"]\n\turl = /tmp/x\n",
        );
        assert_eq!(cfg.get("core.bare"), Some("false"));
        assert_eq!(cfg.get_bool("core.filemode"), Some(true));
        assert_eq!(cfg.get("remote.origin.url"), Some("/tmp/x"));
        assert_eq!(cfg.get("missing.key"), None);
    }

    #[test]
    fn discover_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init(tmp.path(), DEFAULT_INITIAL_BRANCH).unwrap();
        let nested = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        let found = Repo::discover(&nested).unwrap();
        assert_eq!(found.git_dir(), repo.git_dir());
    }
}
