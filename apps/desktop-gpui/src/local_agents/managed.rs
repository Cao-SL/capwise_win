use super::{AgentConnection, AgentPreset};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::{i18n::tr, store};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use sha2::{Digest as _, Sha256};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::{path::Path, process::Stdio, time::Duration};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use tokio::{io::AsyncWriteExt as _, process::Command};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const NODE_RELEASE: &str = "node-v24.21.0-darwin-arm64";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const NODE_URL: &str = "https://nodejs.org/dist/v24.21.0/node-v24.21.0-darwin-arm64.tar.gz";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const NODE_SHA256: &str = "bed7eea5325e1108f32ce5228ddd6a5f0f08a499ee42aa7442aea583702f6057";

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn package(id: &str) -> Option<(&'static str, &'static str)> {
    match id {
        "codex" => Some(("@agentclientprotocol/codex-acp", "2.6.0")),
        "claude" => Some(("@agentclientprotocol/claude-agent-acp", "0.101.0")),
        _ => None,
    }
}

pub(super) fn available(preset: &AgentPreset) -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return package(preset.id).is_some();
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = preset;
        false
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn node_binary(root: &Path) -> std::path::PathBuf {
    root.join(NODE_RELEASE).join("bin/node")
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn cached_in(root: &Path, preset: &AgentPreset) -> Option<AgentConnection> {
    let (name, version) = package(preset.id)?;
    let node = node_binary(root);
    let entry = root
        .join(format!("{}-{version}", preset.id))
        .join("node_modules")
        .join(name)
        .join("dist/index.js");
    (super::executable(&node) && entry.is_file()).then(|| {
        let mut connection = preset.connection(node);
        connection
            .args
            .insert(0, entry.to_string_lossy().into_owned());
        connection
    })
}

pub(super) fn cached(preset: &AgentPreset) -> Option<AgentConnection> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return cached_in(&store::app_data_dir().join("acp-runtime"), preset);
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = preset;
        None
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
async fn install_node(root: &Path) -> anyhow::Result<()> {
    if super::executable(&node_binary(root)) {
        return Ok(());
    }
    let stage = tempfile::tempdir_in(root)?;
    let archive = stage.path().join("node.tar.gz");
    let mut file = tokio::fs::File::create(&archive).await?;
    let mut response = reqwest::Client::new()
        .get(NODE_URL)
        .send()
        .await?
        .error_for_status()?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    while let Some(chunk) = response.chunk().await? {
        bytes += chunk.len();
        anyhow::ensure!(bytes <= 80 * 1024 * 1024, "Node archive is too large");
        hash.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    drop(file);
    anyhow::ensure!(
        format!("{:x}", hash.finalize()) == NODE_SHA256,
        "Node archive checksum mismatch"
    );
    let status = Command::new("/usr/bin/tar")
        .args(["-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(stage.path())
        .kill_on_drop(true)
        .status()
        .await?;
    anyhow::ensure!(
        status.success() && super::executable(&node_binary(stage.path())),
        "Node extraction failed"
    );
    tokio::fs::rename(stage.path().join(NODE_RELEASE), root.join(NODE_RELEASE)).await?;
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
async fn install_in(root: &Path, preset: &AgentPreset) -> anyhow::Result<AgentConnection> {
    static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _install = INSTALL.lock().await;
    let (name, version) =
        package(preset.id).ok_or_else(|| anyhow::anyhow!("No managed adapter"))?;
    if let Some(connection) = cached_in(root, preset) {
        return Ok(connection);
    }
    tokio::fs::create_dir_all(root).await?;
    install_node(root).await?;
    let stage = tempfile::tempdir_in(root)?;
    let prefix = stage.path().join(format!("{}-{version}", preset.id));
    tokio::fs::create_dir(&prefix).await?;
    let status = Command::new(node_binary(root))
        .arg(
            root.join(NODE_RELEASE)
                .join("lib/node_modules/npm/bin/npm-cli.js"),
        )
        .args(["install", "--prefix"])
        .arg(&prefix)
        .args([
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--registry=https://registry.npmjs.org",
        ])
        .arg("--userconfig")
        .arg(stage.path().join("user.npmrc"))
        .arg("--globalconfig")
        .arg(stage.path().join("global.npmrc"))
        .arg("--cache")
        .arg(stage.path().join("npm-cache"))
        .arg(format!("{name}@{version}"))
        .current_dir(stage.path())
        .env_clear()
        .env("HOME", stage.path())
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await?;
    anyhow::ensure!(status.success(), "Adapter package installation failed");
    anyhow::ensure!(
        prefix
            .join("node_modules")
            .join(name)
            .join("dist/index.js")
            .is_file(),
        "Adapter package has no ACP entry point"
    );
    tokio::fs::rename(&prefix, root.join(format!("{}-{version}", preset.id))).await?;
    cached_in(root, preset)
        .ok_or_else(|| anyhow::anyhow!("Adapter is unavailable after installation"))
}

pub(super) async fn install(preset: &AgentPreset) -> Result<AgentConnection, String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let root = store::app_data_dir().join("acp-runtime");
        return match tokio::time::timeout(Duration::from_secs(240), install_in(&root, preset)).await
        {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(error)) => {
                tracing::warn!(agent = preset.id, %error, "Could not prepare ACP adapter");
                Err(tr::settings::agents::prepare_failed().into())
            }
            Err(_) => Err(tr::settings::agents::timeout().into()),
        };
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = preset;
        Err(crate::i18n::tr::settings::agents::not_found().into())
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "downloads real ACP packages from their public sources"]
    async fn installs_and_initializes_without_system_node() {
        let root = tempfile::tempdir().unwrap();
        for id in ["codex", "claude"] {
            let preset = super::super::PRESETS.iter().find(|p| p.id == id).unwrap();
            let connection = install_in(root.path(), preset).await.unwrap();
            assert_eq!(Path::new(&connection.command), node_binary(root.path()));
            assert_eq!(
                cached_in(root.path(), preset).unwrap().args,
                connection.args
            );
            let mut process = super::super::AcpProcess::launch(&connection).unwrap();
            let reply = tokio::time::timeout(
                Duration::from_secs(30),
                process.request(
                    "initialize",
                    serde_json::json!({
                        "protocolVersion": 1,
                        "clientCapabilities": {},
                        "clientInfo": {"name":"capwise-test","version":"0.0.0"}
                    }),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(reply["protocolVersion"], 1);
        }
    }
}
