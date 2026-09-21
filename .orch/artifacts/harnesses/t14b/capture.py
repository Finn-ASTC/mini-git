#!/usr/bin/env python3
"""Capture raw request head bytes from a real `git` client, forwarding to
`git http-backend` (CGI). Prints JSON lines to stdout for each request and
the port on stderr. Used by T14b for the byte-for-byte comparison.
"""
import json
import os
import socket
import subprocess
import sys
import threading

ROOT = sys.argv[1]
records = []
lock = threading.Lock()


def read_head(conn):
    data = b""
    while b"\r\n\r\n" not in data and len(data) < 1_000_000:
        chunk = conn.recv(1)
        if not chunk:
            break
        data += chunk
    return data


def parse_head(head):
    text = head.decode("latin1")
    lines = text.split("\r\n")
    rl = lines[0].split(" ")
    method, target = rl[0], rl[1]
    headers = []
    for line in lines[1:]:
        if not line:
            continue
        n, _, v = line.partition(":")
        headers.append((n.strip(), v.strip()))
    return method, target, headers


def run_backend(method, target, headers, body):
    path, _, query = target.partition("?")
    env = dict(os.environ)
    env.update(
        {
            "GIT_PROJECT_ROOT": ROOT,
            "GIT_HTTP_EXPORT_ALL": "1",
            "PATH_INFO": path,
            "QUERY_STRING": query,
            "REQUEST_METHOD": method,
            "REMOTE_ADDR": "127.0.0.1",
            "SERVER_PROTOCOL": "HTTP/1.1",
        }
    )
    for n, v in headers:
        ln = n.lower()
        if ln == "content-type":
            env["CONTENT_TYPE"] = v
        elif ln == "content-length":
            env["CONTENT_LENGTH"] = v
        elif ln == "git-protocol":
            env["GIT_PROTOCOL"] = v
    p = subprocess.run(
        ["git", "http-backend"],
        input=body,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    return p.stdout, p.stderr


def handle(conn):
    conn.settimeout(15)
    head = read_head(conn)
    if not head:
        conn.close()
        return
    method, target, headers = parse_head(head)
    clen = 0
    for n, v in headers:
        if n.lower() == "content-length":
            clen = int(v)
    body = b""
    while len(body) < clen:
        chunk = conn.recv(clen - len(body))
        if not chunk:
            break
        body += chunk
    with lock:
        records.append({"head": head.decode("latin1"), "body_bytes": len(body), "seq": len(records)})
    out, err = run_backend(method, target, headers, body)
    if b"\r\n\r\n" not in out:
        out = err
    split = out.find(b"\r\n\r\n")
    header_block, body_out = out[:split], out[split + 4 :]
    status_line = b"HTTP/1.1 200 OK\r\n"
    if header_block.startswith(b"Status:"):
        first, _, rest = header_block.partition(b"\r\n")
        code = first.split(b" ", 1)[1]
        header_block = rest
        status_line = b"HTTP/1.1 " + code + b"\r\n"
    resp = status_line
    for line in header_block.split(b"\r\n"):
        if line and not line.lower().startswith(b"status:"):
            resp += line + b"\r\n"
    resp += b"Content-Length: " + str(len(body_out)).encode() + b"\r\nConnection: close\r\n\r\n"
    resp += body_out
    try:
        conn.sendall(resp)
    except OSError:
        pass
    conn.close()


def main():
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(16)
    port = listener.getsockname()[1]
    print(f"PORT={port}", file=sys.stderr, flush=True)
    sys.stdout.write(json.dumps({"port": port}) + "\n")
    sys.stdout.flush()
    while True:
        conn, _ = listener.accept()
        t = threading.Thread(target=handle, args=(conn,))
        t.start()
        t.join()
        with lock:
            sys.stdout.write(json.dumps({"records": list(records)}) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()
