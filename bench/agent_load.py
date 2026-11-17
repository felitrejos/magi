#!/usr/bin/env python3
import argparse
import os
import platform
import resource
import shutil
import socket
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time

PROTOCOL_VERSION = 32
HELLO = struct.pack("<I", 1) + bytes([PROTOCOL_VERSION])


def cpu_seconds(pid):
    if platform.system() == "Linux":
        with open(f"/proc/{pid}/stat") as stat:
            fields = stat.read().rsplit(")", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    raw = subprocess.check_output(["ps", "-o", "time=", "-p", str(pid)], text=True).strip()
    minutes, seconds = raw.split(":")[-2:]
    return int(minutes) * 60 + float(seconds) + (int(raw.split(":")[0]) * 3600 if raw.count(":") == 2 else 0)


def rss_mib(pid):
    return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()) / 1024


def read_exact(sock, size):
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise ConnectionError("agent closed the connection")
        data += chunk
    return data


class Viewer(threading.Thread):
    def __init__(self, path):
        super().__init__(daemon=True)
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(10)
        self.sock.connect(path)
        self.sock.sendall(HELLO)
        length = struct.unpack("<I", read_exact(self.sock, 4))[0]
        read_exact(self.sock, length)
        self.arrivals = []
        self.sizes = []
        self.stopped = False
        self.error = None

    def run(self):
        try:
            while not self.stopped:
                length = struct.unpack("<I", read_exact(self.sock, 4))[0]
                read_exact(self.sock, length)
                self.arrivals.append(time.monotonic())
                self.sizes.append(4 + length)
        except OSError as error:
            if not self.stopped:
                self.error = error

    def stop(self):
        self.stopped = True
        self.sock.close()


def start_agent(binary, port, interval_ms, home, with_sessions):
    if with_sessions:
        os.symlink(os.path.expanduser("~/.claude"), os.path.join(home, ".claude"))
    path = os.path.join(home, ".local/state/magi/agent.sock")
    env = dict(os.environ, HOME=home)
    agent = subprocess.Popen([binary, "agent", "--port", str(port), "--interval", str(interval_ms)], stderr=subprocess.DEVNULL, env=env)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            probe.connect(path)
            probe.close()
            return agent, path
        except OSError:
            time.sleep(0.1)
    agent.kill()
    raise RuntimeError("agent did not start listening")


def measure(binary, port, interval_ms, viewers, seconds, with_sessions=False):
    home = tempfile.mkdtemp(prefix="magi-bench-")
    agent, path = start_agent(binary, port, interval_ms, home, with_sessions)
    try:
        time.sleep(1)
        attached = [Viewer(path) for _ in range(viewers)]
        for viewer in attached:
            viewer.start()
        time.sleep(1)
        cpu_before, wall_before = cpu_seconds(agent.pid), time.monotonic()
        time.sleep(seconds)
        cpu = (cpu_seconds(agent.pid) - cpu_before) / (time.monotonic() - wall_before) * 100
        rss = rss_mib(agent.pid)
        for viewer in attached:
            viewer.stop()
        gaps = [(b - a) * 1000 for viewer in attached for a, b in zip(viewer.arrivals, viewer.arrivals[1:])]
        sizes = [size for viewer in attached for size in viewer.sizes]
        errors = [viewer.error for viewer in attached if viewer.error]
        frames = [len(viewer.arrivals) for viewer in attached]
        return {
            "cpu %": round(cpu, 3),
            "rss MiB": round(rss, 1),
            "frame B": round(statistics.mean(sizes)) if sizes else 0,
            "frames/viewer": f"{min(frames)}..{max(frames)}" if frames else "-",
            "gap p50 ms": round(statistics.median(gaps), 1) if gaps else "-",
            "gap p99 ms": round(sorted(gaps)[int(len(gaps) * 0.99) - 1], 1) if len(gaps) > 1 else "-",
            "gap max ms": round(max(gaps), 1) if gaps else "-",
            "errors": len(errors),
        }
    finally:
        agent.kill()
        agent.wait()
        shutil.rmtree(home, ignore_errors=True)


class Stress:
    def __init__(self, sleepers):
        self.processes = []
        self.sleepers = sleepers

    def __enter__(self):
        for _ in range(os.cpu_count()):
            self.processes.append(subprocess.Popen(["yes"], stdout=subprocess.DEVNULL))
        for _ in range(self.sleepers):
            self.processes.append(subprocess.Popen(["sleep", "600"]))
        time.sleep(2)
        return self

    def __exit__(self, *_):
        for process in self.processes:
            process.kill()
        for process in self.processes:
            process.wait()


def process_budget(requested):
    limit = resource.getrlimit(resource.RLIMIT_NPROC)[0]
    if limit == resource.RLIM_INFINITY:
        limit = 10**6
    running = len(subprocess.check_output(["ps", "-U", str(os.getuid()), "-o", "pid="], text=True).split())
    return max(0, min(requested, int(limit * 0.6) - running))


def report(results):
    keys = list(next(iter(results.values())).keys())
    width = max(len(name) for name in results)
    print(f"{'scenario':<{width}}  " + "  ".join(f"{key:>13}" for key in keys))
    for name, row in results.items():
        print(f"{name:<{width}}  " + "  ".join(f"{str(row[key]):>13}" for key in keys))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--port", type=int, default=27015)
    parser.add_argument("--seconds", type=int, default=20)
    parser.add_argument("--stress", action="store_true")
    parser.add_argument("--sleepers", type=int, default=1000)
    parser.add_argument("--sessions", action="store_true", help="let the agent read your real ~/.claude sessions (read-only)")
    args = parser.parse_args()

    print(f"{platform.system()} {platform.machine()} · {os.cpu_count()} cores · {args.seconds}s per scenario")
    results = {
        "idle, no viewers": measure(args.binary, args.port, 1000, 0, min(args.seconds, 10)),
        "1 viewer @ 1 s": measure(args.binary, args.port, 1000, 1, args.seconds, args.sessions),
        "32 viewers @ 100 ms": measure(args.binary, args.port, 100, 32, args.seconds, args.sessions),
    }
    if args.stress:
        sleepers = process_budget(args.sleepers)
        label = f"{os.cpu_count()} hogs + {sleepers} procs"
        with Stress(sleepers):
            results[f"1 viewer @ 1 s, {label}"] = measure(args.binary, args.port, 1000, 1, args.seconds)
            results[f"32 viewers @ 100 ms, {label}"] = measure(args.binary, args.port, 100, 32, args.seconds)
    report(results)


if __name__ == "__main__":
    sys.exit(main())
