#!/usr/bin/env python3
"""End-to-end smoke test for the stella-mcp server.

Drives the compiled MCP server over stdio: builds a ROM from the bundled
fixture, loads it into a debug session, runs a few frames, screenshots, and
asserts the captured PNG is a full-resolution, non-blank frame.

Usage: scripts/smoke.py [path-to-stella-mcp-binary]

Requires the headless helper (built via scripts/build-emu.sh) and dasm on PATH.
Exits non-zero on any failure.
"""
import collections
import json
import os
import struct
import subprocess
import sys
import threading
import time
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target/release/stella-mcp")
FIXTURE = os.path.join(ROOT, "tests/fixtures/colorbars.asm")
WORK = os.path.join(ROOT, "target/smoke")


def decode_png(path):
    """Returns (width, height, list-of-(r,g,b)) for an 8-bit RGB or RGBA PNG."""
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    i, width, height, depth, ctype, idat = 8, 0, 0, 0, 0, b""
    while i < len(data):
        (ln,) = struct.unpack(">I", data[i : i + 4])
        typ = data[i + 4 : i + 8]
        chunk = data[i + 8 : i + 8 + ln]
        i += 12 + ln
        if typ == b"IHDR":
            width, height, depth, ctype = struct.unpack(">IIBB", chunk[:10])
        elif typ == b"IDAT":
            idat += chunk
        elif typ == b"IEND":
            break
    channels = {2: 3, 6: 4}.get(ctype)
    assert depth == 8 and channels, f"unsupported PNG (depth={depth}, colour type={ctype})"
    raw = zlib.decompress(idat)
    stride = width * channels
    assert len(raw) == height * (stride + 1), "unexpected raw size"

    def paeth(a, b, c):
        p = a + b - c
        pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
        return a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)

    pixels = []
    prev = bytearray(stride)
    pos = 0
    for _ in range(height):
        ftype = raw[pos]
        pos += 1
        line = bytearray(raw[pos : pos + stride])
        pos += stride
        for x in range(stride):
            a = line[x - 4] if x >= 4 else 0
            b = prev[x]
            c = prev[x - 4] if x >= 4 else 0
            if ftype == 1:
                line[x] = (line[x] + a) & 0xFF
            elif ftype == 2:
                line[x] = (line[x] + b) & 0xFF
            elif ftype == 3:
                line[x] = (line[x] + ((a + b) >> 1)) & 0xFF
            elif ftype == 4:
                line[x] = (line[x] + paeth(a, b, c)) & 0xFF
        for x in range(0, stride, channels):
            pixels.append((line[x], line[x + 1], line[x + 2]))
        prev = line
    return width, height, pixels


class Server:
    def __init__(self, binary):
        env = dict(os.environ, RUST_LOG="warn")
        self.p = subprocess.Popen(
            [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1, env=env,
        )
        self.resp = {}
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if "id" in msg:
                self.resp[msg["id"]] = msg

    def _send(self, obj):
        self.p.stdin.write(json.dumps(obj) + "\n")
        self.p.stdin.flush()

    def _wait(self, rid, timeout=90):
        end = time.time() + timeout
        while time.time() < end:
            if rid in self.resp:
                return self.resp[rid]
            if self.p.poll() is not None:
                raise SystemExit(f"server exited early (code {self.p.returncode})\n{self.p.stderr.read()}")
            time.sleep(0.05)
        raise SystemExit(f"timeout waiting for response {rid}")

    def call(self, rid, name, args):
        self._send({"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                    "params": {"name": name, "arguments": args}})
        r = self._wait(rid)
        if "error" in r:
            raise SystemExit(f"{name} failed: {r['error']['message']}")
        res = r["result"]
        text = "\n".join(c.get("text", "") for c in res.get("content", []) if c.get("type") == "text")
        has_image = any(c.get("type") == "image" for c in res.get("content", []))
        return text, has_image

    def initialize(self):
        self._send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "smoke", "version": "0"}}})
        self._wait(1)
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

    def list_tools(self):
        self._send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        return [t["name"] for t in self._wait(2)["result"]["tools"]]

    def stop(self):
        try:
            self.p.stdin.close()
        except Exception:
            pass
        self.p.terminate()


def main():
    if not os.path.exists(BIN):
        raise SystemExit(f"server binary not found: {BIN} (run `cargo build --release`)")
    os.makedirs(WORK, exist_ok=True)
    rom = os.path.join(WORK, "colorbars.bin")
    shot = os.path.join(WORK, "colorbars.png")

    srv = Server(BIN)
    try:
        srv.initialize()

        tools = srv.list_tools()
        for required in ("build_rom", "start_session", "step", "screenshot", "stop_session"):
            assert required in tools, f"missing tool: {required}"
        print(f"tools/list: {len(tools)} tools")

        text, _ = srv.call(3, "build_rom", {"source": FIXTURE, "output": rom})
        assert os.path.exists(rom), f"ROM not built: {text}"
        print(f"build_rom: {os.path.getsize(rom)} bytes")

        srv.call(4, "start_session", {"rom_path": rom})
        srv.call(5, "step", {"mode": "FRAME", "count": 4})
        text, has_image = srv.call(6, "screenshot", {"path": shot})
        assert has_image, "screenshot returned no inline image"
        srv.call(7, "stop_session", {})
    finally:
        srv.stop()

    w, h, pixels = decode_png(shot)
    colors = collections.Counter(pixels)
    assert w == 320, f"unexpected width {w}"
    assert h >= 100, f"unexpected height {h}"
    assert len(colors) >= 8, f"frame looks blank: only {len(colors)} colour(s)"
    print(f"screenshot: {w}x{h}, {len(colors)} distinct colours -> OK")
    print("SMOKE TEST PASSED")


if __name__ == "__main__":
    main()
