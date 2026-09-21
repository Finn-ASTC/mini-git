#!/usr/bin/env python3
"""T11 end-to-end parity harness: `mg` on repo A, real `git` on repo B, same sequence.

Truth comes from the real `git` binary and the filesystem only; nothing is hardcoded.
Every case runs on two parallel copies of the same starting repository.
"""
import os
import shutil
import subprocess
import sys
import filecmp

ROOT = "/tmp/w3-t11-omp"
WORK = os.path.join(ROOT, "e2e-work")
MG = "/home/user/Projects/mini-git/target/debug/mg"
GITC = ["git", "-c", "user.name=t", "-c", "user.email=t@e",

        "-c", "init.defaultBranch=main", "-c", "core.filemode=true"]

BASE_ENV = {
    "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
    "LC_ALL": "C",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_TERMINAL_PROMPT": "0",
}

RESULTS = []


def git(cwd, *args, check=True, env=None):
    e = dict(BASE_ENV)
    e["HOME"] = cwd
    if env:
        e.update(env)
    p = subprocess.run(GITC + list(args), cwd=cwd, env=e, capture_output=True)
    if check and p.returncode != 0:
        raise RuntimeError(f"git {args} failed in {cwd}: {p.stderr.decode(errors='replace')}")
    return p


def gitout(cwd, *args):
    return git(cwd, *args).stdout.decode(errors="replace")


def mg(cwd, *args, check=None):
    p = subprocess.run([MG] + list(args), cwd=cwd, env=dict(BASE_ENV), capture_output=True)
    if check is True and p.returncode != 0:
        raise RuntimeError(f"mg {args} failed: {p.stderr.decode(errors='replace')}")
    if check is False and p.returncode == 0:
        raise RuntimeError(f"mg {args} unexpectedly succeeded: {p.stdout.decode(errors='replace')}")
    return p


# ------------------------------------------------------------------ snapshots


def snapshot(root):
    """path -> kind + payload; ignores `.git`."""
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if not (dirpath == root and d == ".git")]
        for name in filenames + dirnames:
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, root).encode("utf-8", "surrogateescape")
            st = os.lstat(full)
            if os.path.islink(full):
                out[rel] = ("link", os.readlink(full).encode("utf-8", "surrogateescape"))
            elif os.path.isdir(full):
                out[rel] = ("dir", b"")
            else:
                with open(full, "rb") as fh:
                    out[rel] = ("file", fh.read(), bool(st.st_mode & 0o111))
    return out


def tree_sha(root):
    """sha256 over the whole worktree snapshot: used to prove 'nothing moved'."""
    h = {}
    for path, value in snapshot(root).items():
        h[path] = repr(value).encode()
    return h


def scratch(name):
    d = os.path.join(WORK, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    return d


def copy(src, dst):
    shutil.rmtree(dst, ignore_errors=True)
    subprocess.run(["cp", "-a", src, dst], check=True)
    return dst


def parity(a, b, name, checks=("worktree", "index", "status", "head", "branches", "orig")):
    ok = True
    if "worktree" in checks:
        sa, sb = snapshot(a), snapshot(b)
        if sa != sb:
            ok = False
            diff = sorted(set(sa) ^ set(sb)) or [
                k for k in sa if sa[k] != sb.get(k)
            ]
            fail(f"{name}: worktree differs: {diff[:6]}")
    if "index" in checks:
        ia, ib = gitout(a, "ls-files", "--stage"), gitout(b, "ls-files", "--stage")
        if ia != ib:
            ok = False
            fail(f"{name}: index differs:\n  A(mg)={ia!r}\n  B(git)={ib!r}")
    if "status" in checks:
        sa, sb = gitout(a, "status", "--porcelain"), gitout(b, "status", "--porcelain")
        if sa != sb:
            ok = False
            fail(f"{name}: status differs:\n  A(mg)={sa!r}\n  B(git)={sb!r}")
    if "head" in checks:
        with open(os.path.join(a, ".git", "HEAD"), "rb") as fa, open(
            os.path.join(b, ".git", "HEAD"), "rb"
        ) as fb:
            ha, hb = fa.read(), fb.read()
        if ha != hb:
            ok = False
            fail(f"{name}: .git/HEAD differs: {ha!r} vs {hb!r}")
        ra, rb = gitout(a, "rev-parse", "HEAD"), gitout(b, "rev-parse", "HEAD")
        if ra != rb:
            ok = False
            fail(f"{name}: HEAD commit differs: {ra!r} vs {rb!r}")
    if "branches" in checks:
        ra, rb = gitout(a, "show-ref"), gitout(b, "show-ref")
        if ra != rb:
            ok = False
            fail(f"{name}: refs differ:\n  A(mg)={ra!r}\n  B(git)={rb!r}")
    if "orig" in checks:
        pa, pb = os.path.join(a, ".git", "ORIG_HEAD"), os.path.join(b, ".git", "ORIG_HEAD")
        ea, eb = os.path.exists(pa), os.path.exists(pb)
        if ea != eb:
            ok = False
            fail(f"{name}: ORIG_HEAD presence differs: {ea} vs {eb}")
        elif ea:
            with open(pa, "rb") as fa, open(pb, "rb") as fb:
                ba, bb = fa.read(), fb.read()
            if ba != bb:
                ok = False
                fail(f"{name}: ORIG_HEAD differs: {ba!r} vs {bb!r}")
    return ok


FAILED = []


def fail(msg):
    FAILED.append(msg)
    print("FAIL " + msg)


def ok(name, detail=""):
    RESULTS.append(name)
    print(f"ok   {name} {detail}")


# ------------------------------------------------------------------ scenario scaffolding


def base_repo(name):
    """A repository with nested dirs, exec bit, symlink, empty file, non-UTF8 name."""
    d = scratch(name)
    git(d, "init", "-q")
    write(d, "keep.txt", "keep\n")
    write(d, "dir/one.txt", "one\n")
    write(d, "dir/sub/two.txt", "two\n")
    write(d, "run.sh", "#!/bin/sh\necho hi\n")
    os.chmod(os.path.join(d, "run.sh"), 0o755)
    write(d, "empty.txt", "")
    os.symlink("keep.txt", os.path.join(d, "link.txt"))
    write(d, "weird\udcff.txt", "weird\n")
    git(d, "add", "-A")
    git(d, "commit", "-q", "-m", "base")
    git(d, "branch", "feature")
    git(d, "checkout", "-q", "feature")
    git(d, "rm", "-q", "dir/sub/two.txt", "link.txt", "empty.txt")
    write(d, "dir/one.txt", "ONE\n")
    write(d, "new/deep/three.txt", "three\n")
    write(d, "script", "#!/bin/sh\n")
    os.chmod(os.path.join(d, "script"), 0o755)
    git(d, "add", "-A")
    git(d, "commit", "-q", "-m", "feature")
    git(d, "checkout", "-q", "main")
    return d


def write(root, rel, text):
    # rel may contain a lone surrogate to name a non-UTF8 file
    path = os.path.join(root, rel.encode("utf-8", "surrogateescape").decode("utf-8", "surrogateescape"))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def side_pair(name):
    """Build the base repo, then hand out an mg-side copy and a git-side copy."""
    base = base_repo(name)
    a = copy(base, os.path.join(WORK, name + "-mg"))
    b = copy(base, os.path.join(WORK, name + "-git"))
    return a, b


# ------------------------------------------------------------------ cases


def case_switch_plain():
    a, b = side_pair("switch-plain")
    mg(a, "switch", "feature", check=True)
    git(b, "checkout", "-q", "feature")
    if parity(a, b, "switch-plain"):
        ok("switch plain branch (multi-dir/exec/symlink/empty/non-UTF8)", "mg switch feature == git checkout feature")


def case_switch_create():
    a, b = side_pair("switch-create")
    mg(a, "switch", "-c", "from-feature", "feature", check=True)
    git(b, "switch", "-q", "-c", "from-feature", "feature")
    good = parity(a, b, "switch-create")
    mg(a, "switch", "main", check=True)
    git(b, "switch", "-q", "main")
    good &= parity(a, b, "switch-create-back")
    mg(a, "switch", "-c", "second", "feature", check=True)
    git(b, "switch", "-q", "-c", "second", "feature")
    good &= parity(a, b, "switch-create-again")
    if good:
        ok("switch -c (create, switch back, switch away again)")


def case_checkout_detached():
    a, b = side_pair("checkout-detach")
    oid = gitout(a, "rev-parse", "feature").strip()
    mg(a, "checkout", oid, check=True)
    git(b, "checkout", "-q", oid)
    good = parity(a, b, "checkout-detached")
    mg(a, "checkout", "--detach", "main", check=True)
    git(b, "checkout", "-q", "--detach", "main")
    good &= parity(a, b, "checkout-detach-force")
    if good:
        ok("checkout detached oid and --detach on a branch")


def case_checkout_paths():
    a, b = side_pair("checkout-paths")
    # local modifications that the path form must overwrite (git does that without -f)
    write(a, "dir/one.txt", "LOCAL\n")
    write(b, "dir/one.txt", "LOCAL\n")
    write(a, "dir/sub/untracked.txt", "u\n")
    write(b, "dir/sub/untracked.txt", "u\n")
    mg(a, "checkout", "feature", "--", "dir", check=True)
    git(b, "checkout", "-q", "feature", "--", "dir")
    good = parity(a, b, "checkout-paths-dir")
    mg(a, "checkout", "main", "--", "run.sh", check=True)
    git(b, "checkout", "-q", "main", "--", "run.sh")
    good &= parity(a, b, "checkout-paths-file")
    if good:
        ok("checkout <rev> -- <dir|file> (worktree + index, other paths untouched)")


def case_reset_modes():
    good = True
    for mode, flag in (("soft", "--soft"), ("mixed", "--mixed"), ("hard", "--hard")):
        a, b = side_pair("reset-" + mode)
        # local modification + staged change + untracked file
        write(a, "dir/one.txt", "DIRTY\n")
        write(b, "dir/one.txt", "DIRTY\n")
        write(a, "staged.txt", "staged\n")
        write(b, "staged.txt", "staged\n")
        git(a, "add", "staged.txt")
        git(b, "add", "staged.txt")
        write(a, "untracked.txt", "u\n")
        write(b, "untracked.txt", "u\n")
        first = gitout(a, "rev-list", "--max-parents=0", "HEAD").strip()
        mg(a, "reset", flag, first, check=True)
        git(b, "reset", "-q", flag, first)
        good &= parity(a, b, "reset-" + mode)
        if mode == "hard" and not os.path.isfile(os.path.join(a, "untracked.txt")):
            good = False
            fail("reset-hard: untracked file was removed")

    # the acceptance criterion for --hard: a clean status, diff against git empty.
    for dirty in (False, True):
        a, b = side_pair(f"reset-hard-clean-{dirty}")
        if dirty:
            write(a, "dir/one.txt", "DIRTY\n")
            write(b, "dir/one.txt", "DIRTY\n")
        first = gitout(a, "rev-list", "--max-parents=0", "HEAD").strip()
        mg(a, "reset", "--hard", first, check=True)
        git(b, "reset", "-q", "--hard", first)
        st = gitout(a, "status", "--porcelain")
        if st != "" or gitout(b, "status", "--porcelain") != "":
            good = False
            fail(f"reset-hard(clean={dirty}): status not clean: A={st!r} B={gitout(b, 'status', '--porcelain')!r}")
        good &= parity(a, b, f"reset-hard-clean-{dirty}")
    if good:
        ok("reset --soft/--mixed/--hard with local + staged state; --hard leaves a clean status")


def case_refusals():
    good = True
    for cmd in ("switch", "checkout"):
        a, b = side_pair("refuse-" + cmd)
        write(a, "dir/one.txt", "DIRTY\n")
        write(b, "dir/one.txt", "DIRTY\n")
        before_a, before_b = tree_sha(a), tree_sha(b)
        pa = mg(a, cmd, "feature", check=False)
        pb = git(b, "checkout", "feature", check=False)
        if pa.returncode == 0 or pb.returncode == 0:
            good = False
            fail(f"refuse-{cmd}: expected both to refuse (mg={pa.returncode}, git={pb.returncode})")
        if tree_sha(a) != before_a:
            good = False
            fail(f"refuse-{cmd}: mg changed the worktree while refusing")
        if tree_sha(b) != before_b:
            good = False
            fail(f"refuse-{cmd}: git changed the worktree while refusing")
        if not parity(a, b, "refuse-" + cmd):
            good = False
    # untracked file in the way
    a, b = side_pair("refuse-untracked")
    write(a, "new/deep/three.txt", "LOCAL\n")
    write(b, "new/deep/three.txt", "LOCAL\n")
    before_a, before_b = tree_sha(a), tree_sha(b)
    pa = mg(a, "switch", "feature", check=False)
    pb = git(b, "checkout", "feature", check=False)
    if pa.returncode == 0 or pb.returncode == 0:
        good = False
        fail(f"refuse-untracked: expected both to refuse (mg={pa.returncode}, git={pb.returncode})")
    if tree_sha(a) != before_a or tree_sha(b) != before_b:
        good = False
        fail("refuse-untracked: worktree changed while refusing")
    if good:
        ok("refusals: dirty + untracked barriers refuse, worktree byte-identical")


def case_deletion_and_untracked_survival():
    a, b = side_pair("delete")
    for d in (a, b):
        write(d, "untracked.txt", "u\n")
        write(d, "noise.log", "i\n")
    for d in (a, b):
        with open(os.path.join(d, ".gitignore"), "w") as fh:
            fh.write("*.log\n")
        git(d, "add", ".gitignore")
        git(d, "commit", "-q", "-m", "ignore")
    a2, b2 = copy(a, os.path.join(WORK, "delete-copy")), None
    # main lacks dir/sub/two.txt; feature has it → switching to main must delete it
    mg(a, "switch", "main", check=True)
    git(b, "checkout", "-q", "main")
    good = parity(a, b, "delete-tracking")
    for d in (a, b):
        if not os.path.isfile(os.path.join(d, "untracked.txt")):
            good = False
            fail("delete: untracked file disappeared")
        if not os.path.isfile(os.path.join(d, "noise.log")):
            good = False
            fail("delete: ignored file disappeared")
    if good:
        ok("deletion from the worktree keeps untracked + ignored files")


def case_branch_command():
    a, b = side_pair("branch")
    # list parity (compare mg's set with git's --format list)
    got = mg(a, "branch", check=True).stdout.decode().split()
    got = {t for t in got if t != "*"}
    want = set(gitout(b, "branch", "--list", "--format=%(refname:short)").split())
    good = got == want
    if not good:
        fail(f"branch list differs: mg={sorted(got)} git={sorted(want)}")
    # `-l <pattern>` parity against `git branch --list <pattern>`
    for pat in ["feat*", "*x", "*/*", "?eat*", "[fo]*", "main", "nothing*"]:
        m = {t for t in mg(a, "branch", "-l", pat, check=True).stdout.decode().split() if t != "*"}
        g = set(gitout(b, "branch", "--list", pat, "--format=%(refname:short)").split())
        if m != g:
            good = False
            fail(f"branch -l {pat!r} differs: mg={sorted(m)} git={sorted(g)}")
    # create
    mg(a, "branch", "topic", "feature", check=True)
    git(b, "branch", "topic", "feature")
    good &= parity(a, b, "branch-create")
    # create an existing one → refuse
    if mg(a, "branch", "topic", check=False).returncode == 0:
        good = False
        fail("branch: creating an existing branch must be refused")
    # delete unmerged → refuse, merged → accept
    if mg(a, "branch", "-d", "topic", check=False).returncode == 0:
        good = False
        fail("branch -d: deleting an unmerged branch must be refused")
    if mg(a, "branch", "-d", "main", check=False).returncode == 0:
        good = False
        fail("branch -d: deleting the current branch must be refused")
    merged = gitout(b, "rev-parse", "main").strip()
    mg(a, "branch", "merged-branch", "main", check=True)
    git(b, "branch", "merged-branch", "main")
    mg(a, "branch", "-d", "merged-branch", check=True)
    git(b, "branch", "-d", "merged-branch")
    good &= parity(a, b, "branch-delete-merged")
    # rename
    mg(a, "branch", "-m", "topic", "renamed", check=True)
    git(b, "branch", "-m", "topic", "renamed")
    good &= parity(a, b, "branch-rename")
    mg(a, "branch", "-m", "current-renamed", check=True)
    git(b, "branch", "-m", "current-renamed")
    good &= parity(a, b, "branch-rename-current")
    mg(a, "switch", "renamed", check=True)
    git(b, "switch", "-q", "renamed")
    good &= parity(a, b, "branch-after-renames")
    if good:
        ok("branch list/create/delete(-d merged rule)/rename(-m two forms)")


def case_checkout_tag():
    a, b = side_pair("checkout-tag")
    git(a, "tag", "v1", "feature")
    git(b, "tag", "v1", "feature")
    mg(a, "checkout", "v1", check=True)
    git(b, "checkout", "-q", "v1")
    if parity(a, b, "checkout-tag"):
        ok("checkout an annotated-less tag detaches HEAD (same as git)")


def case_same_branch_noop():
    a, b = side_pair("same-branch")
    for d in (a, b):
        write(d, "dir/one.txt", "STAGED\n")
        git(d, "add", "dir/one.txt")
    mg(a, "checkout", "main", check=True)
    git(b, "checkout", "main")
    good = parity(a, b, "same-branch-checkout")
    mg(a, "switch", "main", check=True)
    git(b, "switch", "main")
    good &= parity(a, b, "same-branch-switch")
    if good:
        ok("checkout/switch of the current branch is a no-op even with staged changes")


def case_pathspec_miss():
    a, b = side_pair("pathspec")
    before = tree_sha(a)
    pa = mg(a, "checkout", "main", "--", "nope.txt", check=False)
    pb = git(b, "checkout", "main", "--", "nope.txt", check=False)
    if pa.returncode == 0 or pb.returncode == 0:
        fail(f"pathspec miss: expected both to fail (mg={pa.returncode}, git={pb.returncode})")
        return
    if b"did not match any file" not in pa.stderr:
        fail(f"pathspec miss: unexpected mg error {pa.stderr!r}")
        return
    if tree_sha(a) != before:
        fail("pathspec miss: worktree changed")
        return
    if not parity(a, b, "pathspec-miss"):
        return
    ok("checkout <rev> -- <missing path> fails without touching anything")


def case_counterexamples():
    a, b = side_pair("counter")
    good = True
    p = mg(a, "switch", "ghost", check=False)
    if p.returncode == 0 or b"reference not found" not in p.stderr:
        good = False
        fail(f"switch ghost: rc={p.returncode} err={p.stderr!r}")
    p = mg(a, "checkout", "ghost", check=False)
    if p.returncode == 0 or b"not found" not in p.stderr:
        good = False
        fail(f"checkout ghost: rc={p.returncode} err={p.stderr!r}")
    p = mg(a, "reset", "--hard", "ghost", check=False)
    if p.returncode == 0 or b"not found" not in p.stderr:
        good = False
        fail(f"reset ghost: rc={p.returncode} err={p.stderr!r}")
    p = mg(a, "checkout", "0123456789abcdef0123456789abcdef01234567", check=False)
    if p.returncode == 0 or b"object not found" not in p.stderr:
        good = False
        fail(f"checkout missing oid: rc={p.returncode} err={p.stderr!r}")
    p = mg(a, "switch", "-c", "x", "0123456789abcdef0123456789abcdef01234567", check=False)
    if p.returncode == 0 or b"object not found" not in p.stderr:
        good = False
        fail(f"switch -c missing oid: rc={p.returncode} err={p.stderr!r}")
    for err in FAILED:
        if "panicked" in err:
            good = False
    # nothing may have moved: same bytes as a fresh copy of the base repo
    base = base_repo("counter-base")
    if tree_sha(a) != tree_sha(base):
        good = False
        fail("counterexamples: worktree changed after failed commands")
    if gitout(a, "ls-files", "--stage") != gitout(base, "ls-files", "--stage"):
        good = False
        fail("counterexamples: index changed after failed commands")
    if good:
        ok("counterexamples: RefNotFound / ObjectNotFound, no panic, no leftovers")


def case_symlink_escape():
    outside = scratch("outside")
    a = scratch("escape-src")
    git(a, "init", "-q")
    write(a, "keep.txt", "keep\n")
    git(a, "add", "-A")
    git(a, "commit", "-q", "-m", "base")
    git(a, "branch", "feature")
    git(a, "checkout", "-q", "feature")
    write(a, "a/b/c.txt", "inside\n")
    git(a, "add", "-A")
    git(a, "commit", "-q", "-m", "feature")
    git(a, "checkout", "-q", "main")
    os.symlink(outside, os.path.join(a, "a"))

    p = mg(a, "switch", "feature", check=False)
    escaped = os.listdir(outside)
    inside = os.path.islink(os.path.join(a, "a"))
    good = True
    if escaped:
        good = False
        fail(f"symlink escape: wrote outside the worktree: {escaped}")
    if p.returncode == 0:
        # allowed only if it removed the symlink and stayed inside
        if inside:
            good = False
            fail("symlink escape: succeeded but left the escaping symlink in place")
        canon = os.path.realpath(os.path.join(a, "a"))
        if not canon.startswith(os.path.realpath(a) + os.sep):
            good = False
            fail(f"symlink escape: canonicalized inside path escapes: {canon}")
    else:
        if not inside:
            good = False
            fail("symlink escape: refused but removed the symlink anyway")
    if good:
        ok(f"symlink escape blocked (mg rc={p.returncode}, outside dir empty)")


def case_force_overwrite():
    good = True
    # `-f` on the same branch resets the worktree (git E24) …
    a, b = side_pair("force-same")
    for d in (a, b):
        write(d, "dir/one.txt", "DIRTY\n")
    mg(a, "checkout", "-f", "main", check=True)
    git(b, "checkout", "-q", "-f", "main")
    good &= parity(a, b, "force-same-branch")
    # … and `-f` overwrites untracked files that are in the way (git E25)
    a, b = side_pair("force-untracked")
    for d in (a, b):
        write(d, "dir/one.txt", "DIRTY\n")
        write(d, "new/deep/three.txt", "LOCAL\n")
    mg(a, "switch", "-f", "feature", check=True)
    git(b, "switch", "-q", "-f", "feature")
    good &= parity(a, b, "force-untracked")
    if good:
        ok("force (-f) resets the same branch and overwrites untracked barriers")


def case_ignored_barrier():
    a, b = side_pair("ignored-barrier")
    for d in (a, b):
        with open(os.path.join(d, ".gitignore"), "w") as fh:
            fh.write("new/\n")
        git(d, "add", ".gitignore")
        git(d, "commit", "-q", "-m", "ignore new/")
        write(d, "new/deep/three.txt", "LOCAL-IGNORED\n")
    mg(a, "switch", "feature", check=True)
    git(b, "checkout", "-q", "feature")
    if parity(a, b, "ignored-barrier"):
        ok("ignored file in the way is overwritten (git behaviour)")


def main():
    shutil.rmtree(WORK, ignore_errors=True)
    os.makedirs(WORK)
    if not os.path.exists(MG):
        raise SystemExit("mg binary not built")
    for case in [
        case_switch_plain,
        case_switch_create,
        case_checkout_detached,
        case_checkout_tag,
        case_checkout_paths,
        case_same_branch_noop,
        case_pathspec_miss,
        case_reset_modes,
        case_refusals,
        case_deletion_and_untracked_survival,
        case_branch_command,
        case_counterexamples,
        case_symlink_escape,
        case_ignored_barrier,
        case_force_overwrite,
    ]:
        print(f"--- {case.__name__}")
        case()
    print()
    print(f"{len(RESULTS)} case(s) passed, {len(FAILED)} failure line(s)")
    for line in FAILED:
        print("  " + line)
    return 1 if FAILED else 0


if __name__ == "__main__":
    sys.exit(main())
