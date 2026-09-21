#!/usr/bin/env python3
"""V8 证据：用**独立实现**（本脚本，不使用被测 Rust 代码）切出真实 git 的 pkt-line 帧表。

用法: python3 frame_table.py <repo-dir>
输出: 帧表 + 原始字节流的 sha256（写到 stdout，由调用方重定向到 real-git-frames.txt）

本脚本只做两件事：跑真实 git、按 git 的格式表（<4 位小写十六进制长度><payload>，
长度含前缀；0000/0001/0002 无 payload）自己切片。它是 Rust 侧 read_pkt 的对照物。
"""
import hashlib
import subprocess
import sys


def pkt(payload: bytes) -> bytes:
    return b"%04x" % (len(payload) + 4) + payload


def frames(raw: bytes):
    off = 0
    out = []
    while off < len(raw):
        assert off + 4 <= len(raw), f"truncated header at {off}"
        length = int(raw[off:off + 4], 16)
        if length == 0:
            out.append((off, 4, "flush", b""))
            off += 4
        elif length == 1:
            out.append((off, 4, "delim", b""))
            off += 4
        elif length == 2:
            out.append((off, 4, "response-end", b""))
            off += 4
        else:
            assert length >= 4 and off + length <= len(raw), f"bad frame at {off}: {length}"
            out.append((off, length, "data", raw[off + 4:off + length]))
            off += length
    assert off == len(raw), f"consumed {off} != {len(raw)}"
    return out


def show(title: str, raw: bytes):
    print(f"### {title}")
    print(f"raw bytes: {len(raw)}  sha256: {hashlib.sha256(raw).hexdigest()}")
    for i, (off, length, kind, payload) in enumerate(frames(raw)):
        head = payload[:56].decode("latin-1").replace("\x00", "\\0").replace("\n", "\\n")
        print(f"  [{i:02d}] off={off:5d} len={length:5d} {kind:12s} payload={length - 4 if kind == 'data' else 0:5d} {head!r}")
    print()


def run(args, cwd, stdin=b""):
    out = subprocess.run(["git"] + args, cwd=cwd, input=stdin, capture_output=True)
    print(f"$ git {' '.join(args)}   -> rc={out.returncode}")
    assert out.returncode == 0, out.stderr[:400]
    return out.stdout


def main():
    repo = sys.argv[1]
    print("git --version:", subprocess.run(["git", "--version"], capture_output=True).stdout.decode().strip())
    print()

    raw = run(["-c", "protocol.version=0", "upload-pack", "--stateless-rpc", "--advertise-refs", "."], repo)
    show("真实 git：v0 ref advertisement (upload-pack --stateless-rpc --advertise-refs)", raw)

    oid = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True).stdout.strip()
    req = pkt(b"want " + oid + b" side-band-64k\n") + b"0000" + pkt(b"done\n")
    print("### 客户端 request 的帧表（本脚本按格式表自造，非 Rust 代码）")
    show("request", req)
    resp = run(["-c", "protocol.version=0", "upload-pack", "--stateless-rpc", "."], repo, req)
    show("真实 git：对上述 request 的响应（含 side-band 通道 1/2）", resp)
    pack = b"".join(p[1:] for _, _, k, p in frames(resp) if k == "data" and p[:1] == b"\x01")
    print(f"# 通道 1 重组出的 pack: {len(pack)} 字节, magic={pack[:4]!r}")


if __name__ == "__main__":
    main()
