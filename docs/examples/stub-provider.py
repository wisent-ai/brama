
import json, time
from http.server import BaseHTTPRequestHandler, HTTPServer

class Stub(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _send(self, code, obj, ctype="application/json"):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        if self.path.startswith("/v1/models"):
            self._send(200, {"data":[{"id":"stub-ok"},{"id":"stub-401"},{"id":"stub-429"}]})
        else:
            self._send(404, {"error":{"message":"not found"}})
    def do_POST(self):
        length = int(self.headers.get("Content-Length","0"))
        req = json.loads(self.rfile.read(length) or b"{}")
        model = req.get("model","")
        if model == "stub-401":
            self._send(401, {"error":{"message":"Incorrect API key provided: sk-brama-docs-invalid.","type":"invalid_request_error","code":"invalid_api_key"}})
            return
        if model == "stub-429":
            self.send_response(429)
            self.send_header("Content-Type","application/json")
            self.send_header("Retry-After","1")
            body = json.dumps({"error":{"message":"Rate limit reached for stub-429.","type":"tokens","code":"rate_limit_exceeded"}}).encode()
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body); return
        completion = {
            "id":"chatcmpl-stub-0001","object":"chat.completion","created":int(time.time()),
            "model":model,
            "choices":[{"index":0,"message":{"role":"assistant","content":"Hello from the stub provider."},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":9,"completion_tokens":7,"total_tokens":16}}
        if req.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type","text/event-stream")
            self.end_headers()
            for piece in ["Hello ","from ","the stub."]:
                chunk = {"id":"chatcmpl-stub-0001","object":"chat.completion.chunk","created":completion["created"],"model":model,
                         "choices":[{"index":0,"delta":{"content":piece},"finish_reason":None}]}
                self.wfile.write(b"data: "+json.dumps(chunk).encode()+b"\n\n")
            done = {"id":"chatcmpl-stub-0001","object":"chat.completion.chunk","created":completion["created"],"model":model,
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":completion["usage"]}
            self.wfile.write(b"data: "+json.dumps(done).encode()+b"\n\n")
            self.wfile.write(b"data: [DONE]\n\n")
            return
        self._send(200, completion)

HTTPServer(("127.0.0.1", 18999), Stub).serve_forever()
