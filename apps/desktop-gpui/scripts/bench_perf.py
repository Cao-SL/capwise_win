#!/usr/bin/env python3
"""Capwise performance baseline: one command, one table, one JSON file.

Drives a built Capwise.app (default /Applications/Capwise.app, which carries the
Developer ID signature and therefore the Screen Recording grant) through the
existing CAP_GPUI_AUTO_* hooks inside a throwaway sandbox (its own app data,
recordings and log directories), and reads every timing from the app's own log
timestamps. Nothing touches the user's library or settings.

    apps/desktop-gpui/scripts/bench.sh                    # every stage
    apps/desktop-gpui/scripts/bench.sh --stages startup,export --fixture some.cap
    apps/desktop-gpui/scripts/bench.sh compare old.json new.json

Stages:
  startup   launch to main window / device list, idle memory and CPU
  record    record (screen + system audio) through the app; start latency,
            CPU and memory while recording, stop to the editor's first frame.
            The first take is long enough to serve as the fixture below.
  timeline  in-app CAP_GPUI_AUTO_TIMELINE_BENCH draw times (needs >= 30 s clip)
  playback  frame intervals while the editor plays (CAP_GPUI_AUTO_FRAME_STATS)
  scrub     headless request-to-frame latency of a paused drag
            (examples/scrub-benchmark; built on demand with --build-scrub)
  export    in-app MP4 export (CAP_GPUI_AUTO_EXPORT) and `cap-cli export`

The record stage captures whatever is on the primary display into the sandbox
under the scratch directory; it is deleted afterwards unless --keep is given.
"""
import argparse
import datetime as dt
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
APP_DIR = HERE.parent
DEFAULT_APP = Path("/Applications/Capwise.app")
ANSI = re.compile(r"\x1b\[[0-9;]*m")
LOG_LINE = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z\s+(\w+)\s+(.*)$")
FRAME_BUDGET_MS = 1000.0 / 60.0
BINARY_MTIME = {}

TARGETS = {
    "startup.launch_to_window_ms": ("<", 300.0, "冷启动 → 主窗口创建"),
    "startup.launch_to_devices_ms": ("<", 300.0, "冷启动 → 设备列表就绪（可以开始录制）"),
    "startup.idle_footprint_mb": ("<", 150.0, "空闲内存（phys footprint）"),
    "startup.idle_cpu_pct": ("<", 1.0, "空闲 CPU"),
    "record.start_to_first_frame_ms": ("<", 200.0, "按下录制 → 第一帧写入编码器"),
    "record.cpu_pct": ("<", 30.0, "录制中 CPU（单核=100%）"),
    "record.footprint_mb": ("<", 400.0, "录制中内存（phys footprint）"),
    "record.stop_to_editor_frame_ms": ("<", 500.0, "停止录制 → 编辑器第一帧"),
    "timeline.hover_p95_ms": ("<", FRAME_BUDGET_MS, "时间线悬停/拖动 绘制 p95"),
    "timeline.follow_p95_ms": ("<", FRAME_BUDGET_MS, "播放跟随 绘制 p95"),
    "timeline.tick_p95_ms": ("<", FRAME_BUDGET_MS, "播放刷新 绘制 p95"),
    "playback.frame_p50_ms": ("<", FRAME_BUDGET_MS + 1.0, "播放中帧间隔 p50"),
    "playback.frame_p99_ms": ("<", FRAME_BUDGET_MS * 2, "播放中帧间隔 p99"),
    "scrub.drag_p95_ms": ("<", FRAME_BUDGET_MS, "暂停拖动 请求→出帧 p95"),
    "export.app_speed_x": (">", 5.0, "App 内导出速度（相对实时）"),
    "export.cli_720p30_speed_x": (">", 5.0, "cap-cli 导出 720p30 速度（相对实时）"),
    "export.cli_1080p60_speed_x": (">", 5.0, "cap-cli 导出 1080p60 速度（相对实时）"),
}


def log(message):
    print(f"[bench] {message}", file=sys.stderr, flush=True)


def parse_time(stamp):
    return dt.datetime.fromisoformat(stamp).replace(tzinfo=dt.timezone.utc).timestamp()


class Sandbox:
    def __init__(self, root, name):
        self.root = Path(root) / name
        if self.root.exists():
            shutil.rmtree(self.root)
        (self.root / "rec").mkdir(parents=True)
        (self.root / "logs").mkdir()
        (self.root / "store").write_text(
            json.dumps(
                {
                    "general_settings": {
                        "enableNotifications": False,
                        "hasCompletedOnboarding": True,
                        "hasCompletedStartup": True,
                        "autoCaptionsAfterRecording": False,
                        "recordingCountdown": 0,
                        "enableTelemetry": False,
                    },
                    "recording_start_safety": {
                        "confirmBeforeRecordingWithoutMicrophone": False
                    },
                }
            )
        )
        self.out = self.root / "out.txt"
        self.proc = None
        self.spawned_at = None

    @property
    def recordings(self):
        return self.root / "rec"

    def launch(self, app, extra_env):
        env = dict(os.environ)
        env.update(
            {
                "CAP_GPUI_APP_DATA_DIR": str(self.root),
                "CAP_GPUI_RECORDINGS_DIR": str(self.recordings),
                "CAP_GPUI_LOG_DIR": str(self.root / "logs"),
                "CAP_GPUI_NO_TRAY": "1",
                "CAP_GPUI_NO_MENUS": "1",
                "CAP_GPUI_AUTO_NO_MIC": "1",
                "CAP_GPUI_MUTE_AUDIO": "1",
                "RUST_LOG": "info,cap_gpui=debug,cap_recording=debug,cap_export=info",
                "NO_COLOR": "1",
                "CAP_PERF_MARKS": "1",
            }
        )
        env.update(extra_env)
        binary = app / "Contents" / "MacOS" / "Capwise"
        if BINARY_MTIME.get(str(binary)) not in (None, binary.stat().st_mtime):
            raise RuntimeError(f"{binary} was replaced during the run (reinstalled?); results would mix builds")
        BINARY_MTIME.setdefault(str(binary), binary.stat().st_mtime)
        handle = open(self.out, "wb")
        self.spawned_at = time.time()
        self.proc = subprocess.Popen(
            [str(binary)], env=env, stdout=handle, stderr=subprocess.STDOUT
        )
        return self.proc

    def lines(self):
        try:
            text = self.out.read_text(errors="replace")
        except FileNotFoundError:
            return []
        parsed = []
        for raw in text.splitlines():
            line = ANSI.sub("", raw)
            match = LOG_LINE.match(line)
            if match:
                parsed.append((parse_time(match.group(1)), match.group(2), match.group(3)))
            else:
                parsed.append((None, "", line))
        return parsed

    def find(self, pattern, after=None):
        regex = re.compile(pattern)
        for stamp, _, message in self.lines():
            if stamp is None or (after is not None and stamp < after):
                continue
            if regex.search(message):
                return stamp, message
        return None

    def wait_for(self, pattern, timeout, after=None, sampler=None):
        deadline = time.time() + timeout
        while time.time() < deadline:
            found = self.find(pattern, after)
            if found:
                return found
            if self.proc and self.proc.poll() is not None:
                return self.find(pattern, after)
            if sampler:
                sampler()
            time.sleep(0.1 if sampler is None else 0.5)
        return None

    def stop(self):
        if not self.proc or self.proc.poll() is not None:
            return
        helpers = subprocess.run(
            ["pgrep", "-P", str(self.proc.pid)], capture_output=True, text=True
        ).stdout.split()
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        for helper in helpers:
            try:
                os.kill(int(helper), signal.SIGTERM)
            except (ProcessLookupError, ValueError):
                pass


def cputime_secs(pid):
    result = subprocess.run(
        ["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True
    )
    value = result.stdout.strip()
    if not value:
        return None
    parts = value.split(":")
    secs = 0.0
    for part in parts:
        secs = secs * 60 + float(part)
    return secs


def rss_mb(pid):
    result = subprocess.run(
        ["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True
    )
    value = result.stdout.strip()
    return int(value) / 1024.0 if value else None


def footprint_mb(pid):
    result = subprocess.run(
        ["footprint", "-p", str(pid), "-f", "bytes"], capture_output=True, text=True
    )
    match = re.search(r"phys_footprint:\s*(\d+)", result.stdout) or re.search(
        r"Footprint:\s*(\d+)\s*B", result.stdout
    )
    return int(match.group(1)) / 1048576.0 if match else None


def cpu_pct_over(pid, seconds):
    start_cpu, start = cputime_secs(pid), time.time()
    time.sleep(seconds)
    end_cpu, end = cputime_secs(pid), time.time()
    if start_cpu is None or end_cpu is None or end_cpu < start_cpu:
        return None
    return (end_cpu - start_cpu) / (end - start) * 100.0


def ms(later, earlier):
    if later is None or earlier is None:
        return None
    return round((later - earlier) * 1000.0, 1)


def stamp_of(found):
    return found[0] if found else None


def stage_startup(args, work, results):
    launch, devices, footprints, rss, cpu, phases = [], [], [], [], [], []
    for run in range(args.startup_runs):
        sandbox = Sandbox(work, f"startup-{run}")
        proc = sandbox.launch(args.app, {})
        window = sandbox.wait_for(r"cap_gpui: main window opened", 30)
        listed = sandbox.wait_for(r"enumerated capture devices", 30)
        launch.append(ms(stamp_of(window), sandbox.spawned_at))
        first_line = next((t for t, _, _ in sandbox.lines() if t is not None), None)
        marks = {"first_log_line": ms(first_line, sandbox.spawned_at)}
        for stamp, _, message in sandbox.lines():
            found = re.search(r"perf mark name=\"?(\w+)", message or "")
            if stamp and found:
                marks[found.group(1)] = ms(stamp, sandbox.spawned_at)
        phases.append(marks)
        devices.append(ms(stamp_of(listed), sandbox.spawned_at))
        time.sleep(args.idle_settle)
        cpu.append(cpu_pct_over(proc.pid, 5.0))
        alive = proc.poll() is None
        footprints.append(footprint_mb(proc.pid) if alive else None)
        rss.append(rss_mb(proc.pid) if alive else None)
        if not alive:
            log(f"startup run {run}: the app exited by itself (code {proc.returncode}); see {sandbox.out}")
        sandbox.stop()
        log(f"startup run {run}: window {launch[-1]} ms, devices {devices[-1]} ms, "
            f"footprint {footprints[-1]} MB, cpu {cpu[-1]}%")
    results["startup.launch_to_window_ms"] = median(launch)
    results["startup.launch_to_window_first_ms"] = launch[0]
    results["startup.launch_to_devices_ms"] = median(devices)
    results["startup.idle_footprint_mb"] = median(footprints)
    results["startup.idle_rss_mb"] = median(rss)
    results["startup.idle_cpu_pct"] = median(cpu)
    results["raw.startup"] = {
        "window_ms": launch, "devices_ms": devices,
        "footprint_mb": footprints, "rss_mb": rss, "cpu_pct": cpu,
        "phases_since_spawn_ms": phases,
    }


def record_once(args, work, name, seconds):
    sandbox = Sandbox(work, name)
    proc = sandbox.launch(
        args.app,
        {
            "CAP_GPUI_AUTO_RECORD": f"studio:{seconds}",
            "CAP_GPUI_AUTO_SYSTEM_AUDIO": "1",
        },
    )
    started = sandbox.wait_for(r"cap_gpui::session: recording started", 60)
    if not started:
        sandbox.stop()
        raise RuntimeError(f"{name}: the recording never started; see {sandbox.out}")
    samples = []
    warmup_until = started[0] + min(2.0, seconds / 3)

    def sample():
        if time.time() >= warmup_until:
            samples.append((time.time(), cputime_secs(proc.pid), rss_mb(proc.pid), footprint_mb(proc.pid)))

    stop = sandbox.wait_for(r"auto-record stop requested", seconds + 30, sampler=sample)
    frame = sandbox.wait_for(r"editor frame number=0", 60, after=stamp_of(stop))
    finished = sandbox.find(r"cap_gpui::session: recording finished", stamp_of(stop))
    time.sleep(1.0)
    sandbox.stop()

    requested = stamp_of(sandbox.find(r"auto-record start requested"))
    gate = stamp_of(sandbox.find(r"Start gate admitted first video frame"))
    pipeline = stamp_of(sandbox.find(r"studio_recording: pipeline playing"))
    armed = stamp_of(sandbox.find(r"Recording start gate armed"))
    window = stamp_of(sandbox.find(r"editor window opened", stamp_of(stop)))
    take = {
        "seconds": seconds,
        "start_to_pipeline_ms": ms(pipeline, requested),
        "start_to_gate_armed_ms": ms(armed, requested),
        "start_to_first_frame_ms": ms(gate, requested),
        "start_to_session_started_ms": ms(stamp_of(started), requested),
        "stop_to_editor_window_ms": ms(window, stamp_of(stop)),
        "stop_to_finished_ms": ms(stamp_of(finished), stamp_of(stop)),
        "stop_to_editor_frame_ms": ms(stamp_of(frame), stamp_of(stop)),
    }
    usable = [s for s in samples if s[1] is not None]
    if len(usable) >= 2:
        first, last = usable[0], usable[-1]
        take["cpu_pct"] = round((last[1] - first[1]) / (last[0] - first[0]) * 100.0, 1)
        take["rss_mb"] = round(max(s[2] for s in usable if s[2]), 1)
        footprints = [s[3] for s in usable if s[3]]
        take["footprint_mb"] = round(max(footprints), 1) if footprints else None
    projects = sorted(sandbox.recordings.glob("*.cap"))
    return take, (projects[0] if projects else None), sandbox


def stage_record(args, work, results, state):
    takes = []
    durations = [args.fixture_secs] + [args.short_secs] * (args.record_runs - 1)
    for index, seconds in enumerate(durations):
        take, project, _ = record_once(args, work, f"record-{index}", seconds)
        log(f"record take {index} ({seconds}s): {take}")
        takes.append(take)
        if index == 0 and project and not state.get("fixture"):
            state["fixture"] = project
    pick = lambda key: median([t.get(key) for t in takes])
    for key in (
        "start_to_pipeline_ms", "start_to_gate_armed_ms", "start_to_first_frame_ms",
        "stop_to_editor_window_ms", "stop_to_finished_ms", "stop_to_editor_frame_ms",
    ):
        results[f"record.{key}"] = pick(key)
    long_take = takes[0]
    results["record.cpu_pct"] = long_take.get("cpu_pct")
    results["record.rss_mb"] = long_take.get("rss_mb")
    results["record.footprint_mb"] = long_take.get("footprint_mb")
    results["raw.record"] = takes


def copy_fixture(state, work, name):
    sandbox = Sandbox(work, name)
    target = sandbox.recordings / "fixture.cap"
    shutil.copytree(state["fixture"], target)
    return sandbox, target


def stage_timeline(args, work, results, state):
    sandbox, project = copy_fixture(state, work, "timeline")
    proc = sandbox.launch(
        args.app,
        {"CAP_GPUI_AUTO_EDITOR": str(project), "CAP_GPUI_AUTO_TIMELINE_BENCH": "1"},
    )
    try:
        proc.wait(timeout=240)
    except subprocess.TimeoutExpired:
        sandbox.stop()
    report = re.compile(
        r"timeline-bench: (\S+) ([\d.]+) mean / ([\d.]+) p50 / ([\d.]+) p95"
        r"(?: / ([\d.]+) p99)? / ([\d.]+) max us"
    )
    scenarios = {}
    for _, _, message in sandbox.lines():
        match = report.search(message)
        if match:
            label = match.group(1)
            scenarios[label] = {
                "mean_ms": float(match.group(2)) / 1000,
                "p50_ms": float(match.group(3)) / 1000,
                "p95_ms": float(match.group(4)) / 1000,
                "p99_ms": float(match.group(5)) / 1000 if match.group(5) else None,
                "max_ms": float(match.group(6)) / 1000,
            }
    if not scenarios:
        log(f"timeline: no report lines (exit {proc.returncode}); see {sandbox.out}")
    for label in ("hover", "follow", "tick"):
        if label in scenarios:
            results[f"timeline.{label}_p95_ms"] = round(scenarios[label]["p95_ms"], 2)
    results["raw.timeline"] = scenarios


def stage_playback(args, work, results, state):
    sandbox, project = copy_fixture(state, work, "playback")
    sandbox.launch(
        args.app,
        {
            "CAP_GPUI_AUTO_EDITOR": str(project),
            "CAP_GPUI_AUTO_PLAYBACK": "8",
            "CAP_GPUI_AUTO_FRAME_STATS": "6",
        },
    )
    stats = sandbox.wait_for(r"frame stats frames=", 60)
    sandbox.wait_for(r"(?i)playback.*(fps|frames)", 15, after=stamp_of(stats))
    time.sleep(1.0)
    sandbox.stop()
    if not stats:
        log(f"playback: no frame stats; see {sandbox.out}")
        return
    fields = dict(re.findall(r"(\w+)=\"?([\d.]+)", stats[1]))
    results["playback.frame_p50_ms"] = float(fields.get("p50_ms", "nan"))
    results["playback.frame_p99_ms"] = float(fields.get("p99_ms", "nan"))
    results["playback.frame_max_ms"] = float(fields.get("max_ms", "nan"))
    reports = [m for _, _, m in sandbox.lines() if re.search(r"(?i)playback (report|stats)|dropped", m)]
    results["raw.playback"] = {"frame_stats": stats[1], "engine_report": reports[-3:]}


def stage_scrub(args, work, results, state):
    binary = Path(args.scrub_bin) if args.scrub_bin else APP_DIR / "target" / "debug" / "examples" / "scrub-benchmark"
    if not binary.exists() and args.build_scrub:
        log("building examples/scrub-benchmark (dev profile; dependencies are opt-level 2)")
        subprocess.run(["cargo", "build", "--example", "scrub-benchmark"], cwd=APP_DIR, check=True)
    if not binary.exists():
        log(f"scrub: {binary} missing; pass --build-scrub to build it")
        results["raw.scrub"] = {"skipped": "binary not built"}
        return
    sandbox, project = copy_fixture(state, work, "scrub")
    env = dict(os.environ)
    env["DYLD_FALLBACK_LIBRARY_PATH"] = str(args.app / "Contents" / "Frameworks" / "Spacedrive.framework" / "Libraries")
    result = subprocess.run(
        [str(binary), str(project), "30", "2"], capture_output=True, text=True, timeout=600, env=env
    )
    line = next((l for l in result.stdout.splitlines() if l.startswith("{")), None)
    if not line:
        log(f"scrub failed: {result.stderr[-2000:]}")
        return
    report = json.loads(line)
    results["scrub.drag_p50_ms"] = round(report["dragMedianMs"], 2)
    results["scrub.drag_p95_ms"] = round(report["dragP95Ms"], 2)
    results["scrub.seek_ms_max"] = round(max(r["seekMs"] for r in report["regions"]), 2)
    results["raw.scrub"] = report


def probe(path):
    result = subprocess.run(
        [
            "ffprobe", "-v", "error", "-select_streams", "v:0",
            "-show_entries", "stream=width,height,avg_frame_rate,codec_name,nb_frames:format=duration",
            "-of", "json", str(path),
        ],
        capture_output=True, text=True,
    )
    data = json.loads(result.stdout or "{}")
    stream = (data.get("streams") or [{}])[0]
    num, _, den = stream.get("avg_frame_rate", "0/1").partition("/")
    return {
        "duration": float(data.get("format", {}).get("duration", 0) or 0),
        "width": stream.get("width"),
        "height": stream.get("height"),
        "fps": round(float(num) / float(den or 1), 2) if num else None,
        "codec": stream.get("codec_name"),
    }


def stage_export(args, work, results, state):
    sandbox, project = copy_fixture(state, work, "export")
    output = sandbox.root / "export.mp4"
    sandbox.launch(
        args.app, {"CAP_GPUI_AUTO_EDITOR": str(project), "CAP_GPUI_AUTO_EXPORT": str(output)}
    )
    requested = sandbox.wait_for(r"auto editor export requested", 90)
    done = sandbox.wait_for(r"editor export completed|export failed|Export failed", args.export_timeout, after=stamp_of(requested))
    hardware = [m for _, _, m in sandbox.lines() if re.search(r"(?i)(videotoolbox|hardware).*(encod)|encoder.*(videotoolbox|hardware)", m)]
    sandbox.stop()
    if requested and done and output.exists():
        info = probe(output)
        secs = done[0] - requested[0]
        results["export.app_secs"] = round(secs, 2)
        results["export.app_speed_x"] = round(info["duration"] / secs, 2) if secs > 0 else None
        results["raw.export_app"] = {**info, "encoder_lines": hardware[-3:]}
    else:
        log(f"export: app export did not finish; see {sandbox.out}")

    cli = args.app / "Contents" / "MacOS" / "cap-cli"
    if not cli.exists():
        return
    for label, resolution, fps in (("720p30", "1280x720", "30"), ("1080p60", "1920x1080", "60")):
        cli_out = sandbox.root / f"export-cli-{label}.mp4"
        started = time.time()
        result = subprocess.run(
            [str(cli), "export", str(project), "-o", str(cli_out), "--json",
             "--resolution", resolution, "--fps", fps],
            capture_output=True, text=True, timeout=1800,
        )
        secs = time.time() - started
        if result.returncode == 0 and cli_out.exists():
            info = probe(cli_out)
            results[f"export.cli_{label}_secs"] = round(secs, 2)
            results[f"export.cli_{label}_speed_x"] = round(info["duration"] / secs, 2)
            results[f"raw.export_cli_{label}"] = info
        else:
            log(f"cap-cli export {label} failed: {result.stderr[-1500:]}")


def median(values):
    values = [v for v in values if v is not None]
    return round(statistics.median(values), 1) if values else None


def verdict(key, value):
    if key not in TARGETS or value is None:
        return ""
    op, target, _ = TARGETS[key]
    ok = value < target if op == "<" else value > target
    return "✅" if ok else "❌"


def print_table(results):
    rows = [("指标", "实测", "目标", "达标")]
    for key, (op, target, label) in TARGETS.items():
        value = results.get(key)
        shown = "—" if value is None else f"{value:g}"
        rows.append((f"{label} ({key})", shown, f"{op} {target:g}", verdict(key, value) or "未测"))
    extra = [k for k in results if k not in TARGETS and not k.startswith("raw.") and k != "meta"]
    widths = [max(len(str(row[i])) for row in rows) for i in range(4)]
    for row in rows:
        print("  ".join(str(cell).ljust(widths[i]) for i, cell in enumerate(row)))
    if extra:
        print("\n参考指标：")
        for key in extra:
            print(f"  {key} = {results[key]}")


def run(args):
    args.app = Path(args.app)
    stages = [s.strip() for s in args.stages.split(",") if s.strip()]
    work = Path(args.work or tempfile.mkdtemp(prefix="capwise-bench-"))
    work.mkdir(parents=True, exist_ok=True)
    log(f"sandboxes under {work}")
    plist = args.app / "Contents" / "Info.plist"
    version = subprocess.run(
        ["/usr/libexec/PlistBuddy", "-c", "Print CFBundleShortVersionString", str(plist)],
        capture_output=True, text=True,
    ).stdout.strip()
    binary = args.app / "Contents" / "MacOS" / "Capwise"
    results = {
        "meta": {
            "date": dt.datetime.now().isoformat(timespec="seconds"),
            "app": str(args.app),
            "app_version": version,
            "app_binary_mtime": dt.datetime.fromtimestamp(binary.stat().st_mtime).isoformat(timespec="seconds"),
            "machine": platform.machine(),
            "chip": subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip(),
            "macos": platform.mac_ver()[0],
            "git": subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=APP_DIR, capture_output=True, text=True).stdout.strip(),
            "stages": stages,
        }
    }
    state = {}
    if args.fixture:
        state["fixture"] = Path(args.fixture)
    handlers = {
        "startup": stage_startup,
        "record": stage_record,
        "timeline": stage_timeline,
        "playback": stage_playback,
        "scrub": stage_scrub,
        "export": stage_export,
    }
    try:
        for stage in stages:
            if stage not in handlers:
                raise SystemExit(f"unknown stage {stage}")
            if stage in ("timeline", "playback", "scrub", "export") and not state.get("fixture"):
                log(f"{stage}: no fixture (run the record stage or pass --fixture); skipped")
                continue
            log(f"stage {stage}")
            started = time.time()
            if stage == "startup":
                stage_startup(args, work, results)
            else:
                handlers[stage](args, work, results, state)
            log(f"stage {stage} took {time.time() - started:.0f}s")
        if state.get("fixture"):
            results["meta"]["fixture"] = probe(next(Path(state["fixture"]).rglob("display.mp4"), Path("/nonexistent")))
    finally:
        if not args.keep and not args.work:
            shutil.rmtree(work, ignore_errors=True)
    print_table(results)
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps(results, indent=2, ensure_ascii=False, default=str))
        log(f"wrote {args.out}")


def compare(args):
    old = json.loads(Path(args.old).read_text())
    new = json.loads(Path(args.new).read_text())
    print(f"{'指标':<40} {'旧':>10} {'新':>10} {'变化':>9}")
    for key in TARGETS:
        a, b = old.get(key), new.get(key)
        if a is None and b is None:
            continue
        change = f"{(b - a) / a * 100:+.1f}%" if a and b is not None else ""
        print(f"{key:<40} {str(a):>10} {str(b):>10} {change:>9} {verdict(key, b)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command")
    runner = sub.add_parser("run")
    for target in (parser, runner):
        target.add_argument("--app", default=str(DEFAULT_APP))
        target.add_argument("--stages", default="startup,record,timeline,playback,scrub,export")
        target.add_argument("--fixture", help="studio .cap to use instead of the first recorded take (copied, never modified)")
        target.add_argument("--out", help="write the results JSON here")
        target.add_argument("--work", help="sandbox root (kept); default is a temp dir removed afterwards")
        target.add_argument("--keep", action="store_true", help="keep the temp sandbox root")
        target.add_argument("--startup-runs", type=int, default=5)
        target.add_argument("--idle-settle", type=float, default=8.0)
        target.add_argument("--record-runs", type=int, default=3)
        target.add_argument("--fixture-secs", type=int, default=40)
        target.add_argument("--short-secs", type=int, default=6)
        target.add_argument("--export-timeout", type=float, default=300.0)
        target.add_argument("--scrub-bin")
        target.add_argument("--build-scrub", action="store_true")
    comparer = sub.add_parser("compare")
    comparer.add_argument("old")
    comparer.add_argument("new")
    args = parser.parse_args()
    if args.command == "compare":
        compare(args)
    else:
        run(args)


if __name__ == "__main__":
    main()
