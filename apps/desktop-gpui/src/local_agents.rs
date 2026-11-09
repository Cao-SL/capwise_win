mod managed;

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

const STORE_SECTION: &str = "capwise_agents";

fn saved_connections(section: &serde_json::Map<String, Value>) -> Vec<AgentConnection> {
    section
        .get("connections")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

fn disconnected(section: &serde_json::Map<String, Value>) -> Vec<String> {
    section
        .get("disconnected")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

pub(crate) fn connections() -> Vec<AgentConnection> {
    saved_connections(&store::store_section(STORE_SECTION))
}

pub(crate) fn automatically_connectable(detected: &[AgentConnection]) -> Vec<AgentConnection> {
    let section = store::store_section(STORE_SECTION);
    let connected = saved_connections(&section);
    let blocked = disconnected(&section);
    detected
        .iter()
        .filter(|agent| {
            !blocked.contains(&agent.id) && !connected.iter().any(|saved| saved.id == agent.id)
        })
        .cloned()
        .collect()
}

impl AgentConnection {
    pub(crate) fn update_models(&mut self, mut models: Vec<AgentModel>) {
        models.retain(|model| !model.id.trim().is_empty());
        if !models.iter().any(|model| model.id == self.model) {
            self.model = models
                .first()
                .map_or_else(String::new, |model| model.id.clone());
        }
        self.models = models;
    }
}

pub(crate) fn save(connection: AgentConnection) -> Result<(), String> {
    save_connection(connection, false).map(|_| ())
}

pub(crate) fn save_detected(connection: AgentConnection) -> Result<bool, String> {
    save_connection(connection, true)
}

fn save_connection(connection: AgentConnection, automatic: bool) -> Result<bool, String> {
    let mut skipped = false;
    let saved = store::update_store_section(STORE_SECTION, |section| {
        let mut connections = saved_connections(section);
        let mut blocked = disconnected(section);
        if automatic
            && (blocked.contains(&connection.id)
                || connections.iter().any(|old| old.id == connection.id))
        {
            skipped = true;
            return false;
        }
        connections.retain(|old| old.id != connection.id);
        blocked.retain(|id| *id != connection.id);
        connections.push(connection);
        section.insert("connections".into(), json!(connections));
        section.insert("disconnected".into(), json!(blocked));
        true
    });
    if saved || skipped {
        Ok(saved)
    } else {
        Err(tr::settings::agents::save_failed().into())
    }
}

pub(crate) fn remove(id: &str) -> Result<(), String> {
    let saved = store::update_store_section(STORE_SECTION, |section| {
        let mut connections = saved_connections(section);
        let mut blocked = disconnected(section);
        connections.retain(|old| old.id != id);
        if !blocked.iter().any(|item| item == id) {
            blocked.push(id.into());
        }
        section.insert("connections".into(), json!(connections));
        section.insert("disconnected".into(), json!(blocked));
        true
    });
    if !saved {
        return Err(tr::settings::agents::save_failed().into());
    }
    crate::ai_store::clear_agent_default(&crate::llm::agent_content_id(id, ""));
    Ok(())
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
                ".bun/bin",
                ".npm-global/bin",
                ".volta/bin",
                ".local/share/mise/shims",
                ".local/share/pnpm",
                "Library/pnpm",
                ".opencode/bin",
                ".kimi-code/bin",
                ".grok/bin",
                ".cargo/bin",
            ]
            .map(|p| home.join(p)),
        );
        let nvm = std::env::var_os("NVM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".nvm"));
        let fnm = std::env::var_os("FNM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Library/Application Support/fnm"));
        for (versions, bin) in [
            (nvm.join("versions/node"), "bin"),
            (fnm.join("node-versions"), "installation/bin"),
            (
                home.join(".local/share/fnm/node-versions"),
                "installation/bin",
            ),
        ] {
            paths.extend(node_version_paths(&versions, bin));
        }
    }
    paths.extend(
        ["PNPM_HOME", "FNM_MULTISHELL_PATH"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from),
    );
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| p.is_absolute() && seen.insert(p.clone()));
    paths
}

fn node_version_paths(directory: &Path, bin: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut versions: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let version = entry.file_name().to_str()?.strip_prefix('v')?.to_owned();
            let mut components = version.split('.');
            let version = [
                components.next()?.parse::<u32>().ok()?,
                components.next()?.parse().ok()?,
                components.next()?.parse().ok()?,
            ];
            Some((version, entry.path().join(bin)))
        })
        .collect();
    versions.sort_unstable_by_key(|(version, _)| std::cmp::Reverse(*version));
    versions.into_iter().map(|(_, path)| path).collect()
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

impl AgentPreset {
    fn connection(&self, command: PathBuf) -> AgentConnection {
        AgentConnection {
            id: self.id.into(),
            name: self.name.into(),
            command: command.to_string_lossy().into_owned(),
            args: self.args.iter().map(|v| (*v).to_owned()).collect(),
            models: vec![],
            model: String::new(),
        }
    }

    pub(crate) fn pending_connection(&self) -> AgentConnection {
        self.connection(PathBuf::new())
    }

    pub(crate) fn can_prepare(&self) -> bool {
        managed::available(self)
    }
}

fn detect_in(preset: &AgentPreset, paths: &[PathBuf]) -> Option<AgentConnection> {
    paths
        .iter()
        .flat_map(|path| {
            preset
                .commands
                .iter()
                .map(move |command| path.join(command))
        })
        .find(|path| executable(path))
        .map(|path| preset.connection(path))
        .or_else(|| managed::cached(preset))
}

pub(crate) fn detect_all() -> Vec<AgentConnection> {
    let paths = search_paths();
    PRESETS
        .iter()
        .filter_map(|preset| detect_in(preset, &paths))
        .collect()
}

pub(crate) async fn prepare(id: &str) -> Result<AgentConnection, String> {
    let preset = PRESETS
        .iter()
        .find(|preset| preset.id == id)
        .ok_or_else(|| tr::settings::agents::not_found().to_owned())?;
    if let Some(connection) = detect_in(preset, &search_paths()) {
        return Ok(connection);
    }
    managed::install(preset).await
}

#[derive(Debug)]
enum AcpRequestError {
    LoginRequired,
    Other(String),
}

impl From<&str> for AcpRequestError {
    fn from(message: &str) -> Self {
        Self::Other(message.into())
    }
}

impl From<String> for AcpRequestError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<AcpRequestError> for String {
    fn from(error: AcpRequestError) -> Self {
        match error {
            AcpRequestError::LoginRequired => tr::settings::agents::login_required().into(),
            AcpRequestError::Other(message) => message,
        }
    }
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
        if let Ok(paths) = std::env::join_paths(
            command_path
                .parent()
                .map(Path::to_path_buf)
                .into_iter()
                .chain(search_paths()),
        ) {
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

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, AcpRequestError> {
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
                    AcpRequestError::LoginRequired
                } else {
                    tr::settings::agents::protocol_error().into()
                });
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
        allow_browser_login: bool,
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
        let request = json!({"cwd":self.workspace.path(),"mcpServers":[]});
        let session = match self.request("session/new", request.clone()).await {
            Err(AcpRequestError::LoginRequired)
                if allow_browser_login
                    && connection.id == "codex"
                    && initialized["authMethods"]
                        .as_array()
                        .is_some_and(|methods| {
                            methods.iter().any(|method| {
                                method["id"] == "chat-gpt"
                                    && method["type"].as_str().is_none_or(|kind| kind == "agent")
                            })
                        }) =>
            {
                self.request("authenticate", json!({"methodId":"chat-gpt"}))
                    .await?;
                self.request("session/new", request).await?
            }
            result => result?,
        };
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
    probe_with_login(connection, false).await
}

pub(crate) async fn connect(connection: &AgentConnection) -> Result<Vec<AgentModel>, String> {
    probe_with_login(connection, connection.id == "codex").await
}

async fn probe_with_login(
    connection: &AgentConnection,
    allow_browser_login: bool,
) -> Result<Vec<AgentModel>, String> {
    let duration = if allow_browser_login { 300 } else { 35 };
    tokio::time::timeout(Duration::from_secs(duration), async {
        let mut process = AcpProcess::launch(connection)?;
        process.initialize(connection, allow_browser_login).await
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
        let models = process.initialize(connection, false).await?;
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
    async fn content_model_selection_uses_the_connected_agent_and_named_model() {
        use crate::llm::{ContentModel, selected_content_model};
        use std::sync::Arc;

        let (_dir, connection) = mock_agent(
            r#"
import json,sys
for line in sys.stdin:
    m=json.loads(line); method=m['method']; p=m.get('params',{}); result={}
    if method=='initialize':
        result={'protocolVersion':1}
    elif method=='session/new':
        result={'sessionId':'s','configOptions':[{'id':'model','category':'model','options':[{'value':'a:b','name':'Named Model'}]}]}
    elif method=='session/set_config_option':
        assert p['value']=='a:b'
    elif method=='session/prompt':
        assert p['prompt'][0]['text']=='system\n\nuser'
        print(json.dumps({'method':'session/update','params':{'sessionId':'s','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'{"ok":true}'}}}}),flush=True)
        result={'stopReason':'end_turn'}
    print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':result}),flush=True)
"#,
        );
        let model = probe(&connection).await.unwrap().pop().unwrap();
        let source = ContentModel::Agent {
            connection: Arc::new(connection),
            model: Some(model),
        };
        assert!(selected_content_model([source.clone()], None).is_none());
        let selected = selected_content_model([source], Some("acp:test:a:b")).unwrap();
        assert_eq!(selected.model_name(), "Named Model");
        assert_eq!(
            selected
                .chat_json(uuid::Uuid::new_v4(), "system", "user")
                .await
                .unwrap(),
            r#"{"ok":true}"#,
        );
        let http = ContentModel::Provider(crate::llm::provider("deepseek").unwrap());
        assert!(selected_content_model([http], Some("acp:removed:")).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_browser_login_is_only_attempted_during_explicit_connection() {
        let (_dir, mut connection) = mock_agent(
            r#"
import json,sys
authenticated=False
for line in sys.stdin:
    m=json.loads(line)
    method=m['method']
    reply={'jsonrpc':'2.0','id':m['id']}
    if method=='initialize':
        reply['result']={'protocolVersion':1,'authMethods':[{'id':'chat-gpt','type':'agent'}]}
    elif method=='session/new':
        if authenticated:
            reply['result']={'sessionId':'s'}
        else:
            reply['error']={'code':-32000,'message':'Authentication required'}
    elif method=='authenticate':
        assert m['params']['methodId']=='chat-gpt'
        authenticated=True
        reply['result']={}
    print(json.dumps(reply),flush=True)
"#,
        );
        connection.id = "codex".into();
        assert!(probe(&connection).await.is_err());
        assert!(connect(&connection).await.unwrap().is_empty());
    }

    #[test]
    fn node_paths_rank_newer_major_versions_first() {
        let dir = tempfile::tempdir().unwrap();
        for version in ["v9.2.0", "v22.1.0", "v24.1.2", "v24.1.10", "cache"] {
            std::fs::create_dir(dir.path().join(version)).unwrap();
        }
        assert_eq!(
            node_version_paths(dir.path(), "bin"),
            ["v24.1.10", "v24.1.2", "v22.1.0", "v9.2.0"]
                .map(|version| dir.path().join(version).join("bin"))
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
        let agent = detect_in(preset, &search_paths()).expect("adapter must be installed");
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
