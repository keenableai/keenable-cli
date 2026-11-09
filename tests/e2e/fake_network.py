"""An offline stand-in for api.keenable.ai behind a TLS-inspecting proxy.

`FakeInspectingProxy` is an HTTP CONNECT proxy on 127.0.0.1 that tunnels
every CONNECT to a local TLS server presenting an api.keenable.ai
certificate signed by a throwaway CA. Point HTTPS_PROXY at `proxy_url` and
SSL_CERT_FILE at `ca_file`, and the CLI talks to the fake API without any
network access. `fail_connects` makes the first N CONNECTs fail with 502,
which the CLI sees as a transport error.
"""

import http.server
import json
import shutil
import socket
import ssl
import subprocess
import threading
from pathlib import Path

RESULTS = {"query": "fake", "results": [{"title": "Fake inspected result", "url": "https://example.com/"}]}


def openssl_available() -> bool:
    return shutil.which("openssl") is not None


def _make_certs(d: Path) -> None:
    def run(*args):
        subprocess.run(["openssl", *args], cwd=d, check=True, capture_output=True)

    (d / "ca.cnf").write_text(
        "[req]\ndistinguished_name=dn\nx509_extensions=v3\nprompt=no\n"
        "[dn]\nCN=keenable e2e fake inspection CA\n"
        "[v3]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n"
    )
    run("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "ca.key",
        "-out", "ca.pem", "-days", "2", "-config", "ca.cnf")
    run("req", "-newkey", "rsa:2048", "-nodes", "-keyout", "srv.key", "-out", "srv.csr",
        "-subj", "/CN=api.keenable.ai")
    (d / "srv.ext").write_text(
        "subjectAltName=DNS:api.keenable.ai\nbasicConstraints=CA:FALSE\n"
        "extendedKeyUsage=serverAuth\n"
    )
    run("x509", "-req", "-in", "srv.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
        "-CAcreateserial", "-out", "srv.pem", "-days", "2", "-extfile", "srv.ext")


class _Api(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _reply(self):
        body = json.dumps(RESULTS).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self._reply()

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self._reply()

    def log_message(self, *args):
        pass


class FakeInspectingProxy:
    def __init__(self, workdir: Path, fail_connects: int = 0):
        _make_certs(workdir)
        self.ca_file = str(workdir / "ca.pem")
        self.fail_connects = fail_connects
        self.connects = 0
        self._lock = threading.Lock()

        self._api = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Api)
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.minimum_version = ssl.TLSVersion.TLSv1_2
        ctx.load_cert_chain(str(workdir / "srv.pem"), str(workdir / "srv.key"))
        self._api.socket = ctx.wrap_socket(self._api.socket, server_side=True)
        self._api_port = self._api.server_address[1]

        self._sock = socket.socket()
        self._sock.bind(("127.0.0.1", 0))
        self._sock.listen(50)
        self.port = self._sock.getsockname()[1]
        self.proxy_url = f"http://127.0.0.1:{self.port}"

        for target in (self._api.serve_forever, self._accept_loop):
            threading.Thread(target=target, daemon=True).start()

    def _accept_loop(self):
        while True:
            try:
                conn, _ = self._sock.accept()
            except OSError:
                return
            threading.Thread(target=self._tunnel, args=(conn,), daemon=True).start()

    def _tunnel(self, conn):
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = conn.recv(4096)
            if not chunk:
                conn.close()
                return
            buf += chunk
        with self._lock:
            self.connects += 1
            fail = self.connects <= self.fail_connects
        if fail:
            conn.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
            conn.close()
            return
        upstream = socket.create_connection(("127.0.0.1", self._api_port))
        conn.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
        threading.Thread(target=_pipe, args=(conn, upstream), daemon=True).start()
        _pipe(upstream, conn)

    def close(self):
        self._sock.close()
        self._api.shutdown()


def _pipe(src, dst):
    try:
        while data := src.recv(65536):
            dst.sendall(data)
    except OSError:
        pass
    finally:
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
