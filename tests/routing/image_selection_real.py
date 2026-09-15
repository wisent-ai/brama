"""Qualify real buffered and streaming image requests through Brama.

Run: python3 -S tests/routing/image_selection_real.py
Use the existing BRAMA_URL, BRAMA_TOKEN, WISENT_APP_AGENT_ID and
WISENT_APP_AGENT_AUTH_SECRET of a workload allowed to request best.
BRAMA_REAL_EXPECTED_REVISION must name the serving revision being qualified.
The two image requests spend existing subscription quota. No account, grant,
route or billing configuration is changed. Missing access fails, never skips.
"""

import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import uuid

ROOT = Path(__file__).resolve().parents[2]
IMAGE = ROOT / "tests/routing/fixtures/page.png"
REPORT = {"observations": {}}


class ImageSelection(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.origin = os.environ["BRAMA_URL"].rstrip("/")
        cls.token = os.environ["BRAMA_TOKEN"]
        cls.agent = os.environ["WISENT_APP_AGENT_ID"]
        cls.secret = os.environ["WISENT_APP_AGENT_AUTH_SECRET"]
        expected = os.environ["BRAMA_REAL_EXPECTED_REVISION"]
        image = IMAGE.read_bytes()
        REPORT["input_sha256"] = hashlib.sha256(image).hexdigest()
        cls.image_url = "data:image/png;base64," + base64.b64encode(image).decode()
        status, raw = cls.http("/health")
        identity = json.loads(raw)
        REPORT["gateway_identity"] = {"status": status, "body": identity}
        observed = identity.get("build", {}).get("source_revision")
        if status != 200 or observed != expected:
            raise AssertionError(f"expected serving revision {expected}, got HTTP {status}: {raw}")

    @classmethod
    def http(cls, path, payload=None):
        body = json.dumps(payload, separators=(",", ":")).encode() if payload is not None else b""
        timestamp = str(int(time.time()))
        body_hash = hashlib.sha256(body).hexdigest()
        proof = f"{cls.agent}:{timestamp}:{body_hash}".encode()
        signature = hmac.new(cls.secret.encode(), proof, hashlib.sha256).hexdigest()
        request = Request(cls.origin + path, data=body if payload is not None else None, headers={
            "Authorization": "Bearer " + cls.token,
            "Content-Type": "application/json",
            "X-Agent-Id": cls.agent,
            "X-Agent-Timestamp": timestamp,
            "X-Agent-Body-Sha256": body_hash,
            "X-Agent-Signature": signature,
        })
        try:
            response = urlopen(request)
        except HTTPError as error:
            response = error
        with response:
            return response.status, response.read().decode()

    def completion(self, stream=False, model="best"):
        status, raw = self.http("/v1/chat/completions", {
            "model": model,
            "stream": stream,
            "max_tokens": 128,
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "Read the large main heading in the attached image. Return only that heading."},
                {"type": "image_url", "image_url": {"url": self.image_url}},
            ]}],
        })
        REPORT["observations"][self._testMethodName] = {
            "model": model, "stream": stream, "status": status, "response": raw,
        }
        return status, raw

    def test_buffered_image_answer(self):
        status, raw = self.completion()
        self.assertEqual(status, 200, raw)
        payload = json.loads(raw)
        answer = payload["choices"][0]["message"]["content"]
        self.assertIn("measuring current figma components", " ".join(answer.lower().split()))

    def test_streamed_image_answer(self):
        status, raw = self.completion(stream=True)
        self.assertEqual(status, 200, raw)
        chunks = []
        finished = False
        for line in raw.splitlines():
            if not line.startswith("data:"):
                continue
            data = line.removeprefix("data:").strip()
            if data == "[DONE]":
                finished = True
                continue
            event = json.loads(data)
            for choice in event.get("choices", []):
                content = choice.get("delta", {}).get("content")
                if isinstance(content, str):
                    chunks.append(content)
        self.assertTrue(finished, "the gateway did not complete its event stream")
        self.assertIn("measuring current figma components", " ".join("".join(chunks).lower().split()))

    def test_unauthorized_route_stays_refused(self):
        status, raw = self.completion(model="unauthorized-image-selection-probe")
        self.assertEqual(status, 403, raw)
        self.assertEqual(json.loads(raw)["error"]["code"], "forbidden")


def main():
    output = ROOT / ".wisent-output/image-selection-real" / str(uuid.uuid4())
    output.mkdir(parents=True)
    revision = os.environ.get("WISENT_SOURCE_COMMIT") or subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout.strip()
    REPORT.update({"source_revision": revision, "command": sys.orig_argv})
    if not os.environ.get("WISENT_SOURCE_COMMIT"):
        patch = subprocess.run(["git", "diff", "--binary", "HEAD"], cwd=ROOT, check=True, capture_output=True)
        (output / "source.patch").write_bytes(patch.stdout)
        REPORT["source_patch"] = "source.patch"
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(ImageSelection))
    status = 0 if result.wasSuccessful() else 1
    REPORT.update({
        "exit_code": status, "tests_run": result.testsRun,
        "failures": [{"test": test.id(), "traceback": trace} for test, trace in result.failures + result.errors],
    })
    (output / "report.json").write_text(json.dumps(REPORT, indent=2) + "\n")
    print(f"Image selection evidence: {output / 'report.json'}")
    return status


if __name__ == "__main__":
    sys.exit(main())
