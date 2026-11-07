use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use serde_json::Value;

fn cap() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cap"))
}

fn run(args: &[&str]) -> Output {
    cap()
        .args(args)
        .output()
        .expect("failed to spawn cap binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn parse_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout was not valid JSON: {e}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

#[tokio::test]
async fn organization_operations_validate_wait_timeouts_before_submitting() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

    for (arguments, method, path, body) in [
        (
            vec!["organizations", "delete", "org_test"],
            "DELETE",
            "/api/v1/organizations/org_test",
            serde_json::json!({}),
        ),
        (
            vec!["organizations", "domain", "remove", "org_test"],
            "DELETE",
            "/api/v1/organizations/org_test/domain",
            serde_json::json!({}),
        ),
        (
            vec!["organizations", "domain", "verify", "org_test"],
            "POST",
            "/api/v1/organizations/org_test/domain/verify",
            serde_json::json!({}),
        ),
        (
            vec![
                "organizations",
                "domain",
                "set",
                "org_test",
                "clips.example.com",
            ],
            "PUT",
            "/api/v1/organizations/org_test/domain",
            serde_json::json!({"domain": "clips.example.com"}),
        ),
    ] {
        for (wait, timeout, expected_requests) in [
            (true, "0", 0),
            (true, "86401", 0),
            (true, "86400", 2),
            (false, "0", 1),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (sender, receiver) = mpsc::channel();
            let server = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut reader = tokio::io::BufReader::new(&mut stream);
                    let mut request = String::new();
                    reader.read_line(&mut request).await.unwrap();
                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).await.unwrap();
                        if header == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = header.split_once(':')
                            && name.eq_ignore_ascii_case("content-length")
                        {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                    let mut bytes = vec![0; length];
                    reader.read_exact(&mut bytes).await.unwrap();
                    let state = if request.starts_with("GET ") {
                        "succeeded"
                    } else {
                        "queued"
                    };
                    sender.send((request, bytes)).unwrap();
                    let response = serde_json::json!({"id": "op_test", "state": state}).to_string();
                    stream
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                                response.len()
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                }
            });
            let mut args = arguments.clone();
            args.extend(["--yes", "--json", "--timeout", timeout]);
            if wait {
                args.push("--wait");
            }
            let output = tokio::task::spawn_blocking(move || {
                cap()
                    .args(args)
                    .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_test_token")
                    .env("CAP_SERVER_URL", format!("http://{address}"))
                    .output()
                    .expect("failed to spawn cap binary")
            })
            .await
            .unwrap();
            server.abort();
            assert!(server.await.unwrap_err().is_cancelled());
            let requests = receiver.try_iter().collect::<Vec<_>>();
            assert_eq!(requests.len(), expected_requests, "{arguments:?} {timeout}");
            if expected_requests == 0 {
                assert!(!output.status.success());
                assert!(output.stdout.is_empty());
                assert!(
                    stderr(&output)
                        .contains("INVALID_REQUEST: --timeout must be between 1 and 86400 seconds")
                );
                continue;
            }
            assert!(output.status.success(), "{}", stderr(&output));
            assert_eq!(requests[0].0, format!("{method} {path} HTTP/1.1\r\n"));
            assert_eq!(
                serde_json::from_slice::<Value>(&requests[0].1).unwrap(),
                body
            );
            if wait {
                assert_eq!(requests[1].0, "GET /api/v1/operations/op_test HTTP/1.1\r\n");
            }
            let value = parse_json(&output);
            assert_eq!(value["id"], "op_test");
            assert_eq!(value["state"], if wait { "succeeded" } else { "queued" });
        }
    }
}

#[test]
fn help_succeeds_and_lists_commands() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    for command in [
        "export",
        "record",
        "targets",
        "doctor",
        "version",
        "project",
        "recordings",
        "upload",
        "update",
        "screenshot",
        "caps",
        "account",
        "organizations",
        "library",
        "notifications",
        "analytics",
        "developers",
        "jobs",
        "mcp",
        "agents",
    ] {
        assert!(text.contains(command), "help missing '{command}':\n{text}");
    }
}

#[test]
fn no_args_prints_branded_intro() {
    let output = run(&[]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stderr(&output).is_empty(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("██████████████████"), "stdout: {text}");
    assert!(text.contains("/ ___|__ _ _ __"), "stdout: {text}");
    assert!(text.contains("cap record start --screen <id> --detach"));
    assert!(text.contains("cap --help"));
}

#[test]
fn no_args_json_is_parseable() {
    let output = run(&["--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["name"], "cap");
    assert!(json["commands"].is_array());
}

#[test]
fn subcommand_help_succeeds() {
    for command in [
        "export",
        "record",
        "project",
        "targets",
        "doctor",
        "desktop",
        "recordings",
        "upload",
        "update",
        "screenshot",
        "caps",
        "account",
        "organizations",
        "library",
        "notifications",
        "analytics",
        "developers",
        "jobs",
        "mcp",
        "agents",
    ] {
        let output = run(&[command, "--help"]);
        assert!(
            output.status.success(),
            "`cap {command} --help` failed: {}",
            stderr(&output)
        );
    }
}

#[test]
fn unknown_command_fails() {
    let output = run(&["definitely-not-a-command"]);
    assert!(!output.status.success());
}

#[test]
fn version_json_is_parseable() {
    let output = run(&["version", "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["name"], "cap");
    assert!(json["version"].is_string());
    assert!(json["platform"]["os"].is_string());
    assert!(json["distribution"].is_string());
}

#[test]
fn doctor_json_is_parseable() {
    let output = run(&["doctor", "--format", "json"]);
    // doctor may report failing checks; it should still emit valid JSON and exit 0.
    let json = parse_json(&output);
    assert_eq!(json["version"]["name"], "cap");
    assert!(json["checks"].is_array());
    assert!(json["ok"].is_boolean());
    assert!(json["permissions"].is_object());
}

#[test]
fn completions_generate_for_each_shell() {
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let output = run(&["completions", shell]);
        assert!(
            output.status.success(),
            "completions for {shell} failed: {}",
            stderr(&output)
        );
        assert!(!output.stdout.is_empty(), "completions for {shell} empty");
    }
}

#[test]
fn targets_screens_json_is_parseable() {
    let output = run(&["targets", "screens", "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json.is_array(), "expected a JSON array of screens");
}

#[test]
fn targets_all_json_is_parseable() {
    let output = run(&["targets", "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json["screens"].is_array());
    assert!(json["windows"].is_array());
    assert!(json["cameras"].is_array());
}

#[test]
fn targets_format_before_subcommand_is_rejected() {
    // `--format` before the subcommand must not be silently ignored; clap rejects the conflict.
    let output = run(&["targets", "--format", "json", "screens"]);
    assert!(!output.status.success());
}

#[test]
fn record_requires_duration_when_non_interactive() {
    // The test harness gives the child a non-TTY stdin, so recording without --duration must error
    // rather than stop instantly on EOF.
    let output = run(&["record"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("--duration") || stderr(&output).contains("interactive"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn version_text_reports_distribution() {
    let output = run(&["version"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("distribution:"));
}

#[test]
fn desktop_status_json_is_parseable() {
    let output = run(&["desktop", "status", "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json["installed"].is_boolean());
    assert!(json["onPath"].is_boolean());
    assert!(json["shimPath"].is_string());
}

#[test]
fn project_inspect_missing_fails() {
    let output = run(&[
        "project",
        "inspect",
        "/this/path/does/not/exist.cap",
        "--format",
        "json",
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).to_lowercase().contains("failed to load"));
}

#[test]
fn project_commands_report_duration_for_readable_media() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    write_test_media(&project.join("content/display.mp4"));
    write_single_segment_meta(&project);

    for command in ["validate", "inspect"] {
        let output = run(&[
            "project",
            command,
            project.to_str().unwrap(),
            "--format",
            "json",
        ]);
        assert!(
            output.status.success(),
            "{command} stderr: {}",
            stderr(&output)
        );
        let json = parse_json(&output);
        let duration = json["durationSeconds"]
            .as_f64()
            .unwrap_or_else(|| panic!("{command} did not report durationSeconds: {json}"));
        assert!(
            (duration - 1.0).abs() < 0.25,
            "{command} reported unexpected duration {duration}"
        );
    }
}

#[test]
fn project_commands_omit_duration_for_unreadable_media() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    let content = project.join("content");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(content.join("display.mp4"), b"fake").unwrap();
    write_single_segment_meta(&project);

    for command in ["validate", "inspect"] {
        let output = run(&[
            "project",
            command,
            project.to_str().unwrap(),
            "--format",
            "json",
        ]);
        assert!(
            output.status.success(),
            "{command} stderr: {}",
            stderr(&output)
        );
        let json = parse_json(&output);
        assert!(
            json.get("durationSeconds").is_none(),
            "{command} reported a duration for unreadable media: {json}"
        );
    }
}

#[test]
fn project_validate_missing_meta_reports_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&[
        "project",
        "validate",
        dir.path().to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["valid"], false);
    assert!(json["error"].is_string());
}

#[test]
fn project_validate_complete_project_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    let content = project.join("content");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(content.join("display.mp4"), b"fake").unwrap();
    write_single_segment_meta(&project);

    let output = run(&[
        "project",
        "validate",
        project.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["valid"], true);
    assert_eq!(json["recordingType"], "studio");
}

#[test]
fn project_validate_detects_missing_media() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    std::fs::create_dir_all(&project).unwrap();
    write_single_segment_meta(&project);

    let output = run(&[
        "project",
        "validate",
        project.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["valid"], false);
    let missing = json["missing"].as_array().unwrap();
    assert!(!missing.is_empty(), "expected missing media files");
}

#[test]
fn project_validate_rejects_in_progress_zero_segment_studio_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    std::fs::create_dir_all(&project).unwrap();
    let meta = serde_json::json!({
        "platform": "Linux",
        "pretty_name": "Broken Linux Recording",
        "sharing": null,
        "segments": [],
        "status": { "status": "InProgress" }
    });
    std::fs::write(
        project.join("recording-meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    let output = run(&[
        "project",
        "validate",
        project.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["valid"], false);
    assert!(json["error"].is_string());
    let problems = json["problems"].as_array().unwrap();
    assert!(
        problems.iter().any(|problem| problem
            .as_str()
            .is_some_and(|value| value.contains("no segments"))),
        "expected no-segments validation problem: {json}"
    );
}

#[test]
fn project_inspect_complete_project_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    let content = project.join("content");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(content.join("display.mp4"), b"fake").unwrap();
    write_single_segment_meta(&project);

    let output = run(&[
        "project",
        "inspect",
        project.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["meta"]["pretty_name"], "Test Project");
}

#[test]
fn record_rejects_non_positive_duration() {
    let output = run(&["record", "--duration", "0"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Duration") || stdout(&output).contains("Duration"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn record_rejects_zero_fps() {
    let output = run(&["record", "--fps", "0", "--duration", "1"]);
    assert!(!output.status.success());
}

#[test]
fn record_validation_failures_emit_json_error_events() {
    for command in [
        &["record"][..],
        &["record", "start"][..],
        &["record", "start", "--detach"][..],
    ] {
        for json_flag in [&["--json"][..], &["--format", "json"][..]] {
            for (params, expected_error) in [
                (&["--duration", "0"][..], "Duration"),
                (&["--duration", "NaN"][..], "Duration"),
                (&["--duration", "inf"][..], "Duration"),
                (&["--fps", "0", "--duration", "1"][..], "--fps"),
                (&["--duration", "1"][..], "No target specified"),
            ] {
                let mut args = command.to_vec();
                args.extend_from_slice(json_flag);
                args.extend_from_slice(params);
                let output = run(&args);
                assert_eq!(output.status.code(), Some(1), "args: {args:?}");
                let json = parse_json(&output);
                assert_eq!(json["type"], "error", "args: {args:?}");
                let error = json["error"].as_str().expect("expected an error message");
                assert!(error.contains(expected_error), "args: {args:?}: {error}");
                assert!(stderr(&output).contains(error), "args: {args:?}");
            }
        }
    }
}

#[test]
fn export_rejects_settings_json_with_flags() {
    let output = run(&[
        "export",
        "/tmp/whatever.cap",
        "--settings-json",
        "{\"format\":\"Mp4\"}",
        "--fps",
        "30",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot be combined"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn export_rejects_quality_with_gif() {
    let output = run(&[
        "export",
        "/tmp/whatever.cap",
        "--format",
        "gif",
        "--quality",
        "web",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("only supported for"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn export_rejects_bad_resolution() {
    let output = run(&[
        "export",
        "/tmp/whatever.cap",
        "--resolution",
        "not-a-resolution",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).to_lowercase().contains("resolution"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn export_rejects_duplicate_output() {
    let output = run(&[
        "export",
        "/tmp/whatever.cap",
        "/tmp/out1.mp4",
        "--output",
        "/tmp/out2.mp4",
    ]);
    assert!(!output.status.success());
}

#[test]
fn export_several_projects_requires_output_directory() {
    let output = run(&["export", "/tmp/whatever.cap", "/tmp/also.cap"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("output directory"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn export_batch_reports_every_failed_project() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&[
        "export",
        "/tmp/whatever.cap",
        "/tmp/also.cap",
        "-o",
        dir.path().to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert_eq!(
        text.matches("Failed to export ").count(),
        2,
        "stderr: {text}"
    );
}

#[test]
fn export_batch_json_streams_file_events_and_summary() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&[
        "export",
        "/tmp/whatever.cap",
        "/tmp/also.cap",
        "-o",
        dir.path().to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success());
    let text = stdout(&output);
    for event in ["\"FileStarted\"", "\"FileFailed\"", "\"BatchCompleted\""] {
        assert!(text.contains(event), "stdout missing {event}: {text}");
    }
    let last = text.lines().last().unwrap_or_default();
    assert!(last.contains("\"Error\""), "last line: {last}");
}

#[test]
fn targets_mics_json_is_parseable() {
    let output = run(&["targets", "mics", "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json.is_array(), "expected a JSON array of mics");
}

#[test]
fn targets_all_json_includes_mics() {
    let output = run(&["targets", "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json["mics"].is_array());
}

#[test]
fn recordings_list_empty_dir_is_empty_json() {
    let dir = tempfile::tempdir().unwrap();
    let output = run(&[
        "recordings",
        "list",
        "--dir",
        dir.path().to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json.as_array().map(|a| a.len()), Some(0));
}

#[cfg(target_os = "linux")]
#[test]
fn recordings_list_follows_desktop_storage_unless_dir_is_explicit() {
    let root = tempfile::tempdir().unwrap();
    let app_data = root.path().join("so.cap.desktop");
    let default = app_data.join("recordings");
    let current = root.path().join("custom");
    let previous = root.path().join("previous");
    for dir in [&default, &current, &previous] {
        write_single_segment_meta(&dir.join("example.cap"));
    }
    std::fs::write(
        app_data.join("store"),
        serde_json::to_vec(&serde_json::json!({
            "general_settings": {
                "recordingsPath": current,
                "previousRecordingsPaths": [previous, current]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let list = |dir: Option<&Path>| {
        let mut command = cap();
        command
            .env("XDG_DATA_HOME", root.path())
            .args(["recordings", "list", "--json"]);
        if let Some(dir) = dir {
            command.arg("--dir").arg(dir);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "stderr: {}", stderr(&output));
        parse_json(&output)
    };

    assert_eq!(list(None).as_array().unwrap().len(), 3);
    let explicit = list(Some(&current));
    assert_eq!(explicit.as_array().unwrap().len(), 1);
    assert_eq!(
        explicit[0]["path"],
        current.join("example.cap").to_str().unwrap()
    );
}

#[test]
fn project_config_set_then_get_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    write_single_segment_meta(&project);

    let set = run(&[
        "project",
        "config",
        "set",
        project.to_str().unwrap(),
        "--settings-json",
        "{}",
    ]);
    assert!(set.status.success(), "stderr: {}", stderr(&set));
    assert!(project.join("project-config.json").exists());

    let get = run(&[
        "project",
        "config",
        "get",
        project.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(get.status.success(), "stderr: {}", stderr(&get));
    let json = parse_json(&get);
    assert!(json.is_object(), "expected a project config object");
}

#[test]
fn project_config_get_without_file_returns_default() {
    // Instant / un-edited projects have no project-config.json; `config get` should still succeed
    // with the effective default config rather than erroring.
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("recording.cap");
    write_single_segment_meta(&project);

    let output = run(&[
        "project",
        "config",
        "get",
        project.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    let expected_json =
        serde_json::to_string(&cap_project::ProjectConfiguration::default()).unwrap();
    assert_eq!(json, serde_json::from_str::<Value>(&expected_json).unwrap());
}

#[test]
fn project_config_commands_reject_directories_without_recordings() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("not-a-recording.cap");
    std::fs::create_dir(&project).unwrap();

    for command in ["get", "set"] {
        let mut args = vec!["project", "config", command, project.to_str().unwrap()];
        if command == "set" {
            args.extend(["--settings-json", "{}"]);
        }
        args.push("--json");

        let output = run(&args);
        assert!(!output.status.success(), "{command} unexpectedly succeeded");
        assert!(parse_json(&output)["error"].is_string());
    }

    assert!(!project.join("project-config.json").exists());
}

#[test]
fn export_missing_project_emits_json_error_event() {
    let output = run(&["export", "/this/path/does/not/exist.cap", "--progress-json"]);
    assert!(!output.status.success());
    // The NDJSON stream must end with a machine-readable terminal error rather than just stopping.
    // The `error` field is uniform across every JSON-emitting command (the `type` tag stays Error).
    let json = parse_json(&output);
    assert_eq!(json["type"], "Error");
    assert!(json["error"].is_string());
}

#[test]
fn export_global_json_implies_progress_stream() {
    // `--json` (global) must behave like --progress-json/--completion-json so the agent's universal
    // reflex works on export too; failure still ends with a terminal Error event.
    let output = run(&["export", "/this/path/does/not/exist.cap", "--json"]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["type"], "Error");
    assert!(json["error"].is_string());
}

#[test]
fn export_preview_missing_project_emits_json_error() {
    let output = run(&[
        "export-preview",
        "/this/path/does/not/exist.cap",
        "--frame-time",
        "0",
        "--settings-json",
        "{}",
    ]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert!(json["error"].is_string());
}

#[test]
fn record_no_target_json_emits_error_event() {
    // --duration satisfies the non-interactive guard, so this fails on target resolution and must
    // report that failure as a JSON event on stdout.
    let output = run(&["record", "--duration", "1", "--format", "json"]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["type"], "error");
    assert!(json["error"].is_string());
}

#[test]
fn global_json_flag_works_before_subcommand() {
    // The order-insensitive global --json is the headline agent ergonomic: it must parse in front of
    // the verb and force JSON output the same as a trailing `--format json`.
    let output = run(&["--json", "version"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["name"], "cap");
}

#[test]
fn guide_json_is_parseable_and_self_describing() {
    let output = run(&["guide", "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["binary"], "cap");
    assert_eq!(json["schemaVersion"], 3);
    assert!(json["commands"].is_array());
    assert!(json["env"].is_array());
    assert!(json["outputConvention"].is_object());
    let commands = serde_json::to_string(&json["commands"]).unwrap();
    assert!(commands.contains("caps unlock"));
    assert!(commands.contains("caps comments|reactions|update|sharing"));
    assert!(commands.contains("caps import loom"));
    assert!(commands.contains("account get|update|image|referrals|sign-out-all"));
    assert!(commands.contains(
        "organizations create|update|icon|shareable-icon|settings|invite|member|domain|delete"
    ));
    assert!(commands.contains("organizations billing get|checkout|portal"));
    assert!(commands.contains("organizations storage list|s3|provider|google-drive"));
    assert!(commands.contains("developers list|get|create|update|delete"));
    assert!(commands.contains("project captions get|export|style"));
    assert!(commands.contains("project title-card get|set|remove|export"));
}

#[test]
fn dashboard_parity_help_exposes_secure_command_trees() {
    for args in [
        &["account", "image", "--help"][..],
        &["account", "referrals", "--help"][..],
        &["organizations", "billing", "--help"][..],
        &["organizations", "storage", "--help"][..],
        &["organizations", "storage", "s3", "--help"][..],
        &["organizations", "storage", "google-drive", "--help"][..],
        &["organizations", "invite", "add", "--help"][..],
        &["developers", "credits", "--help"][..],
        &["developers", "videos", "--help"][..],
        &["developers", "transactions", "--help"][..],
        &["library", "folders", "public-page", "--help"][..],
        &["library", "folders", "logo", "--help"][..],
        &["library", "spaces", "public-page", "--help"][..],
        &["library", "spaces", "logo", "--help"][..],
        &["caps", "import", "loom", "--help"][..],
    ] {
        let output = run(args);
        assert!(output.status.success(), "stderr: {}", stderr(&output));
    }

    let output = run(&["organizations", "storage", "s3", "set", "--help"]);
    let help = stdout(&output).to_lowercase();
    assert!(!help.contains("access-key"));
    assert!(!help.contains("secret-access"));

    let output = run(&["organizations", "invite", "add", "--help"]);
    assert!(stdout(&output).contains("--no-email"));
}

#[test]
fn non_interactive_s3_requires_secure_credential_input() {
    let output = cap()
        .args([
            "organizations",
            "storage",
            "s3",
            "set",
            "org_synthetic",
            "--bucket",
            "synthetic",
            "--yes",
            "--format",
            "json",
        ])
        .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_test_token")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("--credentials-stdin or --reuse-credentials"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn caps_help_lists_the_agent_interface() {
    let output = run(&["caps", "--help"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    for command in [
        "list",
        "get",
        "context",
        "status",
        "wait",
        "transcript",
        "download",
        "process",
        "transcript-replace",
        "duplicate",
        "delete",
        "password",
        "unlock",
        "comments",
        "reactions",
        "update",
        "sharing",
    ] {
        assert!(
            text.contains(command),
            "caps help missing {command}:\n{text}"
        );
    }
    for args in [
        &["caps", "comments", "--help"][..],
        &["caps", "reactions", "--help"][..],
        &["caps", "sharing", "--help"][..],
    ] {
        let output = run(args);
        assert!(output.status.success(), "stderr: {}", stderr(&output));
    }
}

#[test]
fn agent_content_mutations_require_explicit_confirmation() {
    for args in [
        vec!["caps", "comments", "add", "cap_synthetic", "hello"],
        vec![
            "caps",
            "comments",
            "reply",
            "cap_synthetic",
            "comment_synthetic",
            "hello",
        ],
        vec!["caps", "reactions", "add", "cap_synthetic", "thumbs-up"],
        vec![
            "caps",
            "update",
            "cap_synthetic",
            "--title",
            "Synthetic title",
        ],
        vec!["caps", "sharing", "set", "cap_synthetic", "--public"],
    ] {
        let output = cap()
            .args(args)
            .arg("--format")
            .arg("json")
            .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_test_token")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(parse_json(&output)["code"], "INVALID_REQUEST");
    }
}

#[test]
fn sharing_requires_one_visibility_flag() {
    let output = run(&["caps", "sharing", "set", "cap_synthetic"]);
    assert_eq!(output.status.code(), Some(2));
    let output = run(&[
        "caps",
        "sharing",
        "set",
        "cap_synthetic",
        "--public",
        "--private",
    ]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn agent_installer_dry_run_is_machine_readable() {
    let output = run(&[
        "agents",
        "install",
        "--target",
        "codex",
        "--component",
        "all",
        "--dry-run",
        "--yes",
        "--format",
        "json",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["target"], "codex");
    assert_eq!(json["dryRun"], true);
    assert_eq!(json["applied"], false);
    assert_eq!(json["changes"].as_array().map(Vec::len), Some(2));
}

#[test]
fn new_commands_emit_json_errors_for_local_format_flag() {
    let caps_output = cap()
        .args(["caps", "get", "!", "--format", "json"])
        .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_test_token")
        .output()
        .unwrap();
    assert_eq!(caps_output.status.code(), Some(1));
    assert_eq!(parse_json(&caps_output)["code"], "INVALID_REQUEST");

    let auth_output = run(&["auth", "login", "--timeout", "0", "--format", "json"]);
    assert_eq!(auth_output.status.code(), Some(1));
    assert!(parse_json(&auth_output)["error"].is_string());

    let home = tempfile::tempdir().unwrap();
    let installer_output = cap()
        .args([
            "agents",
            "install",
            "--target",
            "codex",
            "--component",
            "skill",
            "--format",
            "json",
        ])
        .env("HOME", home.path())
        .env("CODEX_HOME", home.path().join(".codex"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(installer_output.status.code(), Some(1));
    assert!(parse_json(&installer_output)["error"].is_string());
}

#[test]
fn mcp_stdout_is_protocol_only_and_exposes_expected_tools() {
    let mut command = cap();
    command
        .args(["mcp", "serve"])
        .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_test_token")
        .env("CAP_SERVER_URL", "http://127.0.0.1:9")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("failed to start MCP server");
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "cap-test", "version": "1" }
            }
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let initialize = receiver.recv_timeout(Duration::from_secs(10));
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let tools = receiver.recv_timeout(Duration::from_secs(10));
    let _ = child.kill();
    let _ = child.wait();

    let initialize: Value = serde_json::from_str(&initialize.unwrap().unwrap()).unwrap();
    assert_eq!(initialize["id"], 1);
    assert_eq!(initialize["result"]["serverInfo"]["name"], "cap");
    let tools: Value = serde_json::from_str(&tools.unwrap().unwrap()).unwrap();
    assert_eq!(tools["id"], 2);
    let tool_list = tools["result"]["tools"].as_array().unwrap();
    let tool = |name: &str| {
        tool_list
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing MCP tool {name}"))
    };
    assert_eq!(tool("caps_list")["annotations"]["idempotentHint"], true);
    assert_eq!(
        tool("caps_import_loom")["annotations"]["idempotentHint"],
        false
    );
    for tool in tool_list {
        if tool["annotations"]["readOnlyHint"] == false {
            assert_eq!(
                tool["annotations"]["idempotentHint"], false,
                "mutating MCP tool {} must not claim cross-invocation idempotency",
                tool["name"]
            );
        }
    }
    let serialized = serde_json::to_string(tool_list).unwrap();
    for name in [
        "caps_list",
        "caps_get",
        "caps_context",
        "caps_wait",
        "caps_import_loom",
        "caps_comment",
        "caps_reply",
        "caps_react",
        "caps_update_title",
        "caps_set_visibility",
        "account_referrals_open",
        "organizations_list",
        "organization_create",
        "organization_billing_checkout",
        "organization_billing_portal",
        "organization_storage_provider_set",
        "organization_google_drive_connect",
        "organization_google_drive_folders",
        "organization_google_drive_location_set",
        "organization_google_drive_disconnect",
        "organization_update",
        "organization_settings_update",
        "organization_invite_add",
        "organization_member_role",
        "organization_member_seat",
        "organization_delete",
        "organization_domain_set",
        "organization_domain_remove",
        "organization_domain_verify",
        "collection_public_page_update",
        "developer_apps_list",
        "developer_videos_list",
        "developer_transactions_list",
        "developer_video_delete",
        "developer_app_update",
        "developer_domain_add",
        "developer_auto_top_up_update",
        "developer_credits_checkout",
    ] {
        assert!(serialized.contains(name), "missing MCP tool {name}");
    }
    assert!(!serialized.to_lowercase().contains("password"));
    assert!(!serialized.contains("access_key_id"));
    assert!(!serialized.contains("secret_access_key"));
    assert!(!serialized.contains("image_data"));
}

#[test]
fn mcp_cancellation_stops_wait_polling() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let request_count = Arc::new(AtomicUsize::new(0));
    let server_request_count = request_count.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            server_request_count.fetch_add(1, Ordering::SeqCst);
            let body = r#"{"transcript":{"status":"processing"},"ai":{"status":"processing"}}"#;
            write!(
				stream,
				"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
				body.len(),
				body
			)
			.unwrap();
            stream.flush().unwrap();
        }
    });

    let mut command = cap();
    command
        .args(["mcp", "serve"])
        .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_test_token")
        .env("CAP_SERVER_URL", server_url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("failed to start MCP server");
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdin = child.stdin.take().unwrap();
    for message in [
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "cap-test", "version": "1" }
            }
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "caps_wait",
                "arguments": { "cap": "cap_synthetic", "timeout_seconds": 60 }
            }
        }),
    ] {
        writeln!(stdin, "{message}").unwrap();
    }
    stdin.flush().unwrap();
    let initialize: Value = serde_json::from_str(
        &receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(initialize["id"], 1);
    for _ in 0..50 {
        if request_count.load(Ordering::SeqCst) > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(request_count.load(Ordering::SeqCst), 1);
    for message in [
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": 3, "reason": "test" }
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/list",
            "params": {}
        }),
    ] {
        writeln!(stdin, "{message}").unwrap();
    }
    stdin.flush().unwrap();
    let mut list_response = None;
    for _ in 0..3 {
        let response: Value = serde_json::from_str(
            &receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        if response["id"] == 4 {
            list_response = Some(response);
            break;
        }
    }
    thread::sleep(Duration::from_millis(750));
    let _ = child.kill();
    let _ = child.wait();
    assert!(list_response.is_some());
    assert_eq!(request_count.load(Ordering::SeqCst), 1);
}

#[test]
fn record_status_empty_is_json_array() {
    let output = run(&["record", "status", "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json.is_array(), "expected a JSON array of sessions");
}

#[test]
fn record_stop_unknown_id_fails_with_json_error() {
    let output = run(&["record", "stop", "--id", "does-not-exist", "--json"]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["type"], "error");
    assert!(json["error"].is_string());
}

#[test]
fn doctor_exits_zero_even_when_checks_fail() {
    // doctor is a report, not a gate: agents branch on `ok`/`captureReady`, so it must exit 0.
    let output = run(&["doctor", "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json["ok"].is_boolean());
    assert!(json["captureReady"].is_boolean());
}

#[test]
fn doctor_check_ids_are_the_pinned_vocabulary() {
    let output = run(&["doctor", "--json"]);
    let json = parse_json(&output);
    let ids: Vec<&str> = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    #[cfg(target_os = "macos")]
    assert_eq!(
        ids,
        [
            "ffmpeg",
            "screenRecordingPermission",
            "screenCaptureKit",
            "cliInstall"
        ]
    );

    #[cfg(not(target_os = "macos"))]
    assert_eq!(ids, ["ffmpeg", "screenRecordingPermission", "cliInstall"]);
}

#[test]
fn clean_error_has_no_debug_quotes() {
    // main() must print `error: <message>` (not the default `Error: "debug-quoted"`); a leading
    // `Error: "` would mean agents scraping stderr get stray quotes/escapes.
    let output = run(&["record", "--duration", "1"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("error:"), "stderr: {err}");
    assert!(!err.contains("Error: \""), "stderr had debug quotes: {err}");
}

#[test]
fn upload_missing_file_emits_json_error() {
    // Whether auth comes from CAP_API_KEY, the desktop login, or is absent, uploading a path that
    // does not exist must fail with a machine-readable error before any network call.
    let output = run(&["upload", "/tmp/does-not-exist-cap.mp4", "--format", "json"]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert!(json["error"].is_string());
}

#[test]
fn auth_status_json_reports_source() {
    let output = run(&["auth", "status", "--json"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert!(json["authenticated"].is_boolean());
    // source is one of env|desktop|none; server is always reported.
    assert!(json["source"].is_string());
    assert!(json["server"].is_string());
}

#[test]
fn auth_status_recognizes_agent_environment_credentials() {
    let output = cap()
        .args(["auth", "status", "--json"])
        .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_agent_status")
        .env("CAP_SERVER_URL", "http://127.0.0.1:9876")
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["authenticated"], false);
    assert_eq!(json["credentialPresent"], true);
    assert_eq!(json["serverVerified"], false);
    assert_eq!(json["verificationStatus"], "unavailable");
    assert_eq!(json["source"], "env");
    assert_eq!(json["server"], "http://127.0.0.1:9876");
    assert!(!stdout(&output).contains("cap_cli_synthetic_agent_status"));
}

#[test]
fn auth_status_verifies_agent_credentials_with_the_server() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let server_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let length = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(request.starts_with("GET /api/v1/auth/status HTTP/1.1"));
        assert!(request.contains("authorization: Bearer cap_cli_synthetic_agent_status"));
        let body = r#"{"authenticated":true,"tokenKind":"agent","expiresAt":"2027-01-01T00:00:00.000Z","scopes":["caps:read"],"requestId":"request-test"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let output = cap()
        .args(["auth", "status", "--json"])
        .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_agent_status")
        .env("CAP_SERVER_URL", server_url)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json = parse_json(&output);
    assert_eq!(json["authenticated"], true);
    assert_eq!(json["credentialPresent"], true);
    assert_eq!(json["serverVerified"], true);
    assert_eq!(json["verificationStatus"], "verified");
    assert_eq!(json["expiresAt"], "2027-01-01T00:00:00.000Z");
    assert_eq!(json["scopes"], serde_json::json!(["caps:read"]));
    assert!(!stdout(&output).contains("cap_cli_synthetic_agent_status"));
}

#[test]
fn agent_credentials_reject_insecure_remote_servers() {
    let output = cap()
        .args(["caps", "list", "--json"])
        .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_agent_status")
        .env("CAP_SERVER_URL", "http://example.com")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("require HTTPS"));
}

#[test]
fn auth_login_defaults_to_the_creator_profile() {
    let output = run(&["auth", "login", "--help"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("[default: creator]"));
}

fn write_test_media(path: &Path) {
    use ffmpeg::format::Pixel;
    ffmpeg::init().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut output = ffmpeg::format::output(path).unwrap();
    let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::MPEG4).unwrap();
    let mut encoder = ffmpeg::codec::Context::new_with_codec(codec)
        .encoder()
        .video()
        .unwrap();
    encoder.set_width(32);
    encoder.set_height(32);
    encoder.set_format(Pixel::YUV420P);
    encoder.set_time_base((1, 30));
    encoder.set_frame_rate(Some((30, 1)));
    encoder.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    let mut encoder = encoder.open_as(codec).unwrap();
    output.add_stream(codec).unwrap().set_parameters(&encoder);
    output.write_header().unwrap();
    let output_time_base = output.stream(0).unwrap().time_base();
    {
        let mut drain = |encoder: &mut ffmpeg::encoder::video::Encoder, finished: bool| loop {
            let mut packet = ffmpeg::Packet::empty();
            match encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.set_stream(0);
                    packet.rescale_ts((1, 30), output_time_base);
                    packet.write_interleaved(&mut output).unwrap();
                }
                Err(ffmpeg::Error::Other { errno })
                    if !finished && errno == ffmpeg::ffi::EAGAIN =>
                {
                    break;
                }
                Err(ffmpeg::Error::Eof) if finished => break,
                Err(error) => panic!("test media encoding failed: {error}"),
            }
        };
        for index in 0..30 {
            let mut frame = ffmpeg::frame::Video::new(Pixel::YUV420P, 32, 32);
            frame.data_mut(0).fill(32 + index as u8);
            frame.data_mut(1).fill(128);
            frame.data_mut(2).fill(128);
            frame.set_pts(Some(index));
            encoder.send_frame(&frame).unwrap();
            drain(&mut encoder, false);
        }
        encoder.send_eof().unwrap();
        drain(&mut encoder, true);
    }
    output.write_trailer().unwrap();
}

fn write_single_segment_meta(project: &Path) {
    std::fs::create_dir_all(project).unwrap();
    let meta = serde_json::json!({
        "pretty_name": "Test Project",
        "display": { "path": "content/display.mp4", "fps": 30 }
    });
    std::fs::write(
        project.join("recording-meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();
}

#[test]
fn caps_json_errors_include_the_standard_error_field() {
    for flags in [&["--json"][..], &["--format", "json"][..]] {
        let output = cap()
            .args(["caps", "get", "!"])
            .args(flags)
            .env("CAP_AGENT_TOKEN", "cap_cli_synthetic_test_token")
            .env("CAP_SERVER_URL", "http://127.0.0.1:9")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let error = parse_json(&output);
        assert_eq!(error["error"], "Cap ID contains invalid characters");
        assert_eq!(error["message"], "Cap ID contains invalid characters");
        assert_eq!(error["code"], "INVALID_REQUEST");
        assert_eq!(error["retryable"], false);
    }
}

fn write_caption_project(project: &Path) {
    write_single_segment_meta(project);
    let config = serde_json::json!({
        "futureEditorField": {"preserve": true},
        "captions": {
            "segments": [{"id": "intro", "start": 0.25, "end": 0.75, "text": "开始录制"}],
            "sourceTimed": true,
            "settings": {
                "enabled": true, "font": "Noto Sans SC", "size": 32,
                "futureCaptionField": "preserve",
                "translations": {"intro": {"sourceText": "开始录制", "text": "Start recording", "language": "en"}}
            }
        },
        "timeline": {
            "segments": [{"recordingSegment": 0, "start": 0.25, "end": 0.9, "timescale": 1.0}],
            "zoomSegments": [], "captionSegments": []
        }
    });
    std::fs::write(
        project.join("project-config.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
}

fn write_title_card_project(project: &Path) {
    write_caption_project(project);
    let path = project.join("project-config.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["timeline"]["futureTimelineField"] = "keep".into();
    value["timeline"]["textSegments"] = serde_json::json!([
        {"start": 0.1, "end": 0.4, "content": "已有文字", "futureTextField": {"keep": true}}
    ]);
    value["timeline"]["audioSegments"] = serde_json::json!([
        {"start": 0.0, "end": 0.6, "path": "music.wav", "futureMusicField": true}
    ]);
    std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
fn title_card_image_refuses_disabled_duplicate_or_misplaced_cards_without_rewriting() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let config_path = project.join("project-config.json");
    let mut config: cap_project::ProjectConfiguration =
        serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config
        .upsert_title_card(cap_project::TextSegment::new_title_card(
            cap_project::TitleCardRole::Outro,
            "感谢观看",
        ))
        .unwrap();
    let timeline = config.timeline.as_ref().unwrap();
    let card_index = timeline
        .text_segments
        .iter()
        .position(|segment| segment.title_card.is_some())
        .unwrap();
    for scenario in ["disabled", "duplicate", "misplaced"] {
        let mut invalid = config.clone();
        let segments = &mut invalid.timeline.as_mut().unwrap().text_segments;
        match scenario {
            "disabled" => segments[card_index].enabled = false,
            "duplicate" => segments.push(segments[card_index].clone()),
            "misplaced" => {
                segments[card_index].start += 0.5;
                segments[card_index].end += 0.5;
            }
            _ => unreachable!(),
        }
        let before = serde_json::to_vec_pretty(&invalid).unwrap();
        std::fs::write(&config_path, &before).unwrap();
        let output_path = directory.path().join(format!("{scenario}.png"));
        let output = run(&[
            "project",
            "title-card",
            "export",
            project.to_str().unwrap(),
            "--role",
            "outro",
            "--output",
            output_path.to_str().unwrap(),
            "--dry-run",
            "--json",
        ]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{scenario}: {}",
            stderr(&output)
        );
        assert_eq!(parse_json(&output)["code"], "INVALID_PROJECT", "{scenario}");
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
        assert!(!output_path.exists());
    }
}

#[test]
fn title_card_image_preview_preserves_project_and_checks_roles_revisions_and_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let config_path = project.join("project-config.json");
    let mut config: cap_project::ProjectConfiguration =
        serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config
        .upsert_title_card(cap_project::TextSegment::new_title_card(
            cap_project::TitleCardRole::Outro,
            "感谢观看",
        ))
        .unwrap();
    std::fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let before = std::fs::read(&config_path).unwrap();
    let output_path = directory.path().join("cover.png");
    let args = [
        "project",
        "title-card",
        "export",
        project.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
        "--dry-run",
        "--json",
    ];
    let missing = run(&args);
    assert_eq!(parse_json(&missing)["code"], "NO_TITLE_CARD");
    let preview = run(&[args.as_slice(), &["--role", "outro"]].concat());
    assert!(preview.status.success(), "{}", stderr(&preview));
    let value = parse_json(&preview);
    assert_eq!(value["operation"], "title-card.export");
    assert_eq!(value["written"], false);
    assert_eq!(value["role"], "outro");
    let card = config
        .title_card(cap_project::TitleCardRole::Outro)
        .unwrap();
    let frame_time = value["frameTime"].as_f64().unwrap();
    assert!(frame_time >= card.start && frame_time < card.end);
    for extra in [
        vec!["--width", "0"],
        vec!["--height", "16385"],
        vec!["--width", "10000", "--height", "10000"],
    ] {
        let rejected = run(&[args.as_slice(), &["--role", "outro"], extra.as_slice()].concat());
        assert_eq!(rejected.status.code(), Some(1));
        assert_eq!(parse_json(&rejected)["code"], "INVALID_ARGUMENT");
    }
    let conflict = run(&[
        args.as_slice(),
        &["--role", "outro", "--expected-revision", "stale"],
    ]
    .concat());
    assert_eq!(parse_json(&conflict)["code"], "REVISION_CONFLICT");
    assert_eq!(std::fs::read(&config_path).unwrap(), before);
    assert!(!output_path.exists());
    assert!(!project.join("output").exists());
    assert!(!project.join(".cap-project-edit.lock").exists());
}

#[test]
fn title_card_image_rejects_existing_paths_and_wrong_extensions_before_rendering() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let before = std::fs::read(project.join("project-config.json")).unwrap();
    for (name, expected) in [
        ("existing.png", "OUTPUT_EXISTS"),
        ("cover.mp4", "INVALID_ARGUMENT"),
        ("missing/cover.png", "INVALID_ARGUMENT"),
    ] {
        let output_path = directory.path().join(name);
        if name == "existing.png" {
            std::fs::write(&output_path, b"original image").unwrap();
        }
        let output = run(&[
            "project",
            "title-card",
            "export",
            project.to_str().unwrap(),
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(parse_json(&output)["code"], expected);
    }
    assert_eq!(
        std::fs::read(directory.path().join("existing.png")).unwrap(),
        b"original image"
    );
    assert_eq!(
        std::fs::read(project.join("project-config.json")).unwrap(),
        before
    );
}

#[test]
fn title_card_preview_apply_and_remove_preserve_body_music_and_unknown_fields() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let path = project.join("project-config.json");
    let bytes = std::fs::read(&path).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let args = [
        "project",
        "title-card",
        "set",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--title",
        "把复杂的操作讲清楚",
        "--subtitle",
        "三步完成第一次录制",
        "--duration",
        "2.5",
        "--json",
    ];
    let preview = run(&[args.as_slice(), &["--dry-run"]].concat());
    assert!(preview.status.success(), "{}", stderr(&preview));
    let preview = parse_json(&preview);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(preview["written"], false);
    assert_eq!(preview["after"]["intro"]["end"], 2.5);
    assert_eq!(preview["after"]["intro"]["fontSize"], 96.0);
    let revision = preview["revision"].as_str().unwrap();
    let applied = run(&[args.as_slice(), &["--expected-revision", revision]].concat());
    assert!(applied.status.success(), "{}", stderr(&applied));
    let applied = parse_json(&applied);
    assert_eq!(applied["after"], preview["after"]);
    assert_eq!(applied["resultRevision"], preview["resultRevision"]);
    let updated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(updated["futureEditorField"], original["futureEditorField"]);
    assert_eq!(updated["timeline"]["futureTimelineField"], "keep");
    assert_eq!(updated["timeline"]["textSegments"][0]["start"], 2.6);
    assert_eq!(
        updated["timeline"]["textSegments"][0]["futureTextField"],
        original["timeline"]["textSegments"][0]["futureTextField"]
    );
    assert_eq!(
        updated["timeline"]["audioSegments"],
        original["timeline"]["audioSegments"]
    );
    assert_eq!(updated["captions"], original["captions"]);
    let before_noop = std::fs::read(&path).unwrap();
    let repeated = run(&[
        args.as_slice(),
        &[
            "--expected-revision",
            applied["resultRevision"].as_str().unwrap(),
        ],
    ]
    .concat());
    assert!(repeated.status.success(), "{}", stderr(&repeated));
    assert_eq!(parse_json(&repeated)["changed"], false);
    assert_eq!(std::fs::read(&path).unwrap(), before_noop);
    let conflict = run(&[args.as_slice(), &["--expected-revision", revision]].concat());
    assert_eq!(conflict.status.code(), Some(1));
    assert_eq!(parse_json(&conflict)["code"], "REVISION_CONFLICT");
    assert_eq!(std::fs::read(&path).unwrap(), before_noop);
    let removed = run(&[
        "project",
        "title-card",
        "remove",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--expected-revision",
        applied["resultRevision"].as_str().unwrap(),
        "--json",
    ]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    let restored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        restored["timeline"]["textSegments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        (restored["timeline"]["textSegments"][0]["start"]
            .as_f64()
            .unwrap()
            - 0.1)
            .abs()
            < 1e-9
    );
    assert_eq!(
        restored["timeline"]["textSegments"][0]["futureTextField"],
        original["timeline"]["textSegments"][0]["futureTextField"]
    );
    assert_eq!(restored["captions"], original["captions"]);
    assert_eq!(
        restored["timeline"]["audioSegments"],
        original["timeline"]["audioSegments"]
    );
}

#[test]
fn title_card_outro_reanchors_and_keeps_identity_when_intro_is_added() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let path = project.to_str().unwrap();
    let initial = run(&["project", "title-card", "get", path, "--json"]);
    assert!(initial.status.success(), "{}", stderr(&initial));
    let initial = parse_json(&initial);
    let outro = run(&[
        "project",
        "title-card",
        "set",
        path,
        "--role",
        "outro",
        "--title",
        "下一步，自己试一次",
        "--expected-revision",
        initial["revision"].as_str().unwrap(),
        "--json",
    ]);
    assert!(outro.status.success(), "{}", stderr(&outro));
    let outro = parse_json(&outro);
    let intro = run(&[
        "project",
        "title-card",
        "set",
        path,
        "--role",
        "intro",
        "--title",
        "从这里开始",
        "--expected-revision",
        outro["resultRevision"].as_str().unwrap(),
        "--json",
    ]);
    assert!(intro.status.success(), "{}", stderr(&intro));
    let intro = parse_json(&intro);
    assert_eq!(
        intro["after"]["outro"]["titleCard"]["id"],
        outro["after"]["outro"]["titleCard"]["id"]
    );
    assert!((intro["after"]["outro"]["start"].as_f64().unwrap() - 2.65).abs() < 1e-9);
    assert!((intro["after"]["outputDuration"].as_f64().unwrap() - 4.65).abs() < 1e-9);
}

#[test]
fn title_card_rejects_invalid_inputs_and_requires_revision_before_writing() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let config_path = project.join("project-config.json");
    let before = std::fs::read(&config_path).unwrap();
    let args = [
        "project",
        "title-card",
        "set",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--json",
    ];
    for flags in [
        vec!["--dry-run"],
        vec!["--dry-run", "--title", "   "],
        vec!["--dry-run", "--title", "标题", "--duration", "NaN"],
        vec!["--dry-run", "--title", "标题", "--duration", "31"],
        vec!["--dry-run", "--title", "标题", "--font-size", "23"],
        vec!["--dry-run", "--title", "标题", "--font-size", "241"],
        vec!["--dry-run", "--title", "标题", "--background-color", "red"],
        vec![
            "--dry-run",
            "--title",
            "标题",
            "--background-image",
            "../outside.png",
        ],
    ] {
        let output = run(&[args.as_slice(), flags.as_slice()].concat());
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(parse_json(&output)["code"], "INVALID_ARGUMENT");
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
    }
    let no_revision = run(&[args.as_slice(), &["--title", "标题"]].concat());
    assert_eq!(no_revision.status.code(), Some(2));
    assert_eq!(std::fs::read(&config_path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn title_card_rejects_background_symlink_outside_project_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let outside = directory.path().join("outside.png");
    image::RgbImage::new(2, 2).save(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("linked.png")).unwrap();
    let before = std::fs::read(project.join("project-config.json")).unwrap();
    let output = run(&[
        "project",
        "title-card",
        "set",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--title",
        "标题",
        "--background-image",
        "linked.png",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(parse_json(&output)["code"], "INVALID_ARGUMENT");
    assert_eq!(
        std::fs::read(project.join("project-config.json")).unwrap(),
        before
    );
}

#[test]
fn title_card_legacy_text_keeps_appearance_and_new_card_does_not_grow_on_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("legacy.cap");
    write_title_card_project(&project);
    let path = project.join("project-config.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["timeline"]["textSegments"][0]["size"] = serde_json::json!({"x": 0.35, "y": 0.4});
    config["timeline"]["textSegments"][0]["fontSize"] = 60.0.into();
    std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let args = [
        "project",
        "title-card",
        "set",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--title",
        "新封面",
        "--json",
    ];
    let preview = run(&[args.as_slice(), &["--dry-run"]].concat());
    assert!(preview.status.success(), "{}", stderr(&preview));
    let preview = parse_json(&preview);
    let written = run(&[
        args.as_slice(),
        &["--expected-revision", preview["revision"].as_str().unwrap()],
    ]
    .concat());
    assert!(written.status.success(), "{}", stderr(&written));
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["timeline"]["textSegments"][0]["fontSize"], 120.0);
    assert_eq!(
        written["timeline"]["textSegments"][0]["futureTextField"],
        config["timeline"]["textSegments"][0]["futureTextField"]
    );
    let reopened = cap_project::ProjectConfiguration::load(&project).unwrap();
    assert_eq!(
        reopened
            .title_card(cap_project::TitleCardRole::Intro)
            .unwrap()
            .font_size,
        96.0
    );
    assert_eq!(reopened.timeline.unwrap().text_segments[0].font_size, 120.0);
}

#[test]
fn title_card_retimes_unsorted_scenes_without_swapping_their_content_or_unknown_fields() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("scenes.cap");
    write_title_card_project(&project);
    let path = project.join("project-config.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["timeline"]["sceneSegments"] = serde_json::json!([
        {"start": 0.4, "end": 0.6, "mode": "cameraOnly", "futureSceneField": "later"},
        {"start": 0.1, "end": 0.2, "mode": "hideCamera", "futureSceneField": "earlier"}
    ]);
    std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let read = parse_json(&run(&[
        "project",
        "title-card",
        "get",
        project.to_str().unwrap(),
        "--json",
    ]));
    let written = run(&[
        "project",
        "title-card",
        "set",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--title",
        "封面",
        "--expected-revision",
        read["revision"].as_str().unwrap(),
        "--json",
    ]);
    assert!(written.status.success(), "{}", stderr(&written));
    let updated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let scenes = updated["timeline"]["sceneSegments"].as_array().unwrap();
    assert_eq!(scenes[0]["mode"], "cameraOnly");
    assert_eq!(scenes[0]["futureSceneField"], "later");
    assert_eq!(scenes[0]["start"], 2.4);
    assert_eq!(scenes[0]["end"], 2.6);
    assert_eq!(scenes[1]["mode"], "hideCamera");
    assert_eq!(scenes[1]["futureSceneField"], "earlier");
    assert_eq!(scenes[1]["start"], 2.1);
    assert_eq!(scenes[1]["end"], 2.2);
}

#[test]
fn title_card_rejects_image_with_valid_dimensions_but_corrupt_pixels() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let image_path = project.join("broken.png");
    image::RgbImage::new(8, 8).save(&image_path).unwrap();
    let mut bytes = std::fs::read(&image_path).unwrap();
    let offset = bytes
        .windows(4)
        .position(|window| window == b"IDAT")
        .unwrap();
    bytes[offset + 4] ^= 0xff;
    std::fs::write(&image_path, bytes).unwrap();
    assert_eq!(image::image_dimensions(&image_path).unwrap(), (8, 8));
    let path = project.join("project-config.json");
    let before = std::fs::read(&path).unwrap();
    let output = run(&[
        "project",
        "title-card",
        "set",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--title",
        "标题",
        "--background-image",
        "broken.png",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(parse_json(&output)["code"], "INVALID_MEDIA");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn title_card_and_caption_writes_respect_the_same_process_lock() {
    use std::os::fd::AsRawFd;

    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("cards.cap");
    write_title_card_project(&project);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(project.join(".cap-project-edit.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let path = project.join("project-config.json");
    let before = std::fs::read(&path).unwrap();
    let read = parse_json(&run(&[
        "project",
        "title-card",
        "get",
        project.to_str().unwrap(),
        "--json",
    ]));
    for flags in [
        vec![
            "title-card",
            "set",
            project.to_str().unwrap(),
            "--role",
            "intro",
            "--title",
            "标题",
        ],
        vec![
            "captions",
            "style",
            project.to_str().unwrap(),
            "--size",
            "36",
        ],
    ] {
        let output = run(&[
            &["project"][..],
            flags.as_slice(),
            &[
                "--expected-revision",
                read["revision"].as_str().unwrap(),
                "--json",
            ],
        ]
        .concat());
        assert_eq!(output.status.code(), Some(1));
        let result = parse_json(&output);
        assert_eq!(result["code"], "PROJECT_BUSY");
        assert_eq!(result["retryable"], true);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    drop(lock);
    let output = run(&[
        "project",
        "title-card",
        "set",
        project.to_str().unwrap(),
        "--role",
        "intro",
        "--title",
        "标题",
        "--expected-revision",
        read["revision"].as_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn captions_get_projects_edited_time_without_migrating_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("captioned.cap");
    write_caption_project(&project);
    write_test_media(&project.join("content/display.mp4"));
    let before = std::fs::read(project.join("project-config.json")).unwrap();
    let output = run(&[
        "project",
        "captions",
        "get",
        project.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let result = parse_json(&output);
    assert_eq!(result["timeBasis"], "edited-output-seconds");
    assert_eq!(result["coverage"]["translated"], 1);
    assert_eq!(result["captions"][0]["start"], 0.0);
    assert_eq!(result["captions"][0]["end"], 0.5);
    assert_eq!(result["captions"][0]["sourceText"], "开始录制");
    assert_eq!(result["revision"].as_str().unwrap().len(), 64);
    assert_eq!(
        std::fs::read(project.join("project-config.json")).unwrap(),
        before
    );
}

#[test]
fn captions_style_preview_revision_and_patch_preserve_unrelated_data() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("captioned.cap");
    write_caption_project(&project);
    let path = project.join("project-config.json");
    let before = std::fs::read(&path).unwrap();
    let args = [
        "project",
        "captions",
        "style",
        project.to_str().unwrap(),
        "--color",
        "#FFFFFF",
        "--outline",
        "true",
        "--outline-color",
        "#000000",
        "--outline-width",
        "7",
        "--background-opacity",
        "0",
        "--json",
    ];
    let preview = cap().args(args).arg("--dry-run").output().unwrap();
    assert!(preview.status.success(), "{}", stderr(&preview));
    let preview = parse_json(&preview);
    assert_eq!(preview["written"], false);
    assert_eq!(preview["after"]["outlineWidth"], 7.0);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let result = cap()
        .args(args)
        .args(["--expected-revision", preview["revision"].as_str().unwrap()])
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", stderr(&result));
    let changed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(changed["futureEditorField"]["preserve"], true);
    assert_eq!(
        changed["captions"]["settings"]["futureCaptionField"],
        "preserve"
    );
    assert_eq!(changed["captions"]["settings"]["size"], 32);
    assert_eq!(changed["captions"]["segments"][0]["text"], "开始录制");
    assert_eq!(
        changed["captions"]["settings"]["translations"]["intro"]["text"],
        "Start recording"
    );
    let result = cap()
        .args(args)
        .args(["--expected-revision", preview["revision"].as_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(parse_json(&result)["code"], "REVISION_CONFLICT");
    assert_eq!(
        std::fs::read_dir(&project)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        0
    );
}

#[test]
fn captions_style_typography_changes_each_group_independently_and_preserves_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("captioned.cap");
    write_caption_project(&project);
    let path = project.join("project-config.json");
    let before = std::fs::read(&path).unwrap();
    let base = ["project", "captions", "style", project.to_str().unwrap()];
    let translation = [
        "--translation-font",
        "Inter",
        "--translation-size",
        "24",
        "--translation-gap",
        "10",
        "--bottom-margin",
        "60",
        "--json",
    ];
    let preview = cap()
        .args(base)
        .args(translation)
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(preview.status.success(), "{}", stderr(&preview));
    let preview = parse_json(&preview);
    assert_eq!(preview["after"]["font"], "Noto Sans SC");
    assert_eq!(preview["after"]["translationGap"], 10);
    assert_eq!(preview["after"]["bottomMargin"], 60);
    assert_eq!(preview["after"]["size"], 32);
    assert_eq!(
        preview["after"]["translationStyle"],
        serde_json::json!({"font":"Inter", "size":24})
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let apply = cap()
        .args(base)
        .args(translation)
        .args(["--expected-revision", preview["revision"].as_str().unwrap()])
        .output()
        .unwrap();
    assert!(apply.status.success(), "{}", stderr(&apply));
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["captions"]["settings"]["translationStyle"]["futureTypography"] = "keep".into();
    let content = stored["captions"]["segments"].clone();
    let translations = stored["captions"]["settings"]["translations"].clone();
    std::fs::write(&path, serde_json::to_vec_pretty(&stored).unwrap()).unwrap();

    for flags in [
        vec!["--font", "Noto Serif SC", "--size", "48"],
        vec!["--translation-size", "40"],
        vec!["--translation-font", "Manrope"],
    ] {
        let output = cap()
            .args(base)
            .args(&flags)
            .arg("--json")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        stored = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["captions"]["settings"]["font"], "Noto Serif SC");
        assert_eq!(stored["captions"]["settings"]["size"], 48);
        assert_eq!(
            stored["captions"]["settings"]["translationStyle"]["futureTypography"],
            "keep"
        );
        assert_eq!(stored["captions"]["segments"], content);
        assert_eq!(stored["captions"]["settings"]["translations"], translations);
        assert_eq!(stored["futureEditorField"]["preserve"], true);
        assert_eq!(
            stored["captions"]["settings"]["futureCaptionField"],
            "preserve"
        );
        if flags[0] == "--font" {
            assert_eq!(
                stored["captions"]["settings"]["translationStyle"]["font"],
                "Inter"
            );
            assert_eq!(
                stored["captions"]["settings"]["translationStyle"]["size"],
                24
            );
        }
    }
    assert_eq!(
        stored["captions"]["settings"]["translationStyle"]["font"],
        "Manrope"
    );
    assert_eq!(
        stored["captions"]["settings"]["translationStyle"]["size"],
        40
    );
}

#[test]
fn captions_style_typography_source_edit_freezes_legacy_translation_size() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("captioned.cap");
    write_caption_project(&project);
    let output = run(&[
        "project",
        "captions",
        "style",
        project.to_str().unwrap(),
        "--font",
        "Noto Serif SC",
        "--size",
        "48",
        "--json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let after = &parse_json(&output)["after"];
    assert_eq!(after["font"], "Noto Serif SC");
    assert_eq!(after["size"], 48);
    assert_eq!(
        after["translationStyle"],
        serde_json::json!({"font":"Noto Sans SC", "size":32})
    );
}

#[test]
fn captions_export_previews_bilingual_srt_vtt_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("captioned.cap");
    write_caption_project(&project);
    let media = project.join("content/display.mp4");
    write_test_media(&media);
    let source_before = std::fs::read(&media).unwrap();
    let config_before = std::fs::read(project.join("project-config.json")).unwrap();
    for format in ["srt", "vtt"] {
        let output_path = dir.path().join(format!("bilingual.{format}"));
        let args = [
            "project",
            "captions",
            "export",
            project.to_str().unwrap(),
            "--subtitle-format",
            format,
            "--display-mode",
            "bilingual",
            "--output",
            output_path.to_str().unwrap(),
            "--json",
        ];
        let preview = cap().args(args).arg("--dry-run").output().unwrap();
        assert!(preview.status.success(), "{}", stderr(&preview));
        assert!(!output_path.exists());
        let preview = parse_json(&preview);
        assert_eq!(preview["cueCount"], 1);
        assert!(
            preview["content"]
                .as_str()
                .unwrap()
                .contains("开始录制\nStart recording")
        );
        let output = run(&args);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(
            std::fs::read_to_string(&output_path).unwrap(),
            preview["content"]
        );
        let duplicate = run(&args);
        assert_eq!(duplicate.status.code(), Some(1));
        assert_eq!(parse_json(&duplicate)["code"], "OUTPUT_EXISTS");
        assert_eq!(
            std::fs::read_to_string(&output_path).unwrap(),
            preview["content"]
        );
    }
    assert_eq!(std::fs::read(&media).unwrap(), source_before);
    assert_eq!(
        std::fs::read(project.join("project-config.json")).unwrap(),
        config_before
    );
}

#[test]
fn captions_style_rejects_invalid_values_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("captioned.cap");
    write_caption_project(&project);
    let before = std::fs::read(project.join("project-config.json")).unwrap();
    for (flag, value) in [
        ("--size", "0"),
        ("--translation-size", "0"),
        ("--translation-size", "101"),
        ("--translation-gap", "201"),
        ("--bottom-margin", "401"),
        ("--translation-font", " "),
        ("--outline-width", "NaN"),
        ("--shadow-size", "31"),
        ("--color", "white"),
        ("--font", " "),
    ] {
        let output = run(&[
            "project",
            "captions",
            "style",
            project.to_str().unwrap(),
            flag,
            value,
            "--json",
        ]);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(parse_json(&output)["code"], "INVALID_ARGUMENT");
    }
    assert_eq!(
        std::fs::read(project.join("project-config.json")).unwrap(),
        before
    );
}

#[test]
fn captions_translation_modes_reject_stale_translations_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("captioned.cap");
    write_caption_project(&project);
    write_test_media(&project.join("content/display.mp4"));
    let path = project.join("project-config.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["captions"]["settings"]["translations"]["intro"]["sourceText"] = "过期文本".into();
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let output_path = dir.path().join("bilingual.srt");
    for command in ["style", "export"] {
        let mut args = vec![
            "project",
            "captions",
            command,
            project.to_str().unwrap(),
            "--display-mode",
            "bilingual",
            "--json",
        ];
        if command == "export" {
            args.extend(["--output", output_path.to_str().unwrap()]);
        }
        let output = run(&args);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert_eq!(parse_json(&output)["code"], "NO_TRANSLATIONS");
    }
    assert!(!output_path.exists());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    config["captions"]["segments"] = serde_json::json!([]);
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let output = run(&[
        "project",
        "captions",
        "export",
        project.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(parse_json(&output)["code"], "NO_CAPTIONS");
    assert!(!output_path.exists());
}
