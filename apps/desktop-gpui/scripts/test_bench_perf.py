#!/usr/bin/env python3
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_perf


class LogParsing(unittest.TestCase):
    def test_reads_ansi_log_lines_and_finds_after(self):
        with tempfile.TemporaryDirectory() as root:
            sandbox = bench_perf.Sandbox(root, "s")
            sandbox.out.write_text(
                "\x1b[2m2026-11-07T17:22:09.000000Z\x1b[0m \x1b[32m INFO\x1b[0m a: auto-record start requested\n"
                "2026-11-07T17:22:10.250000Z  INFO b: Start gate admitted first video frame held_frames=0\n"
                "2026-11-07T17:22:14.000000Z  INFO c: auto-record stop requested\n"
                "2026-11-07T17:22:15.000000Z DEBUG d: editor frame number=0\n"
                "not a log line\n"
            )
            start = sandbox.find(r"auto-record start requested")
            gate = sandbox.find(r"Start gate admitted first video frame")
            self.assertEqual(bench_perf.ms(gate[0], start[0]), 1250.0)
            stop = sandbox.find(r"auto-record stop requested")
            frame = sandbox.find(r"editor frame number=0", stop[0])
            self.assertEqual(bench_perf.ms(frame[0], stop[0]), 1000.0)
            self.assertIsNone(sandbox.find(r"auto-record start requested", stop[0]))

    def test_seeded_store_skips_onboarding_and_captions(self):
        with tempfile.TemporaryDirectory() as root:
            store = bench_perf.json.loads(bench_perf.Sandbox(root, "s").root.joinpath("store").read_text())
            general = store["general_settings"]
            self.assertTrue(general["hasCompletedOnboarding"])
            self.assertTrue(general["hasCompletedStartup"])
            self.assertFalse(general["autoCaptionsAfterRecording"])
            self.assertEqual(general["recordingCountdown"], 0)


class Verdicts(unittest.TestCase):
    def test_directions(self):
        self.assertEqual(bench_perf.verdict("record.start_to_first_frame_ms", 150), "✅")
        self.assertEqual(bench_perf.verdict("record.start_to_first_frame_ms", 250), "❌")
        self.assertEqual(bench_perf.verdict("export.app_speed_x", 6), "✅")
        self.assertEqual(bench_perf.verdict("export.app_speed_x", 4), "❌")
        self.assertEqual(bench_perf.verdict("export.app_speed_x", None), "")

    def test_median_ignores_missing(self):
        self.assertEqual(bench_perf.median([None, 3, 1, 2]), 2)
        self.assertIsNone(bench_perf.median([None]))


if __name__ == "__main__":
    unittest.main()
