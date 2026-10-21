"""Check the built desktop through its own loopback control channel, never OS input.

No Cloud server, credentials, browser login or user library is used. Screenshots
explicitly use the headless renderer; this is not native compositor acceptance.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time


def run(exe, output):
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="lightcraft-cloud-smoke-") as scratch:
        env = dict(os.environ, LIGHTCRAFT_CONFIG_DIR=str(Path(scratch) / "config"),
                   LIGHTCRAFT_GPU="0")
        # Bind an ephemeral port to find a free one; startup failures are bounded.
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        with (output / "desktop.log").open("w", encoding="utf-8") as log:
            proc = subprocess.Popen(
                [str(exe), "--library", str(Path(scratch) / "library"),
                 "--no-demo", "--control", str(port)],
                env=env, stdout=log, stderr=subprocess.STDOUT,
            )
            try:
                deadline = time.monotonic() + 120
                while True:
                    if proc.poll() is not None:
                        raise RuntimeError(f"Desktop exited during startup: {proc.returncode}")
                    try:
                        connection = socket.create_connection(("127.0.0.1", port), timeout=1)
                        break
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise RuntimeError("Desktop control channel did not start")
                        time.sleep(0.25)
                with connection:
                    connection.settimeout(90)
                    with connection.makefile("rwb") as wire:
                        def request(method, params=None):
                            wire.write((json.dumps({"id": 1, "method": method, "params": params or {}}) + "\n").encode())
                            wire.flush()
                            reply = json.loads(wire.readline())
                            if not reply.get("ok"):
                                raise RuntimeError(f"{method}: {reply}")
                            return reply.get("result")

                        def widgets(*expected, absent=()):
                            deadline = time.monotonic() + 20
                            while True:
                                ids = {w["id"] for w in request("ui.widgets")}
                                if set(expected) <= ids and not set(absent) & ids:
                                    return
                                if time.monotonic() >= deadline:
                                    raise RuntimeError(f"Missing widgets: {set(expected) - ids}; unexpected: {set(absent) & ids}")
                                time.sleep(0.1)

                        commands = {c["id"] for c in request("engine.commands")}
                        required = {"cloud.open", "cloud.settings", "cloud.login", "cloud.logout",
                                    "cloud.local", "cloud.upload", "cloud.download"}
                        if not required <= commands:
                            raise RuntimeError(f"Cloud commands missing: {required - commands}")
                        widgets("icon:cloud")
                        request("ui.clickWidget", {"id": "icon:cloud"})
                        widgets("button:cloud.settings", "button:cloud.local")
                        request("ui.screenshot", {"path": str(output / "cloud-library.png"), "headless": True})
                        request("ui.clickWidget", {"id": "button:cloud.settings"})
                        widgets("field:cloudServer", "button:cloud.check", "button:cloud.login")
                        request("ui.screenshot", {"path": str(output / "cloud-settings.png"), "headless": True})
                        request("ui.dialog.cancel")
                        widgets("button:cloud.local")
                        request("ui.clickWidget", {"id": "button:cloud.local"})
                        widgets("icon:cloud", absent=("button:cloud.local",))
                        request("ui.screenshot", {"path": str(output / "local-library.png"), "headless": True})
                        request("app.quit")
                proc.wait(timeout=30)
                if proc.returncode != 0:
                    raise RuntimeError(f"Desktop exited with {proc.returncode}")
                (output / "result.json").write_text(json.dumps({
                    "result": "passed", "executable": str(exe),
                    "checks": ["Cloud commands", "Cloud Library", "Settings -> Cloud",
                               "browser sign-in control", "return to local library", "clean shutdown"],
                    "screenshots": "Headless renderer in the actual desktop process",
                    "online_login": "not run; separate HTTPS Backend/Keycloak required",
                }, indent=2), encoding="utf-8")
                print("Desktop Cloud navigation smoke test passed")
            finally:
                if proc.poll() is None:
                    proc.terminate()
                    try:
                        proc.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                        proc.wait(timeout=10)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    run(args.exe.resolve(strict=True), args.output.resolve())
