#!/usr/bin/env python3
"""Real LAN + release Rust CLI + mTLS smoke test using simulated USB byte echo.

The root runner must prepare/upload the fixture, initialize a separate mock
server, export its public CA and start it on port 3242 before running this tool.
No Windows USB driver attach, real USB request or physical-device test is done.

Example (the command file contains an SSH argv JSON array, with strict host-key
checking, that prints ONLY the mock server's current pair-code):
  python live-network-smoke.py --client C:/.../bridge-client.exe \
    --ca C:/.../mock-ca.pem --work-dir C:/.../smoke-run \
    --pair-code-command-file C:/.../private-pair-command.json

The pairing provider's stdout is captured in memory and never printed, logged or
included in the report. Each successful pairing rotates the code on the server.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import struct
import subprocess
import sys
import time
from typing import Any

BUSIDS = ("1-2", "1-3", "1-4")
HEADER_IMPORT = struct.pack("!HHI", 0x0111, 0x8003, 0)


class SmokeFailure(RuntimeError):
    pass


def local_path(value: str, *, must_exist: bool = False) -> Path:
    path = Path(value).expanduser().resolve()
    if os.name == "nt" and path.drive.casefold() != "c:":
        raise SmokeFailure("All local input/output paths must resolve to C:.")
    if must_exist and not path.is_file():
        raise SmokeFailure("A required input file does not exist.")
    return path


def recv_exact(connection: socket.socket, length: int) -> bytes:
    data = bytearray()
    while len(data) < length:
        chunk = connection.recv(length - len(data))
        if not chunk:
            raise SmokeFailure("A simulated data connection closed unexpectedly.")
        data.extend(chunk)
    return bytes(data)


def import_device(proxy_port: int, busid: str, *, expect_busy: bool = False) -> socket.socket | None:
    connection = socket.create_connection(("127.0.0.1", proxy_port), timeout=30)
    try:
        connection.settimeout(30)
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        connection.sendall(HEADER_IMPORT + busid.encode("ascii").ljust(32, b"\0"))
        version, operation, status = struct.unpack("!HHI", recv_exact(connection, 8))
        if (version, operation) != (0x0111, 0x0003):
            raise SmokeFailure("The server returned an unexpected USB/IP import header.")
        if expect_busy:
            if status == 0:
                raise SmokeFailure("Exclusive ownership failed: another profile imported the same device.")
            connection.close()
            return None
        if status:
            raise SmokeFailure("The simulated import was rejected by the server.")
        descriptor = recv_exact(connection, 312)
        returned_busid = descriptor[256:288].split(b"\0", 1)[0].decode("ascii")
        if returned_busid != busid:
            raise SmokeFailure("The imported descriptor belongs to the wrong device.")
        vendor, product = struct.unpack("!HH", descriptor[300:304])
        if vendor != 0xF000 or product != int(busid.split("-")[1]):
            raise SmokeFailure("The endpoint is not the deliberately synthetic USB fixture.")
        return connection
    except BaseException:
        connection.close()
        raise


def round_trip(connection: socket.socket, busid: str, phase: str, count: int) -> dict[str, Any]:
    # All possible byte values are exercised. Chunked send/receive avoids a
    # sender-only deadlock against small TCP buffers; devices run concurrently.
    seed = hashlib.sha256(f"simulated-echo:{busid}:{phase}".encode()).digest()
    pattern = seed + bytes(range(256))
    payload = (pattern * ((count + len(pattern) - 1) // len(pattern)))[:count]
    received = bytearray()
    started = time.monotonic()
    for offset in range(0, count, 32768):
        chunk = payload[offset:offset + 32768]
        connection.sendall(chunk)
        received.extend(recv_exact(connection, len(chunk)))
    expected = hashlib.sha256(payload).hexdigest()
    actual = hashlib.sha256(received).hexdigest()
    if expected != actual:
        raise SmokeFailure("Raw simulated byte echo SHA256 mismatch.")
    return {"busid": busid, "phase": phase, "bytes": count, "sent_sha256": expected,
            "received_sha256": actual, "elapsed_seconds": round(time.monotonic() - started, 3),
            "hardware": False}


def close_data(connection: socket.socket) -> None:
    try:
        connection.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass
    connection.close()


class Harness:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        self.client = local_path(args.client, must_exist=True)
        self.ca = local_path(args.ca, must_exist=True)
        self.work = local_path(args.work_dir)
        if self.work.exists() and any(self.work.iterdir()):
            raise SmokeFailure("Work directory must be new or empty; existing credentials will not be overwritten.")
        self.work.mkdir(parents=True, exist_ok=True)
        self.sequence = 0
        self.secrets: list[str] = []
        self.proxies: list[tuple[subprocess.Popen, Any]] = []
        self.sockets: list[socket.socket] = []
        self.command: list[str] | None = None
        self.code_file: Path | None = None
        if args.pair_code_command_file:
            command_file = local_path(args.pair_code_command_file, must_exist=True)
            self.command = json.loads(command_file.read_text(encoding="utf-8-sig"))
            if not isinstance(self.command, list) or not self.command or not all(isinstance(s, str) for s in self.command):
                raise SmokeFailure("Pair-code command file must contain a nonempty JSON argv array.")
            # The command runs without a shell. The runner supplies the strict
            # SSH command; this tool never adds sudo, installs or starts services.
        else:
            self.code_file = local_path(args.pair_code_file, must_exist=True)
        self.report: dict[str, Any] = {
            "test_kind": "simulated USB byte echo over real LAN and authenticated TLS",
            "physical_usb_acceptance": False,
            "windows_driver_attach_performed": False,
            "two_physical_windows_computers_tested": False,
            "server": f"{args.host}:{args.port}", "certificate_server_name": args.server_name,
            "client_executable_sha256": hashlib.sha256(self.client.read_bytes()).hexdigest(),
            "public_ca_sha256": hashlib.sha256(self.ca.read_bytes()).hexdigest(),
            "profiles": [], "checks": [], "transfers": [], "ok": False,
        }

    def redact(self, text: str) -> str:
        for secret in self.secrets:
            text = text.replace(secret, "[REDACTED]")
        return text

    def pair_code(self) -> str:
        if self.command:
            try:
                result = subprocess.run(self.command, capture_output=True, timeout=40, check=False)
            except (OSError, subprocess.TimeoutExpired) as error:
                raise SmokeFailure("Pair-code provider could not complete; its arguments/output are private.") from error
            if result.returncode:
                raise SmokeFailure(f"Pair-code provider failed with exit code {result.returncode}; output is private.")
            value = result.stdout.decode("utf-8-sig").strip()
        else:
            assert self.code_file is not None
            value = self.code_file.read_text(encoding="utf-8-sig").strip()
        if not re.fullmatch(r"[0-9a-fA-F-]{32,64}", value):
            raise SmokeFailure("Pair-code provider did not return one UUID-format code; output is private.")
        self.secrets.append(value)
        return value

    def invoke(self, config: Path, phase: str, *arguments: str) -> dict[str, Any]:
        self.sequence += 1
        output = self.work / f"control-{self.sequence:03d}-{phase}.json"
        command = [str(self.client), "--direct", "--config", str(config), "--host", self.args.host,
                   "--port", str(self.args.port), "--server-name", self.args.server_name,
                   "--output", str(output), *arguments]
        try:
            # Management may wait for durable Linux configuration writes.
            # The client enforces its own 90s deadline; give it time to emit
            # structured errors and commit the returned pairing locally.
            result = subprocess.run(command, capture_output=True, timeout=110, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise SmokeFailure(f"Release client CLI did not complete during {phase}; command arguments are private.") from error
        # Stage tracing contains only operation names and timings. Still redact
        # every issued pairing code before preserving child stderr for diagnosis.
        diagnostic = self.redact(result.stderr.decode("utf-8", errors="replace"))
        if diagnostic:
            (self.work / f"control-{self.sequence:03d}-{phase}.stderr.log").write_text(diagnostic, encoding="utf-8")
        if not output.exists():
            raise SmokeFailure(f"Release client did not produce structured output during {phase}.")
        value = json.loads(output.read_text(encoding="utf-8-sig"))
        if result.returncode or not value.get("ok"):
            reason = self.redact(str(value.get("error", "unknown CLI failure")))
            raise SmokeFailure(f"Release client failed during {phase}: {reason}")
        return value

    def start_proxy(self, config: Path, label: str, allowed: tuple[str, ...] = BUSIDS) -> int:
        output = self.work / f"proxy-{label}.json"
        log = (self.work / f"proxy-{label}.log").open("wb")
        command = [str(self.client), "--direct", "--config", str(config), "--proxy",
                   "--proxy-port", "0", "--output", str(output)]
        for busid in allowed:
            command += ["--allow-busid", busid]
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0
        try:
            process = subprocess.Popen(command, stdout=log, stderr=log, creationflags=flags)
        except OSError as error:
            log.close()
            raise SmokeFailure("Could not launch the release client's explicit test proxy.") from error
        self.proxies.append((process, log))
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if output.exists():
                try:
                    value = json.loads(output.read_text(encoding="utf-8-sig"))
                except (OSError, json.JSONDecodeError):
                    time.sleep(0.1)
                    continue
                if not value.get("ok"):
                    raise SmokeFailure("The explicit test proxy returned a startup error.")
                host, port = value["listen"].rsplit(":", 1)
                if host != "127.0.0.1":
                    raise SmokeFailure("The test proxy did not bind exclusively to loopback.")
                return int(port)
            if process.poll() is not None:
                raise SmokeFailure("The explicit test proxy exited before publishing its port.")
            time.sleep(0.1)
        raise SmokeFailure("The explicit test proxy did not start within the deadline.")

    def devices(self, config: Path) -> dict[str, Any]:
        response = self.invoke(config, "list", "--list")
        return {device["busid"]: device for device in response["devices"]}

    def wait_owners(self, config: Path, expected: dict[str, str | None]) -> dict[str, Any]:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            devices = self.devices(config)
            if all(devices.get(busid, {}).get("owner") == owner for busid, owner in expected.items()):
                return devices
            time.sleep(0.2)
        raise SmokeFailure("Simulated-device ownership did not reach the expected state.")

    def transfer_parallel(self, streams: dict[str, socket.socket], phase: str) -> None:
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(streams)) as pool:
            tasks = [pool.submit(round_trip, stream, busid, phase, self.args.payload_bytes)
                     for busid, stream in streams.items()]
            for task in tasks:
                self.report["transfers"].append(task.result())

    def run(self) -> None:
        count = self.args.profiles or (2 if self.command else 1)
        if count == 2 and not self.command:
            raise SmokeFailure("Two profiles require a fresh pair-code command provider because codes rotate.")
        configs: list[Path] = []
        names: list[str] = []
        for index in range(count):
            name = f"LAN smoke logical profile {'AB'[index]}"
            config = self.work / f"profile-{'ab'[index]}" / "client.json"
            config.parent.mkdir()
            response = self.invoke(config, "pair", "--pair", "--name", name,
                                   "--ca", str(self.ca), "--code", self.pair_code())
            configs.append(config)
            names.append(name)
            self.report["profiles"].append({"name": name, "client_id": response["client_id"],
                                            "server_id": response["server_id"], "physical_computer": False})
        if count == 2:
            if self.report["profiles"][0]["client_id"] == self.report["profiles"][1]["client_id"]:
                raise SmokeFailure("The two logical profiles received the same client identity.")
            self.report["checks"].append("Two independently paired certificates on one Windows machine")
        devices = self.devices(configs[0])
        for busid in BUSIDS:
            device = devices.get(busid, {})
            if (not device.get("present") or device.get("protected_reason") or device.get("owner")
                    or device.get("vendor") != 0xF000 or device.get("product") != int(busid.split("-")[1])
                    or device.get("serial") != f"SIMULATED-BYTE-ECHO-{busid.split('-')[1]}"
                    or str(device.get("path", "")).startswith("/sys/")
                    or "SIMULATED USB byte echo" not in str(device.get("name", ""))):
                raise SmokeFailure("Pre-share device inspection is not the isolated synthetic fixture; no sharing operation was sent.")
        for busid in BUSIDS:
            self.invoke(configs[0], "share", "--share", busid)
        devices = self.devices(configs[0])
        if any(not devices[busid].get("shared") for busid in BUSIDS):
            raise SmokeFailure("A validated synthetic fixture device could not be shared.")
        self.report["checks"].append("Release Windows CLI paired, listed and shared three isolated synthetic devices")
        proxy_a = self.start_proxy(configs[0], "a")
        proxy_b = self.start_proxy(configs[1], "b") if count == 2 else proxy_a
        streams: dict[str, socket.socket] = {}
        for busid, proxy in [("1-2", proxy_a), ("1-3", proxy_a), ("1-4", proxy_b)]:
            connection = import_device(proxy, busid)
            assert connection is not None
            streams[busid] = connection
            self.sockets.append(connection)
        owners = {"1-2": names[0], "1-3": names[0], "1-4": names[-1]}
        self.wait_owners(configs[0], owners)
        self.transfer_parallel(streams, "three-device-concurrent")
        self.report["checks"].append("Three independent synthetic streams forwarded concurrently with exact SHA256")
        if count == 2:
            import_device(proxy_b, "1-2", expect_busy=True)
            self.report["checks"].append("A second certificate cannot import a device already owned by another session")
        close_data(streams.pop("1-2"))
        owners["1-2"] = None
        self.wait_owners(configs[0], owners)
        self.transfer_parallel(streams, "after-first-device-disconnect")
        self.report["checks"].append("Closing one synthetic device released its lease; other two streams still forwarded")
        # Permits deliberately authorize one session each. A fresh test proxy
        # creates a fresh permit/session; no old requests are replayed.
        fresh_proxy = self.start_proxy(configs[0], "fresh", ("1-2",))
        fresh = import_device(fresh_proxy, "1-2")
        assert fresh is not None
        self.sockets.append(fresh)
        self.report["transfers"].append(round_trip(fresh, "1-2", "fresh-session", self.args.payload_bytes))
        close_data(fresh)
        for connection in streams.values():
            close_data(connection)
        self.wait_owners(configs[0], {busid: None for busid in BUSIDS})
        self.report["checks"].append("Fresh import worked after release; every synthetic lease cleared after socket close")
        for busid in BUSIDS:
            self.invoke(configs[0], "unshare", "--unshare", busid)
        final = self.devices(configs[0])
        if any(final[busid]["shared"] or final[busid]["owner"] for busid in BUSIDS):
            raise SmokeFailure("Explicit mock unshare did not clear selections and ownership.")
        self.report["checks"].append("Explicit unshare restored the original mock driver and cleared all selections")
        self.report["ok"] = True

    def cleanup(self) -> None:
        for connection in self.sockets:
            close_data(connection)
        for process, log in reversed(self.proxies):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            log.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--client", required=True, help="C: path to the built release Windows bridge-client.exe")
    parser.add_argument("--ca", required=True, help="Exported public CA of the separate Linux mock server")
    parser.add_argument("--work-dir", required=True, help="New or empty C: test directory; contains private test credentials")
    provider = parser.add_mutually_exclusive_group(required=True)
    provider.add_argument("--pair-code-command-file", help="Private JSON argv file for an existing strict SSH pair-code command")
    provider.add_argument("--pair-code-file", help="Private file containing the current code; one logical profile only")
    parser.add_argument("--host", default="192.0.2.10")
    parser.add_argument("--port", type=int, default=3242)
    parser.add_argument("--server-name", default="usb-bridge")
    parser.add_argument("--profiles", type=int, choices=(1, 2))
    parser.add_argument("--payload-bytes", type=int, default=262144)
    args = parser.parse_args()
    if not 1 <= args.port <= 65535 or not 1 <= args.payload_bytes <= 8 * 1024 * 1024:
        parser.error("Port or payload size is outside the accepted range.")
    if os.name != "nt":
        parser.error("This harness must run the release Windows client on Windows.")
    harness: Harness | None = None
    try:
        harness = Harness(args)
        started = time.monotonic()
        try:
            harness.run()
        except Exception as error:
            # Only sanitized, deliberate failure descriptions enter the report;
            # Python subprocess exceptions can otherwise contain the pair code.
            harness.report["error"] = harness.redact(str(error)) if isinstance(error, SmokeFailure) else f"Unexpected harness error: {type(error).__name__}"
        finally:
            harness.cleanup()
        harness.report["elapsed_seconds"] = round(time.monotonic() - started, 3)
        report = harness.work / "network-smoke-report.json"
        report.write_text(json.dumps(harness.report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"ok": harness.report["ok"], "report": str(report),
                          "test_kind": harness.report["test_kind"], "physical_usb_acceptance": False}, ensure_ascii=False))
        return 0 if harness.report["ok"] else 1
    except Exception as error:
        message = str(error) if isinstance(error, SmokeFailure) else type(error).__name__
        print(json.dumps({"ok": False, "error": message, "physical_usb_acceptance": False}, ensure_ascii=False))
        return 1


if __name__ == "__main__":
    sys.exit(main())
