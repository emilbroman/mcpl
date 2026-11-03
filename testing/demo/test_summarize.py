import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from summarize import summarize, validate_fixture


class SummaryTests(unittest.TestCase):
    def test_transcript(self):
        def message(role, text, usage):
            return {"type": "message", "message": {
                "role": role, "content": [{"type": "text", "text": text}], "usage": usage,
            }}

        def result(kind, call_id, text, error=False):
            return {"type": kind, "toolCallId": call_id, "isError": error,
                    "result": {"isError": error, "content": [{"type": "text", "text": text}]}}

        events = [
            {"type": "session", "tools": [{"name": "run", "description": "Rún"}]},
            message("user", "prompt", {"input": 999}),
            message("assistant", "Working", {"input": 4, "output": 2, "totalTokens": 6,
                                            "cost": {"total": 0.25, "totalFromProvider": True}}),
            {"type": "tool_call", "toolCallId": "1", "name": "run",
             "arguments": {"program": "x = 1;"}},
            result("mcp_result", "1", "bad", True),
            result("tool_result", "1", "bad", True),
            {"type": "tool_call", "toolCallId": "2", "name": "show",
             "arguments": {"expression": "x"}},
            result("mcp_result", "2", "1"),
            result("tool_result", "2", "1"),
            {"type": "tool_call", "toolCallId": "3", "name": "help", "arguments": {}},
            result("mcp_result", "3", "Tool documentation"),
            message("toolResult", "ignored", {"input": 999}),
            message("assistant", "Final answer", {"input": 8, "output": 3, "totalTokens": 11,
                                                 "cost": {"total": 0.5}}),
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            path.write_text("\n".join(json.dumps(event) for event in events), encoding="utf-8")
            summary = summarize(path)
            output = subprocess.check_output(
                [sys.executable, str(Path(__file__).with_name("summarize.py")), str(path), str(path)],
                cwd=directory, text=True,
            )
            self.assertEqual([json.loads(line) for line in output.splitlines()], [summary, summary])
        self.assertEqual(summary["assistant_turns"], 2)
        self.assertEqual(summary["tool_calls"], {"run": 1, "show": 1, "help": 1})
        self.assertEqual(summary["error_count"], 1)
        self.assertEqual(summary["errors"], [{"tool": "run", "tool_call_id": "1", "text": "bad"}])
        self.assertEqual(summary["run_programs"], ["x = 1;"])
        self.assertEqual(summary["show_expressions"], ["x"])
        self.assertEqual(summary["final_assistant_text"], "Final answer")
        self.assertEqual(summary["reported_usage_sum"], {
            "input": 12, "output": 5, "totalTokens": 17, "cost": {"total": 0.75},
        })
        self.assertEqual(summary["initial_tool_descriptions"], {"run": {"chars": 3, "utf8_bytes": 4}})
        self.assertEqual(summary["initial_tool_description_totals"], {"chars": 3, "utf8_bytes": 4})
        self.assertEqual(summary["raw_mcp_result_text_chars_by_tool"], {
            "run": 3, "show": 1, "help": 18,
        })
        self.assertEqual(summary["max_single_mcp_result_text_chars"], 18)

    def test_fixture(self):
        self.assertEqual(validate_fixture()["matchingCount"], 7)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "links.html").write_text(
                '<a href="a">rUsT <b>&amp; systems</b></a><a href="b">Other</a>', encoding="utf-8",
            )
            expected = {"needle": "Rust", "caseSensitive": False, "totalLinks": 2,
                        "matchingTexts": ["rUsT & systems"], "matchingCount": 1}
            (path / "expected.json").write_text(json.dumps(expected), encoding="utf-8")
            self.assertEqual(validate_fixture(path)["matchingTexts"], ["rUsT & systems"])
            for field, invalid in (("totalLinks", 3), ("matchingCount", 0), ("matchingTexts", [])):
                with self.subTest(field=field):
                    (path / "expected.json").write_text(
                        json.dumps({**expected, field: invalid}), encoding="utf-8",
                    )
                    with self.assertRaisesRegex(ValueError, field):
                        validate_fixture(path)


if __name__ == "__main__":
    unittest.main()
