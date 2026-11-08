use crate::{i18n::tr, store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AgentModel {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AgentConnection {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub models: Vec<AgentModel>,
    #[serde(default)]
    pub model: String,
}

pub(crate) struct AgentPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub commands: &'static [&'static str],
    pub args: &'static [&'static str],
}

pub(crate) const PRESETS: &[AgentPreset] = &[
    AgentPreset {
        id: "claude",
        name: "Claude Code",
        commands: &["claude-agent-acp"],
        args: &[],
    },
    AgentPreset {
        id: "codex",
        name: "Codex",
        commands: &["codex-acp"],
        args: &[],
    },
    AgentPreset {
        id: "gemini",
        name: "Gemini CLI",
        commands: &["gemini"],
        args: &["--acp", "--approval-mode", "plan"],
    },
    AgentPreset {
        id: "opencode",
        name: "OpenCode",
        commands: &["opencode"],
        args: &["acp"],
    },
    AgentPreset {
        id: "qwen",
        name: "Qwen Code",
        commands: &["qwen"],
        args: &["--acp", "--approval-mode", "plan"],
    },
    AgentPreset {
        id: "kimi",
        name: "Kimi CLI",
        commands: &["kimi", "kimi-cli"],
        args: &["acp"],
    },
    AgentPreset {
        id: "cursor",
        name: "Cursor",
        commands: &["cursor-agent"],
        args: &["acp"],
    },
    AgentPreset {
        id: "cline",
        name: "Cline",
        commands: &["cline"],
        args: &["--acp"],
    },
    AgentPreset {
        id: "auggie",
        name: "Auggie",
        commands: &["auggie"],
        args: &["--acp"],
    },
    AgentPreset {
        id: "grok",
        name: "Grok",
        commands: &["grok"],
        args: &["--no-auto-update", "agent", "stdio"],
    },
    AgentPreset {
        id: "hermes",
        name: "Hermes",
        commands: &["hermes"],
        args: &["acp"],
    },
    AgentPreset {
        id: "openclaw",
        name: "OpenClaw",
        commands: &["openclaw"],
        args: &["acp"],
    },
];

pub(crate) fn connections() -> Vec<AgentConnection> {
    store::store_section("capwise_agents")
        .get("connections")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub(crate) fn save(connection: AgentConnection) -> Result<(), String> {
    let mut saved = connections();
    saved.retain(|old| old.id != connection.id);
    saved.push(connection);
    persist(&saved)
}

pub(crate) fn remove(id: &str) -> Result<(), String> {
    let mut saved = connections();
    saved.retain(|old| old.id != id);
    persist(&saved)
}

fn persist(saved: &[AgentConnection]) -> Result<(), String> {
    let value =
        serde_json::to_value(saved).map_err(|_| tr::settings::agents::save_failed().to_owned())?;
    if store::set_store_setting("capwise_agents", "connections", value) {
        Ok(())
    } else {
        Err(tr::settings::agents::save_failed().into())
    }
}

fn search_paths() -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    paths.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.extend(
            [
                ".local/bin",
                ".opencode/bin",
                ".kimi-code/bin",
                ".grok/bin",
                ".cargo/bin",
            ]
            .map(|p| home.join(p)),
        );
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut versions: Vec<_> = entries.flatten().map(|e| e.path().join("bin")).collect();
            versions.sort();
            versions.reverse();
            paths.extend(versions);
        }
    }
    paths.retain(|p| p.is_absolute());
    paths
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn detect(preset: &AgentPreset) -> Option<AgentConnection> {
    let path = search_paths()
        .into_iter()
        .flat_map(|path| {
            preset
                .commands
                .iter()
                .map(move |command| path.join(command))
        })
        .find(|path| executable(path))?;
    Some(AgentConnection {
        id: preset.id.into(),
        name: preset.name.into(),
        command: path.to_string_lossy().into_owned(),
        args: preset.args.iter().map(|v| (*v).to_owned()).collect(),
        models: vec![],
        model: String::new(),
    })
}

struct AcpProcess {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    workspace: tempfile::TempDir,
    next_id: u64,
    session: String,
    response: String,
    model_config: Option<String>,
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}

impl AcpProcess {
    fn launch(connection: &AgentConnection) -> Result<Self, String> {
        let command_path = Path::new(&connection.command);
        if !command_path.is_absolute()
            || !executable(command_path)
            || connection.args.len() > 64
            || connection
                .args
                .iter()
                .any(|a| a.len() > 4096 || a.contains('\0'))
        {
            return Err(tr::settings::agents::invalid_command().into());
        }
        let workspace =
            tempfile::tempdir().map_err(|_| tr::settings::agents::launch_failed().to_owned())?;
        let mut command = Command::new(command_path);
        command
            .args(&connection.args)
            .current_dir(workspace.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Ok(paths) = std::env::join_paths(search_paths()) {
            command.env("PATH", paths);
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| tr::settings::agents::launch_failed().to_owned())?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| tr::settings::agents::launch_failed().to_owned())?;
        let output = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| tr::settings::agents::launch_failed().to_owned())?,
        );
        Ok(Self {
            child,
            input,
            output,
            workspace,
            next_id: 0,
            session: String::new(),
            response: String::new(),
            model_config: None,
        })
    }

    async fn write(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value)
            .map_err(|_| tr::settings::agents::protocol_error().to_owned())?;
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|_| tr::settings::agents::disconnected().to_owned())?;
        self.input
            .flush()
            .await
            .map_err(|_| tr::settings::agents::disconnected().to_owned())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        let mut packet_count = 0;
        loop {
            let mut line = Vec::new();
            let count = (&mut self.output)
                .take(2_097_153)
                .read_until(b'\n', &mut line)
                .await
                .map_err(|_| tr::settings::agents::disconnected().to_owned())?;
            if count == 0 {
                return Err(tr::settings::agents::disconnected().into());
            }
            packet_count += 1;
            if count > 2_097_152 || packet_count > 20_000 {
                return Err(tr::settings::agents::protocol_error().into());
            }
            let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                if let Some(request_id) = message.get("id") {
                    if method == "session/request_permission" {
                        self.write(json!({"jsonrpc":"2.0","id":request_id,"result":{"outcome":{"outcome":"cancelled"}}})).await?;
                    } else {
                        self.write(json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"Unsupported client operation"}})).await?;
                    }
                } else if method == "session/update" {
                    let params = &message["params"];
                    if params["sessionId"].as_str() == Some(self.session.as_str())
                        && params["update"]["sessionUpdate"] == "agent_message_chunk"
                        && params["update"]["content"]["type"] == "text"
                        && let Some(text) = params["update"]["content"]["text"].as_str()
                    {
                        if self.response.len() + text.len() > 262_144 {
                            return Err(tr::settings::agents::protocol_error().into());
                        }
                        self.response.push_str(text);
                    }
                }
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(if error["code"] == -32000 || error["code"] == -32001 {
                    tr::settings::agents::login_required()
                } else {
                    tr::settings::agents::protocol_error()
                }
                .into());
            }
            return message
                .get("result")
                .cloned()
                .ok_or_else(|| tr::settings::agents::protocol_error().into());
        }
    }

    async fn initialize(
        &mut self,
        connection: &AgentConnection,
    ) -> Result<Vec<AgentModel>, String> {
        let initialized = self.request("initialize", json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},"clientInfo":{"name":"capwise","title":"Capwise","version":"0.1.0"}})).await?;
        if initialized["protocolVersion"] != 1 {
            return Err(tr::settings::agents::protocol_error().into());
        }
        if connection.id == "grok" {
            let cached = initialized["authMethods"]
                .as_array()
                .is_some_and(|methods| methods.iter().any(|m| m["id"] == "cached_token"));
            if cached {
                self.request(
                    "authenticate",
                    json!({"methodId":"cached_token","_meta":{"headless":true}}),
                )
                .await?;
            }
        }
        let session = self
            .request(
                "session/new",
                json!({"cwd":self.workspace.path(),"mcpServers":[]}),
            )
            .await?;
        self.session = session["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| tr::settings::agents::protocol_error().to_owned())?
            .into();
        if session["modes"]["availableModes"]
            .as_array()
            .is_some_and(|modes| modes.iter().any(|m| m["id"] == "plan"))
        {
            self.request(
                "session/set_mode",
                json!({"sessionId":self.session,"modeId":"plan"}),
            )
            .await?;
        }
        let (config, models) = models_from_session(&session);
        self.model_config = config;
        Ok(models)
    }
}

fn models_from_session(session: &Value) -> (Option<String>, Vec<AgentModel>) {
    if let Some(config) = session["configOptions"].as_array().and_then(|options| {
        options
            .iter()
            .find(|option| option["category"] == "model" || option["id"] == "model")
    }) {
        let options = config["options"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|v| {
                v["options"]
                    .as_array()
                    .map(|group| group.iter().collect::<Vec<_>>())
                    .unwrap_or_else(|| vec![v])
            });
        let models = options
            .filter_map(|v| {
                Some(AgentModel {
                    id: v["value"].as_str()?.into(),
                    name: v["name"].as_str().or_else(|| v["value"].as_str())?.into(),
                })
            })
            .take(1024)
            .collect();
        (config["id"].as_str().map(str::to_owned), models)
    } else {
        (
            None,
            session["models"]["availableModels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| {
                    Some(AgentModel {
                        id: v["modelId"].as_str()?.into(),
                        name: v["name"].as_str().or_else(|| v["modelId"].as_str())?.into(),
                    })
                })
                .take(1024)
                .collect(),
        )
    }
}

pub(crate) async fn probe(connection: &AgentConnection) -> Result<Vec<AgentModel>, String> {
    tokio::time::timeout(Duration::from_secs(35), async {
        let mut process = AcpProcess::launch(connection)?;
        process.initialize(connection).await
    })
    .await
    .map_err(|_| tr::settings::agents::timeout().to_owned())?
}

pub(crate) async fn chat(
    connection: &AgentConnection,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    tokio::time::timeout(Duration::from_secs(180), async {
        let mut process = AcpProcess::launch(connection)?;
        let models = process.initialize(connection).await?;
        if !model.is_empty() {
            if !models.iter().any(|m| m.id == model) { return Err(tr::ai::model_missing().to_owned()); }
            if let Some(config) = &process.model_config {
                process.request("session/set_config_option", json!({"sessionId":process.session,"configId":config,"value":model})).await?;
            } else {
                process.request("session/set_model", json!({"sessionId":process.session,"modelId":model})).await?;
            }
        }
        let result = process.request("session/prompt", json!({"sessionId":process.session,"prompt":[{"type":"text","text":format!("{system}\n\n{user}") }]})).await?;
        if result["stopReason"] == "cancelled" { return Err(tr::settings::agents::cancelled().into()); }
        if process.response.trim().is_empty() { return Err(tr::settings::agents::empty_reply().into()); }
        Ok(std::mem::take(&mut process.response))
    }).await.map_err(|_| tr::settings::agents::timeout().to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn mock_agent(body: &str) -> (tempfile::TempDir, AgentConnection) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("agent.py");
        std::fs::write(&script, body).unwrap();
        let connection = AgentConnection {
            id: "test".into(),
            name: "Test agent".into(),
            command: "/usr/bin/python3".into(),
            args: vec![script.to_string_lossy().into_owned()],
            models: vec![],
            model: String::new(),
        };
        (dir, connection)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acp_roundtrip_negotiates_models_denies_tools_and_collects_only_own_session() {
        let (_dir, connection) = mock_agent(
            r#"
import json,sys
for line in sys.stdin:
    m=json.loads(line); method=m.get('method'); p=m.get('params',{}); result={}
    if method=='initialize':
        assert not p['clientCapabilities']['terminal']
        assert not p['clientCapabilities']['fs']['writeTextFile']
        result={'protocolVersion':1}
    elif method=='session/new':
        assert p['mcpServers']==[]
        result={'sessionId':'s','configOptions':[{'id':'model','category':'model','options':[{'value':'a','name':'A'}]}]}
    elif method=='session/set_config_option':
        assert p['value']=='a'
    elif method=='session/prompt':
        print(json.dumps({'jsonrpc':'2.0','id':'permission','method':'session/request_permission','params':{}}),flush=True)
        answer=json.loads(sys.stdin.readline())
        assert answer['result']['outcome']['outcome']=='cancelled'
        print(json.dumps({'jsonrpc':'2.0','id':'file','method':'fs/read_text_file','params':{}}),flush=True)
        answer=json.loads(sys.stdin.readline())
        assert answer['error']['code']==-32601
        for session,text in [('wrong','ignore'),('s','hello'),('s',' world')]:
            print(json.dumps({'method':'session/update','params':{'sessionId':session,'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}}}}),flush=True)
        result={'stopReason':'end_turn'}
    print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':result}),flush=True)
"#,
        );
        assert_eq!(probe(&connection).await.unwrap()[0].id, "a");
        assert_eq!(
            chat(&connection, "a", "system", "user").await.unwrap(),
            "hello world"
        );
        assert!(
            chat(&connection, "missing", "system", "user")
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_acp_session_stops_owned_child() {
        let (_dir, connection) = mock_agent("import time; time.sleep(60)");
        let process = AcpProcess::launch(&connection).unwrap();
        let pid = process.child.id().unwrap() as i32;
        drop(process);
        for _ in 0..50 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("ACP child survived cancellation");
    }

    #[tokio::test]
    #[ignore]
    async fn installed_adapter_probe() {
        let id = std::env::var("CAPWISE_TEST_AGENT").unwrap();
        let preset = PRESETS.iter().find(|p| p.id == id).unwrap();
        let agent = detect(preset).expect("adapter must be installed");
        let models = probe(&agent).await.unwrap();
        eprintln!("ACP probe {}: {} models", agent.name, models.len());
    }

    #[test]
    fn reads_grouped_config_models_and_legacy_models() {
        let (config, models) = models_from_session(
            &json!({"configOptions":[{"id":"pick-model","category":"model","options":[{"group":"A","options":[{"value":"a","name":"Model A"}]},{"value":"b","name":"Model B"}]}]}),
        );
        assert_eq!(config.as_deref(), Some("pick-model"));
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        let (_, models) = models_from_session(
            &json!({"models":{"availableModels":[{"modelId":"legacy","name":"Legacy"}]}}),
        );
        assert_eq!(models[0].id, "legacy");
    }
}
