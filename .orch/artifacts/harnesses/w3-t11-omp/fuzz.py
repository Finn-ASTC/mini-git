#!/usr/bin/env python3
"""Randomized differential test: `mg` (A) vs real `git` (B) on identical random states.

Every iteration builds a random repository with real git, leaves the worktree in a random
dirty state, then runs one random mg/git command pair and compares the whole surface
(worktree incl. modes/symlinks/dirs, index, status, HEAD, refs, ORIG_HEAD).
A seed makes any mismatch reproducible.
"""
import os
import random
import shutil
import subprocess
import sys

ROOT = "/tmp/w3-t11-omp"
WORK = os.path.join(ROOT, "fuzz-work")
MG = "/home/user/Projects/mini-git/target/debug/mg"
GITC = ["git", "-c", "user.name=t", "-c", "user.email=t@e", "-c", "init.defaultBranch=main",
        "-c", "gc.auto=0", "-c", "maintenance.auto=false", "-c", "core.fsmonitor=false"]
ENV = {
    "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
    "LC_ALL": "C",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_TERMINAL_PROMPT": "0",
    "HOME": "/tmp/w3-t11-omp/fuzz-home",
}
os.makedirs(ENV["HOME"], exist_ok=True)

NAMES = ["a.txt", "b.txt", "dir/c.txt", "dir/d.txt", "dir/sub/e.txt", "s p.txt",
         "up\xff.txt", "run.sh", "empty.txt", "z/deep/deeper/f.txt", "link.txt"]


def git(cwd, *args, check=True):
    p = subprocess.run(GITC + list(args), cwd=cwd, env=ENV, capture_output=True)
    if check and p.returncode != 0:
        raise RuntimeError(f"git {args}: {p.stderr.decode(errors='replace')}")
    return p


def gitout(cwd, *args):
    return git(cwd, *args).stdout.decode(errors="replace")


def mg(cwd, *args):
    return subprocess.run([MG] + list(args), cwd=cwd, env=ENV, capture_output=True)


def snapshot(root):
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if not (dirpath == root and d == ".git")]
        for name in list(filenames) + list(dirnames):
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, root).encode("utf-8", "surrogateescape")
            st = os.lstat(full)
            if os.path.islink(full):
                out[rel] = ("link", os.readlink(full).encode("utf-8", "surrogateescape"))
            elif os.path.isdir(full):
                out[rel] = ("dir",)
            else:
                with open(full, "rb") as fh:
                    out[rel] = ("file", fh.read(), bool(st.st_mode & 0o111))
    return out


def build(seed):
    rng = random.Random(seed)
    base = os.path.join(WORK, "base")
    subprocess.run(["rm", "-rf", base], check=True)
    os.makedirs(base)
    git(base, "init", "-q")
    with open(os.path.join(base, ".gitignore"), "w") as fh:
        fh.write("*.log\nignored/\n")
    live = set()
    for _ in range(3):  # commits
        for name in rng.sample(NAMES, rng.randint(1, 5)):
            path = os.path.join(base, name)
            if os.path.dirname(path):
                os.makedirs(os.path.dirname(path), exist_ok=True)
            if os.path.islink(path):
                os.unlink(path)
            if name == "link.txt":
                if os.path.exists(path):
                    os.unlink(path)
                os.symlink(rng.choice(["a.txt", "dir/c.txt", "nope"]), path)
            else:
                with open(path, "w") as fh:
                    fh.write(f"{name}:{rng.randint(0, 9)}\n")
                if name == "run.sh":
                    os.chmod(path, 0o755 if rng.random() < 0.5 else 0o644)
            live.add(name)
        # deletions
        for name in sorted(live):
            if rng.random() < 0.2:
                path = os.path.join(base, name)
                if os.path.lexists(path):
                    os.unlink(path)
                live.discard(name)
        git(base, "add", "-A")
        git(base, "commit", "-q", "-m", f"c{rng.randint(0, 999)}", "--allow-empty")

    branches = []
    for i in range(rng.randint(1, 2)):
        name = f"br{i}"
        git(base, "branch", name)
        branches.append(name)

    # random dirty worktree state
    for name in sorted(live):
        r = rng.random()
        path = os.path.join(base, name)
        if r < 0.15:
            if os.path.lexists(path):
                os.unlink(path)
        elif r < 0.35 and not os.path.islink(path) and name != "link.txt":
            with open(path, "w") as fh:
                fh.write("DIRTY\n")
        elif r < 0.45 and name == "run.sh" and os.path.exists(path):
            os.chmod(path, 0o755)
    if rng.random() < 0.5:
        with open(os.path.join(base, "untracked.txt"), "w") as fh:
            fh.write("u\n")
    if rng.random() < 0.4:
        with open(os.path.join(base, "noise.log"), "w") as fh:
            fh.write("i\n")
    if rng.random() < 0.3:
        os.makedirs(os.path.join(base, "ignored"), exist_ok=True)
        with open(os.path.join(base, "ignored", "x.txt"), "w") as fh:
            fh.write("i\n")
    if rng.random() < 0.4:
        target = rng.choice(sorted(live)[:6] if live else ["a.txt"])
        probe = os.path.join(base, target)
        if os.path.isfile(probe):
            with open(probe, "w") as fh:
                fh.write("STAGED-ONLY\n")
            git(base, "add", "--", target)
            with open(probe, "w") as fh:
                fh.write("DIRTY-AFTER-STAGE\n")
    return base, branches, live, rng


def compare(a, b, label):
    problems = []
    sa, sb = snapshot(a), snapshot(b)
    if sa != sb:
        keys = sorted(set(sa) | set(sb))
        diff = [k for k in keys if sa.get(k) != sb.get(k)]
        problems.append(f"worktree differs at {diff[:5]}: A={[sa.get(k) for k in diff[:5]]} B={[sb.get(k) for k in diff[:5]]}")
    ia, ib = gitout(a, "ls-files", "--stage"), gitout(b, "ls-files", "--stage")
    if ia != ib:
        problems.append(f"index differs A={ia!r} B={ib!r}")
    sta, stb = gitout(a, "status", "--porcelain"), gitout(b, "status", "--porcelain")
    if sta != stb:
        problems.append(f"status differs A={sta!r} B={stb!r}")
    ra, rb = gitout(a, "rev-parse", "HEAD"), gitout(b, "rev-parse", "HEAD")
    if ra != rb:
        problems.append(f"HEAD differs A={ra!r} B={rb!r}")
    with open(os.path.join(a, ".git", "HEAD"), "rb") as fa, open(os.path.join(b, ".git", "HEAD"), "rb") as fb:
        if fa.read() != fb.read():
            problems.append("HEAD file differs")
    if gitout(a, "show-ref") != gitout(b, "show-ref"):
        problems.append(f"refs differ A={gitout(a, 'show-ref')!r} B={gitout(b, 'show-ref')!r}")
    for name in ("ORIG_HEAD",):
        pa, pb = os.path.join(a, ".git", name), os.path.join(b, ".git", name)
        if os.path.exists(pa) != os.path.exists(pb):
            problems.append(f"{name} presence differs")
        elif os.path.exists(pa):
            with open(pa, "rb") as fa, open(pb, "rb") as fb:
                if fa.read() != fb.read():
                    problems.append(f"{name} differs")
    return problems


def one(seed):
    base, branches, live, rng = build(seed)
    a = os.path.join(WORK, "A")
    b = os.path.join(WORK, "B")
    for dst in (a, b):
        subprocess.run(["rm", "-rf", dst], check=True)
        subprocess.run(["cp", "-a", base, dst], check=True)

    kinds = ["switch", "checkout", "checkout-detach", "force-switch", "reset-soft",
             "reset-mixed", "reset-hard", "checkout-path", "switch-create"]
    kind = rng.choice(kinds)
    branch = rng.choice(branches) if branches else "main"
    oid = gitout(base, "rev-parse", "HEAD").strip()
    root = gitout(base, "rev-list", "--max-parents=0", "HEAD").strip()

    if kind == "switch":
        mg_args, git_args = ["switch", branch], ["switch", "-q", branch]
    elif kind == "checkout":
        mg_args, git_args = ["checkout", "main"], ["checkout", "-q", "main"]
    elif kind == "checkout-detach":
        mg_args, git_args = ["checkout", "--detach", oid], ["checkout", "-q", "--detach", oid]
    elif kind == "force-switch":
        mg_args, git_args = ["switch", "-f", branch], ["switch", "-q", "-f", branch]
    elif kind == "reset-soft":
        mg_args, git_args = ["reset", "--soft", root], ["reset", "-q", "--soft", root]
    elif kind == "reset-mixed":
        mg_args, git_args = ["reset", "--mixed", root], ["reset", "-q", "--mixed", root]
    elif kind == "reset-hard":
        mg_args, git_args = ["reset", "--hard", root], ["reset", "-q", "--hard", root]
    elif kind == "switch-create":
        newb = f"new{rng.randint(0, 99)}"
        mg_args, git_args = ["switch", "-c", newb, branch], ["switch", "-q", "-c", newb, branch]
    else:
        path = rng.choice(sorted(live) + ["dir"]) if live else "dir"
        mg_args, git_args = ["checkout", "br0" if branches else "main", "--", path], \
                            ["checkout", "-q", "br0" if branches else "main", "--", path]

    pa = mg(a, *mg_args)
    pb = git(b, *git_args, check=False)
    if (pa.returncode == 0) != (pb.returncode == 0):
        return [f"exit mismatch: mg {' '.join(mg_args)} rc={pa.returncode} err={pa.stderr.decode(errors='replace').strip()!r} "
                f"git {' '.join(git_args)} rc={pb.returncode} err={pb.stderr.decode(errors='replace').strip().splitlines()[:1]!r}"]
    return compare(a, b, kind)


def main():
    subprocess.run(["rm", "-rf", WORK], check=True)
    os.makedirs(WORK)
    iterations = int(sys.argv[1]) if len(sys.argv) > 1 else 60
    bad = 0
    for seed in range(iterations):
        problems = one(seed)
        if problems:
            bad += 1
            print(f"seed {seed}: {' | '.join(problems)}")
    print(f"\n{iterations - bad}/{iterations} random scenarios matched real git exactly")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
