"""P2-3 端到端验证用的假机翻服务。

- 前 FAIL_FIRST 个请求返回 HTTP 429（触发客户端指数退避重试）；
- 之后的请求返回 200，内容 = 输入 JSON 数组逐条加前缀 "T:"；
- 记录「同时在途请求数」的峰值，打印到 stdout，用于证明客户端真的并发。
"""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

FAIL_FIRST = 3
PORT = int(sys.argv[1])

lock = threading.Lock()
seen = 0
inflight = 0
max_inflight = 0


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        global seen, inflight, max_inflight
        n = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(n)
        with lock:
            seen += 1
            my = seen
            inflight += 1
            max_inflight = max(max_inflight, inflight)
        try:
            time.sleep(0.2)  # 放大并发窗口
            if my <= FAIL_FIRST:
                body = json.dumps({"error": {"message": "rate limited"}}).encode()
                self.send_response(429)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            req = json.loads(raw)
            texts = json.loads(req["messages"][-1]["content"])
            out = ["T:" + t for t in texts]
            payload = {"choices": [{"message": {"content": json.dumps(out, ensure_ascii=False)}}]}
            body = json.dumps(payload, ensure_ascii=False).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        finally:
            with lock:
                inflight -= 1

    def do_GET(self):
        # 用于取统计：/stat -> {"seen":n,"max_inflight":m}
        body = json.dumps({"seen": seen, "max_inflight": max_inflight}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
