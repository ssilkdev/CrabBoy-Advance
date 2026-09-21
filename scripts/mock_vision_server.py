#!/usr/bin/env python3
"""
Mock OpenAI-compatible vision endpoint for CrabBoy Advance's AI Agent.

Speaks the same /v1/chat/completions contract as llama.cpp's server started
with --mmproj, so the emulator's agent can be exercised end-to-end without a
GPU. Saves every screenshot the agent sends so you can confirm the agent is
really seeing the game.

Usage:
    python3 scripts/mock_vision_server.py [--port 8099] [--outdir /tmp/ai_frames]
"""
import argparse
import base64
import json
import os
import re
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

ARGS = None
STATE = {"n": 0}

# A canned "playthrough": press START, then mash A through dialog, then walk.
SCRIPT = [
    {"observation": "Nintendo/Game Freak boot logo is on screen.",
     "goal": "Wait for the intro to finish.",
     "actions": [{"buttons": [], "frames": 60}]},
    {"observation": "Title screen with the legendary Pokemon is displayed.",
     "goal": "Start the game.",
     "actions": [{"buttons": ["START"], "frames": 5}, {"buttons": [], "frames": 40}]},
    {"observation": "Main menu / intro dialog from Professor Birch.",
     "goal": "Advance the dialog.",
     "actions": [{"buttons": ["A"], "frames": 4}, {"buttons": [], "frames": 20}]},
    {"observation": "Player character is standing in the truck.",
     "goal": "Walk out of the truck.",
     "actions": [{"buttons": ["DOWN"], "frames": 40}, {"buttons": [], "frames": 10}]},
]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass  # keep stdout clean

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.endswith("/models"):
            self._json(200, {"data": [{"id": "mock-vision", "object": "model"}]})
        else:
            self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length)
        try:
            payload = json.loads(raw)
        except Exception as e:
            self._json(400, {"error": {"message": f"bad json: {e}"}})
            return

        # Pull the image out of the last user message and persist it.
        image_b64 = None
        text_part = ""
        for msg in payload.get("messages", []):
            content = msg.get("content")
            if isinstance(content, list):
                for part in content:
                    if part.get("type") == "image_url":
                        url = part["image_url"]["url"]
                        m = re.match(r"data:image/png;base64,(.*)", url, re.S)
                        if m:
                            image_b64 = m.group(1)
                    elif part.get("type") == "text":
                        text_part = part.get("text", "")

        if image_b64 is None:
            self._json(400, {"error": {"message": "no image_url part in request"}})
            return

        n = STATE["n"]
        STATE["n"] += 1
        png = base64.b64decode(image_b64)
        os.makedirs(ARGS.outdir, exist_ok=True)
        path = os.path.join(ARGS.outdir, f"frame_{n:04d}.png")
        with open(path, "wb") as f:
            f.write(png)

        print(f"[req {n}] image {len(png)} bytes -> {path}", flush=True)
        if text_part:
            first = text_part.strip().splitlines()[0]
            print(f"         context: {first}", flush=True)

        plan = SCRIPT[n] if n < len(SCRIPT) else SCRIPT[-1]
        self._json(200, {
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": json.dumps(plan)},
                "finish_reason": "stop",
            }]
        })


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=8099)
    p.add_argument("--outdir", default="/tmp/crabboy_ai_frames")
    ARGS = p.parse_args()
    srv = HTTPServer(("127.0.0.1", ARGS.port), Handler)
    print(f"mock vision server on http://127.0.0.1:{ARGS.port}/v1/chat/completions", flush=True)
    print(f"saving received frames to {ARGS.outdir}", flush=True)
    srv.serve_forever()
