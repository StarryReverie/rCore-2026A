"""Exercise the installed OpenCode 2 loader, event stream, and session APIs locally."""

import base64
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "plugins/rcore-session-archive/scripts"))
sys.path.insert(0, str(ROOT / "plugins/rcore-session-archive/tests"))
from test_opencode_v2 import snapshot_v2
import opencode_hook


class OpenCodeRuntimeTests(unittest.TestCase):
    def setUp(self):
        binary = shutil.which("opencode")
        if not binary:
            self.skipTest("OpenCode 2 CLI is required for the real runtime test")
        version = subprocess.check_output([binary, "--version"], text=True, timeout=10)
        if not version.strip().startswith("opencode v2."):
            self.skipTest("This runtime regression test targets OpenCode 2")
        temporary = tempfile.TemporaryDirectory(prefix="course-opencode-runtime-")
        self.addCleanup(temporary.cleanup)
        self.temp = Path(temporary.name)
        self.root = self.temp / "project with spaces"
        self.root.mkdir()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        opencode_hook.install_hooks(self.root)
        self.policy = self.root / ".opencode/session-archive.json"
        self.policy.write_text('{"enabled":true,"mode":"full"}')
        # Isolate all client state and credentials; use no model service.
        env = {key: value for key, value in os.environ.items() if key in {"PATH", "LANG", "TMPDIR"}}
        env.update({name: str(self.temp / name.lower()) for name in (
            "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME")})
        env.update(OPENCODE_CONFIG_DIR=str(self.temp / "client-config"), OPENCODE_DISABLE_AUTOUPDATE="1",
                   OPENCODE_DISABLE_MODELS_FETCH="1", OPENCODE_PASSWORD="course-local-test")
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        self.url = f"http://127.0.0.1:{port}"
        self.log = self.temp / "server.log"
        output = self.log.open("w")
        self.addCleanup(output.close)
        self.server = subprocess.Popen([binary, "serve", "--hostname", "127.0.0.1", "--port", str(port)],
                                       cwd=self.root, env=env, stdout=output, stderr=output)
        self.addCleanup(self.stop_server)
        self.wait_for(lambda: self.request("GET", "/api/info"), "server startup")

    def stop_server(self):
        self.server.terminate()
        try:
            self.server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.server.kill()
            self.server.wait(timeout=5)

    def request(self, method, path, data=None):
        token = base64.b64encode(b"opencode:course-local-test").decode()
        request = urllib.request.Request(self.url + path, method=method,
            data=json.dumps(data).encode() if data is not None else None,
            headers={"Authorization": "Basic " + token, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=8) as response:
                body = response.read()
                return json.loads(body) if body else True
        except urllib.error.HTTPError as error:
            raise AssertionError(f"{method} {path}: {error.code}: {error.read().decode()}") from error

    def wait_for(self, check, description):
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            try:
                result = check()
                if result:
                    return result
            except (OSError, AssertionError):
                pass
            if self.server.poll() is not None:
                break
            time.sleep(0.1)
        self.fail(description + "\n" + self.log.read_text())

    def test_real_loader_and_execution_event_archive(self):
        query = urllib.parse.urlencode({"location[directory]": str(self.root)})
        def loaded():
            plugins = self.request("GET", "/api/plugin?" + query)["data"]
            return next((item for item in plugins if item.get("id") == "rcore-session-archive"), None)

        archive_plugin = self.wait_for(loaded, "archive plugin loading")
        self.assertEqual("active", archive_plugin["state"]["status"], json.dumps(archive_plugin))
        fixture = snapshot_v2(self.root)
        info = self.request("POST", "/api/session", {
            "location": {"directory": str(self.root)},
            "model": {"providerID": "course-test-missing", "id": "course-test-missing"}})["data"]
        info["id"] = "ses_course_import"
        self.request("POST", "/api/experimental/session/import", {
            "info": info, "messages": fixture["messages"], "location": {"directory": str(self.root)}})
        # An unavailable model emits a real terminal event without any network model call.
        # Imported transcript content still crosses the real native context API.
        self.request("POST", "/api/session/ses_course_import/prompt", {"text": "RUNTIME_QUESTION"})

        def archived():
            files = list((self.root / ".ai/agent-sessions/opencode").glob("*.jsonl"))
            if len(files) != 1:
                return None
            text = files[0].read_text()
            return text if "V2_ANSWER" in text and "RUNTIME_QUESTION" in text else None

        text = self.wait_for(archived, "real runtime archive")
        self.assertIn("V2_QUESTION", text)
        self.assertIn("V2_TOOL_OUTPUT", text)
        self.assertNotIn("PROVIDER_SECRET", text)
        self.assertNotIn("QklOQVJZ", text)
        self.assertNotIn("归档未完成", self.log.read_text())
