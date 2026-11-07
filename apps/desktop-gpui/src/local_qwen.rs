//! Qwen3-ASR 0.6B on this Mac: the GGUF model, downloaded on demand, run by
//! the bundled llama.cpp `llama-server`, which hears audio through its
//! OpenAI-style chat endpoint. The server is started for one transcription
//! and stopped when it finishes, so the model holds no memory between runs.

use std::{
    io::Write as _,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use cap_project::CaptionSegment;
use futures_util::StreamExt as _;
use sha2::Digest as _;

use crate::{i18n::tr, transcription};

pub const MODEL: &str = "qwen3-asr";

const REPO: &str = "https://huggingface.co/ggml-org/Qwen3-ASR-0.6B-GGUF/resolve/928ab958557df9aa2ef1c93e0e83c7ad0933fae2";

struct ModelFile {
    name: &'static str,
    bytes: u64,
    sha256: &'static str,
}

const FILES: [ModelFile; 2] = [
    ModelFile {
        name: "Qwen3-ASR-0.6B-Q8_0.gguf",
        bytes: 804_749_248,
        sha256: "bca259818b50ca7c4c05e9bdb35a5dc04fa039653a6d6f3f0f331f96f6aa1971",
    },
    ModelFile {
        name: "mmproj-Qwen3-ASR-0.6B-Q8_0.gguf",
        bytes: 214_392_480,
        sha256: "41a342b5e4c514e968cb756de6cd1b7be39eff43c44c57a2ef5fc6522e36603d",
    },
];

const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// The bundled build is Apple-silicon only.
pub fn supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

pub fn dir() -> PathBuf {
    transcription::models_dir().join("qwen3-asr-0.6b")
}

/// Both files present at their full size (their checksums were verified when
/// they were downloaded).
pub fn installed() -> bool {
    FILES.iter().all(|file| {
        std::fs::metadata(dir().join(file.name)).is_ok_and(|meta| meta.len() == file.bytes)
    })
}

pub fn delete() -> Result<(), String> {
    std::fs::remove_dir_all(dir()).map_err(|error| format!("Failed to delete the model: {error}"))
}

/// Downloads both files with one progress bar, each verified before it is
/// moved into place.
pub async fn download() -> Result<(), String> {
    let dir = dir();
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create {}: {error}", dir.display()))?;
    let total: u64 = FILES.iter().map(|file| file.bytes).sum();
    let mut done: u64 = 0;
    let client = reqwest::Client::new();
    for file in &FILES {
        let path = dir.join(file.name);
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() == file.bytes) {
            done += file.bytes;
            continue;
        }
        let staging = path.with_extension("gguf.downloading");
        let result = download_file(&client, file, &staging, done, total).await;
        if let Err(error) = result {
            let _ = std::fs::remove_file(&staging);
            return Err(error);
        }
        std::fs::rename(&staging, &path)
            .map_err(|error| format!("Failed to install the model: {error}"))?;
        done += file.bytes;
    }
    Ok(())
}

async fn download_file(
    client: &reqwest::Client,
    file: &ModelFile,
    staging: &std::path::Path,
    before: u64,
    total: u64,
) -> Result<(), String> {
    let response = client
        .get(format!("{REPO}/{}", file.name))
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|error| format!("Failed to download the model: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Failed to download the model: HTTP {}",
            response.status()
        ));
    }
    let mut out = std::fs::File::create(staging)
        .map_err(|error| format!("Failed to create file: {error}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut written: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Error while downloading: {error}"))?;
        out.write_all(&chunk)
            .map_err(|error| format!("Error while writing to file: {error}"))?;
        hasher.update(&chunk);
        written += chunk.len() as u64;
        let progress = (before + written) as f64 / total as f64 * 100.0;
        transcription::set_download_progress(
            MODEL,
            progress,
            tr::transcription::download::progress(format!("{progress:.0}")),
        );
    }
    out.flush()
        .map_err(|error| format!("Failed to flush file: {error}"))?;
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != file.sha256 {
        return Err(format!(
            "The downloaded {} is damaged; try again.",
            file.name
        ));
    }
    Ok(())
}

pub(crate) fn runtime_available() -> bool {
    server_binary().is_some_and(|path| path.is_file())
}

/// `CAPWISE_LLAMA_SERVER`, or `Contents/Helpers/llama-server/llama-server`
/// next to the running executable.
fn server_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CAPWISE_LLAMA_SERVER") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let path = exe
        .parent()?
        .parent()?
        .join("Helpers")
        .join("llama-server")
        .join("llama-server");
    path.is_file().then_some(path)
}

/// A running `llama-server`, stopped when dropped.
struct Server {
    child: Child,
    base_url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        stop_group(&mut self.child);
    }
}

/// `Drop` never runs when Capwise crashes or is force-quit, and macOS has no
/// parent-death signal, so `llama-server` runs under a shell that kills it
/// within a second of Capwise going away. Without this an orphaned server
/// kept about 1 GB of model in memory until the next transcription.
#[cfg(unix)]
const WATCHDOG: &str = r#"parent=$1; shift; "$@" & server=$!; while kill -0 "$parent" 2>/dev/null && kill -0 "$server" 2>/dev/null; do sleep 1; done; kill "$server" 2>/dev/null; wait "$server""#;

#[cfg(unix)]
fn watched_command(parent: u32, program: &std::path::Path) -> Command {
    use std::os::unix::process::CommandExt as _;
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(WATCHDOG)
        .arg("capwise-llama-watchdog")
        .arg(parent.to_string())
        .arg(program)
        .process_group(0);
    command
}

#[cfg(not(unix))]
fn watched_command(_parent: u32, program: &std::path::Path) -> Command {
    Command::new(program)
}

#[cfg(unix)]
fn stop_group(child: &mut Child) {
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `killpg` only sends a signal; the group is the watchdog's own.
        unsafe {
            libc::killpg(group, libc::SIGTERM);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn stop_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|error| format!("No free local port: {error}"))
}

fn start_server() -> Result<Server, String> {
    let binary =
        server_binary().ok_or_else(|| tr::transcription::qwen_missing_server().to_string())?;
    let port = free_port()?;
    let dir = dir();
    let child = watched_command(std::process::id(), &binary)
        .arg("-m")
        .arg(dir.join(FILES[0].name))
        .arg("--mmproj")
        .arg(dir.join(FILES[1].name))
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args([
            "-ngl",
            "99",
            "-np",
            "3",
            "-c",
            "12288",
            "--alias",
            MODEL,
            "--log-disable",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Failed to start llama-server: {error}"))?;
    let mut server = Server {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
    };

    let health = format!("{}/health", server.base_url);
    let started = Instant::now();
    let client = reqwest::Client::new();
    loop {
        if let Ok(Some(status)) = server.child.try_wait() {
            return Err(format!(
                "llama-server exited while loading the model ({status})"
            ));
        }
        let ready = tokio::runtime::Handle::current().block_on(async {
            client
                .get(&health)
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
        });
        if ready {
            tracing::info!(
                ms = started.elapsed().as_millis() as u64,
                "llama-server ready"
            );
            return Ok(server);
        }
        if started.elapsed() > STARTUP_TIMEOUT {
            return Err("llama-server did not become ready in time".to_string());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Recognise 16 kHz mono audio with Qwen3-ASR on this Mac. Runs on a tokio
/// blocking thread, like the cloud services.
pub fn transcribe(samples: &[f32], hints: &[String]) -> Result<Vec<CaptionSegment>, String> {
    if !installed() {
        return Err(tr::transcription::qwen_not_installed().to_string());
    }
    let server = start_server()?;
    crate::asr_cloud::transcribe_llama(&format!("{}/v1", server.base_url), MODEL, samples, hints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[cfg(unix)]
    fn grandchild(watchdog: &Child) -> u32 {
        let started = Instant::now();
        loop {
            let output = Command::new("/usr/bin/pgrep")
                .args(["-P", &watchdog.id().to_string(), "sleep"])
                .output()
                .unwrap();
            if let Some(pid) = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .and_then(|line| line.trim().parse().ok())
            {
                return pid;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    #[test]
    fn stopping_the_server_kills_the_process_it_runs() {
        let mut watchdog = watched_command(std::process::id(), std::path::Path::new("/bin/sleep"))
            .arg("30")
            .spawn()
            .unwrap();
        let server = grandchild(&watchdog);
        stop_group(&mut watchdog);
        let started = Instant::now();
        while alive(server) && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(server));
    }

    #[cfg(unix)]
    #[test]
    fn the_server_exits_when_capwise_goes_away_without_dropping_it() {
        let mut parent = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let mut watchdog = watched_command(parent.id(), std::path::Path::new("/bin/sleep"))
            .arg("30")
            .spawn()
            .unwrap();
        let server = grandchild(&watchdog);
        parent.kill().unwrap();
        parent.wait().unwrap();
        let started = Instant::now();
        while alive(server) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!alive(server));
        watchdog.wait().unwrap();
    }

    /// Needs the downloaded model and `CAPWISE_LLAMA_SERVER` (or an installed
    /// app), plus `CAPWISE_QWEN_TEST_WAV`: a 16 kHz mono WAV of Chinese speech.
    #[test]
    #[ignore]
    fn transcribes_chinese_speech_with_context() {
        let wav = std::env::var("CAPWISE_QWEN_TEST_WAV").expect("CAPWISE_QWEN_TEST_WAV");
        let bytes = std::fs::read(wav).unwrap();
        let samples: Vec<f32> = bytes[44..]
            .chunks_exact(2)
            .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0)
            .collect();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let hints = vec!["灵录".to_string(), "通义千问".to_string()];
        let segments = runtime
            .block_on(async move {
                tokio::task::spawn_blocking(move || transcribe(&samples, &hints)).await
            })
            .unwrap()
            .unwrap();
        let text: String = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect();
        eprintln!("{segments:#?}");
        assert!(text.contains("灵录"), "{text}");
        assert!(text.contains("通义千问"), "{text}");
        assert!(
            segments
                .windows(2)
                .all(|pair| pair[0].end <= pair[1].start + 0.01)
        );
    }

    /// Downloads whatever is missing from Hugging Face (network, ~1 GB at
    /// most) and checks it against the pinned checksums.
    #[test]
    #[ignore]
    fn download_fetches_missing_files_and_verifies_them() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(download()).unwrap();
        assert!(installed());
    }
}
