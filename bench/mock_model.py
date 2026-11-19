#!/usr/bin/env python3
"""A loopback model that plays a fixed list of tool calls.

The n-th request is answered with the n-th call of the scenario, and every
request after the last call with a short final reply. Each request's path and
size is appended to the log file as one JSON line, and the latest body is
kept beside it, so the runner can count and check what the agent sent
without trusting the agent's own accounting.

Socium's scenarios use the OpenAI chat wire. The Anthropic Messages, OpenAI
Responses and Gemini wires answer with text only, which is all
bench/agents.py needs to see what other agents send for one short prompt.

usage: mock_model.py --calls FILE --log FILE --port-file FILE
"""
import argparse
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

FINAL_REPLY = "Done."
COMPLETION_TOKENS = 20


def tool_result_bytes(messages):
    return sum(
        len(m["content"].encode()) if isinstance(m.get("content"), str) else len(json.dumps(m.get("content")).encode())
        for m in messages
        if m.get("role") == "tool"
    )


def sse(events):
    """Server-sent events from (event name or None, data) pairs."""
    return "".join(
        (f"event: {name}\n" if name else "") + f"data: {json.dumps(data)}\n\n" for name, data in events
    ).encode()


def chat(call, index, prompt_tokens, stream):
    if call:
        delta = {"tool_calls": [{
            "index": 0,
            "id": f"call_{index}",
            "type": "function",
            "function": {"name": call["tool"], "arguments": json.dumps(call["input"])},
        }]}
        finish = "tool_calls"
    else:
        delta, finish = {"content": FINAL_REPLY}, "stop"
    usage = {"prompt_tokens": prompt_tokens, "completion_tokens": COMPLETION_TOKENS,
             "total_tokens": prompt_tokens + COMPLETION_TOKENS}
    if not stream:
        message = {"role": "assistant", "content": delta.get("content"), **({"tool_calls": delta["tool_calls"]} if call else {})}
        if call:
            del message["tool_calls"][0]["index"]
        return "application/json", json.dumps({
            "id": "mock", "object": "chat.completion", "model": "mock-model",
            "choices": [{"index": 0, "message": message, "finish_reason": finish}], "usage": usage,
        }).encode()
    chunks = [
        {"id": "mock", "object": "chat.completion.chunk", "model": "mock-model",
         "choices": [{"index": 0, "delta": {"role": "assistant", **delta}, "finish_reason": None}]},
        {"id": "mock", "object": "chat.completion.chunk", "model": "mock-model",
         "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]},
        {"id": "mock", "object": "chat.completion.chunk", "model": "mock-model", "choices": [], "usage": usage},
    ]
    return "text/event-stream", sse((None, c) for c in chunks) + b"data: [DONE]\n\n"


def anthropic(prompt_tokens, stream):
    message = {"id": "msg_mock", "type": "message", "role": "assistant", "model": "mock-model",
               "content": [], "stop_reason": None, "stop_sequence": None,
               "usage": {"input_tokens": prompt_tokens, "output_tokens": 1}}
    if not stream:
        message.update(content=[{"type": "text", "text": FINAL_REPLY}], stop_reason="end_turn",
                       usage={"input_tokens": prompt_tokens, "output_tokens": COMPLETION_TOKENS})
        return "application/json", json.dumps(message).encode()
    return "text/event-stream", sse([
        ("message_start", {"type": "message_start", "message": message}),
        ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": FINAL_REPLY}}),
        ("content_block_stop", {"type": "content_block_stop", "index": 0}),
        ("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                           "usage": {"output_tokens": COMPLETION_TOKENS}}),
        ("message_stop", {"type": "message_stop"}),
    ])


def responses(prompt_tokens):
    item = {"type": "message", "id": "msg_mock", "status": "completed", "role": "assistant",
            "content": [{"type": "output_text", "text": FINAL_REPLY, "annotations": []}]}
    response = {"id": "resp_mock", "object": "response", "status": "in_progress", "model": "mock-model", "output": []}
    done = {**response, "status": "completed", "output": [item],
            "usage": {"input_tokens": prompt_tokens, "output_tokens": COMPLETION_TOKENS,
                      "total_tokens": prompt_tokens + COMPLETION_TOKENS,
                      "input_tokens_details": {"cached_tokens": 0}, "output_tokens_details": {"reasoning_tokens": 0}}}
    return "text/event-stream", sse([
        ("response.created", {"type": "response.created", "response": response}),
        ("response.output_item.added", {"type": "response.output_item.added", "output_index": 0,
                                        "item": {**item, "status": "in_progress", "content": []}}),
        ("response.output_text.delta", {"type": "response.output_text.delta", "item_id": "msg_mock",
                                        "output_index": 0, "content_index": 0, "delta": FINAL_REPLY}),
        ("response.output_item.done", {"type": "response.output_item.done", "output_index": 0, "item": item}),
        ("response.completed", {"type": "response.completed", "response": done}),
    ])


def gemini(prompt_tokens):
    return "text/event-stream", sse([(None, {
        "candidates": [{"content": {"role": "model", "parts": [{"text": FINAL_REPLY}]}, "finishReason": "STOP", "index": 0}],
        "usageMetadata": {"promptTokenCount": prompt_tokens, "candidatesTokenCount": COMPLETION_TOKENS,
                          "totalTokenCount": prompt_tokens + COMPLETION_TOKENS},
        "modelVersion": "mock-model",
    })])


def make_handler(calls, log):
    served = 0
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def reply(self, content_type, data, status=200):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            # Some agents list models before they start.
            self.reply("application/json", json.dumps({
                "object": "list", "data": [{"id": "mock-model", "object": "model", "owned_by": "mock"}],
                "models": [{"name": "models/mock-model"}],
            }).encode())

        def do_POST(self):
            nonlocal served
            raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            body = json.loads(raw) if raw else {}
            path = self.path.split("?")[0]
            with lock:
                index = served
                served += 1
                with log.open("a") as out:
                    out.write(json.dumps({
                        "path": path,
                        "bytes": len(raw),
                        "messages": len(body.get("messages", [])),
                        "tool_result_bytes": tool_result_bytes(body.get("messages", [])),
                    }) + "\n")
                (log.parent / "last-request.json").write_bytes(raw)
            prompt_tokens = len(raw) // 4
            stream = body.get("stream", True)
            if path.endswith("/chat/completions"):
                call = calls[index] if index < len(calls) else None
                self.reply(*chat(call, index, prompt_tokens, stream))
            elif path.endswith("/messages/count_tokens"):
                self.reply("application/json", json.dumps({"input_tokens": prompt_tokens}).encode())
            elif path.endswith("/messages"):
                self.reply(*anthropic(prompt_tokens, stream))
            elif path.endswith("/responses"):
                self.reply(*responses(prompt_tokens))
            elif ":streamGenerateContent" in path or ":generateContent" in path:
                self.reply(*gemini(prompt_tokens))
            elif ":countTokens" in path:
                self.reply("application/json", json.dumps({"totalTokens": prompt_tokens}).encode())
            else:
                self.reply("application/json", b'{"error": "unknown path"}', status=404)

        def log_message(self, *args):
            pass

    return Handler


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--calls", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--port-file", type=Path, required=True)
    args = parser.parse_args()
    calls = json.loads(args.calls.read_text())
    server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(calls, args.log))
    args.port_file.write_text(str(server.server_address[1]))
    server.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
