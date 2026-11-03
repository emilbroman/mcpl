#!/usr/bin/env python3
"""Summarize recorded harness events without estimating tokens or answer correctness."""

import argparse
from collections import Counter
from html.parser import HTMLParser
import json
from pathlib import Path


class Links(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.texts = []
        self.current = None

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            if self.current is not None:
                raise ValueError("Nested anchor in fixture")
            self.current = []

    def handle_data(self, data):
        if self.current is not None:
            self.current.append(data)

    def handle_endtag(self, tag):
        if tag == "a" and self.current is not None:
            self.texts.append("".join(self.current).strip())
            self.current = None


def validate_fixture(directory=None):
    directory = Path(directory) if directory else Path(__file__).resolve().parent
    expected = json.loads((directory / "expected.json").read_text(encoding="utf-8"))
    parser = Links()
    parser.feed((directory / "links.html").read_text(encoding="utf-8"))
    parser.close()
    if parser.current is not None:
        raise ValueError("Unclosed anchor in fixture")
    normalize = (lambda text: text) if expected["caseSensitive"] else str.casefold
    needle = normalize(expected["needle"])
    matches = [text for text in parser.texts if needle in normalize(text)]
    actual = {"totalLinks": len(parser.texts), "matchingTexts": matches,
              "matchingCount": len(matches)}
    for key, value in actual.items():
        if value != expected[key]:
            raise ValueError(f"Fixture {key} mismatch: expected {expected[key]!r}, got {value!r}")
    return actual


def add_usage(total, usage):
    for key, value in usage.items():
        if isinstance(value, dict):
            add_usage(total.setdefault(key, {}), value)
        elif isinstance(value, (int, float)) and not isinstance(value, bool):
            total[key] = total.get(key, 0) + value


def summarize(path):
    path = Path(path)
    events = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()
              if line.strip()]
    calls = {event["toolCallId"]: event["name"] for event in events
             if event["type"] == "tool_call"}
    counts, raw_chars = Counter(), Counter()
    programs, expressions, errors = [], [], []
    descriptions, usage = {}, {}
    turns, maximum, final_text = 0, 0, ""
    for event in events:
        kind = event["type"]
        if kind == "session":
            if descriptions:
                raise ValueError(f"Multiple sessions in {path}")
            for tool in event["tools"]:
                text = tool.get("description", "")
                descriptions[tool["name"]] = {"chars": len(text),
                                               "utf8_bytes": len(text.encode("utf-8"))}
        elif kind == "message" and event["message"]["role"] == "assistant":
            message = event["message"]
            turns += 1
            add_usage(usage, message.get("usage", {}))
            final_text = "".join(block["text"] for block in message["content"]
                                 if block["type"] == "text")
        elif kind == "tool_call":
            name, arguments = event["name"], event["arguments"]
            counts[name] += 1
            if name == "run":
                programs.append(arguments["program"])
            elif name == "show":
                expressions.append(arguments["expression"])
        elif kind == "mcp_result":
            name = calls[event["toolCallId"]]
            result = event["result"]
            text = "".join(block["text"] for block in result.get("content", [])
                           if block["type"] == "text")
            raw_chars[name] += len(text)
            maximum = max(maximum, len(text))
            if result.get("isError", False):
                errors.append({"tool": name, "tool_call_id": event["toolCallId"], "text": text})
    return {
        "path": str(path), "assistant_turns": turns, "tool_calls": dict(counts),
        "error_count": len(errors), "errors": errors, "run_programs": programs,
        "show_expressions": expressions, "final_assistant_text": final_text,
        "reported_usage_sum": usage, "initial_tool_descriptions": descriptions,
        "initial_tool_description_totals": {
            key: sum(size[key] for size in descriptions.values())
            for key in ("chars", "utf8_bytes")
        },
        "raw_mcp_result_text_chars_by_tool": dict(raw_chars),
        "max_single_mcp_result_text_chars": maximum,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcripts", nargs="+", type=Path)
    args = parser.parse_args()
    validate_fixture()
    for path in args.transcripts:
        print(json.dumps(summarize(path), ensure_ascii=False))


if __name__ == "__main__":
    main()
