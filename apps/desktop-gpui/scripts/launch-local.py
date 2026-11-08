#!/usr/bin/env python3

import argparse
import os
from pathlib import Path
import subprocess


def launch_environment(environ, project=None):
    result = {
        key: value
        for key, value in environ.items()
        if not key.startswith("CAP_GPUI_")
    }
    if project is not None:
        result["CAP_GPUI_AUTO_EDITOR"] = str(project)
    return result


def main():
    parser = argparse.ArgumentParser(description="Launch installed Capwise with saved user settings")
    parser.add_argument("--project", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    app = Path("/Applications/Capwise.app")
    binary = app / "Contents/MacOS/Capwise"
    profile = Path.home() / "Library/Application Support/so.cap.desktop"
    if not binary.is_file():
        parser.error("Install /Applications/Capwise.app before launching")
    project = args.project.expanduser().resolve() if args.project else None
    if project is not None and (not project.is_dir() or project.suffix != ".cap"):
        parser.error("--project must be an existing .cap project directory")
    environment = launch_environment(os.environ, project)
    subprocess.run(["codesign", "--verify", "--strict", str(app)], check=True)
    print(f"Application: {app}")
    print(f"Settings: {profile / 'store'}")
    print("API keys: existing macOS login keychain")
    if args.check:
        return
    running = subprocess.run(["pgrep", "-x", "Capwise"], capture_output=True)
    if running.returncode == 0:
        parser.error("Capwise is already running. Save edits and quit it normally before relaunching")
    if running.returncode != 1:
        parser.error("Could not determine whether Capwise is running")
    logs = Path.home() / "Library/Logs/Capwise"
    logs.mkdir(parents=True, exist_ok=True)
    with (logs / "local-launch.log").open("ab") as log:
        process = subprocess.Popen(
            [str(binary)], env=environment, stdout=log, stderr=log,
            start_new_session=True,
        )
    print(f"Started Capwise (PID {process.pid})")


if __name__ == "__main__":
    main()
