//! The Tauri side: the window, and the commands `ui/app.js` calls.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use mimir_tsf::client::EngineClient;
use serde_json::{Value, json};
use tauri::{Builder, Manager, RunEvent, Runtime, State};

use crate::dict;

const TIMEOUT: Duration = Duration::from_secs(2);

/// The engine methods the page may call: the settings and the learning.
const METHODS: [&str; 8] = [
    "get_config",
    "set_config",
    "learning_list",
    "learning_remove",
    "learning_clear",
    "history_list",
    "history_remove",
    "history_clear",
];

struct Engine {
    /// `None` when the server didn't start; taken at exit.
    client: Mutex<Option<EngineClient>>,
    /// `user_dicts/`, as the engine named it (`None` if it didn't).
    user_dicts: Option<PathBuf>,
}

impl Engine {
    fn client(&self) -> MutexGuard<'_, Option<EngineClient>> {
        self.client.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn user_dicts(&self) -> Result<&Path, String> {
        self.user_dicts
            .as_deref()
            .ok_or_else(|| "ユーザー辞書の場所がわかりません（エンジンが起動していません）".into())
    }
}

/// `mimir-imserver`, installed next to this program.
fn server_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(format!("mimir-imserver{}", std::env::consts::EXE_SUFFIX))
}

#[tauri::command(async)]
fn rpc(engine: State<Engine>, method: String, params: Value) -> Result<Value, String> {
    if !METHODS.contains(&method.as_str()) {
        return Err(format!("{method} は呼べません"));
    }
    let mut client = engine.client();
    let client = client.as_mut().ok_or("エンジンを起動できませんでした")?;
    client
        .call(&method, params, TIMEOUT)
        .ok_or_else(|| format!("{method} に失敗しました"))
}

#[tauri::command(async)]
fn dict_list(engine: State<Engine>) -> Result<dict::Listing, String> {
    Ok(dict::list(engine.user_dicts()?))
}

#[tauri::command(async)]
fn dict_add(engine: State<Engine>, reading: String, word: String) -> Result<(), String> {
    dict::add(engine.user_dicts()?, &reading, &word).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn dict_remove(engine: State<Engine>, entries: Vec<dict::Place>) -> Result<(), String> {
    dict::remove(engine.user_dicts()?, &entries).map_err(|e| e.to_string())
}

/// Show `user_dicts/` in the file manager.
#[tauri::command(async)]
fn dict_open(engine: State<Engine>) -> Result<(), String> {
    let dir = engine.user_dicts()?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let opener = if cfg!(windows) { "explorer" } else { "open" };
    Command::new(opener)
        .arg(dir)
        .spawn()
        .map(drop)
        .map_err(|e| format!("{opener}: {e}"))
}

/// `mimir-imserver --profile-models`, in its own process so the window's
/// engine stays free: one JSON line per model.
#[tauri::command(async)]
fn profile_models() -> Result<Vec<Value>, String> {
    let mut command = Command::new(server_path());
    command
        .arg("--profile-models")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

pub fn run() {
    let mut client = EngineClient::spawn_without_models(&server_path()).ok();
    let user_dicts = client
        .as_mut()
        .and_then(|c| c.call("get_config", json!({}), TIMEOUT))
        .and_then(|config| config["user_dict_dir"].as_str().map(PathBuf::from));
    let engine = Engine {
        client: Mutex::new(client),
        user_dicts,
    };
    let app = with_commands(tauri::Builder::default(), engine)
        .build(tauri::generate_context!())
        .expect("the settings window");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Closing its stdin lets the engine save before it exits.
            drop(app.state::<Engine>().client().take());
        }
    });
}

fn with_commands<R: Runtime>(builder: Builder<R>, engine: Engine) -> Builder<R> {
    builder
        .manage(engine)
        .invoke_handler(tauri::generate_handler![
            rpc,
            dict_list,
            dict_add,
            dict_remove,
            dict_open,
            profile_models
        ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    /// The commands under the names and arguments `ui/app.js` uses.
    #[test]
    fn the_page_reaches_the_commands() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine {
            client: Mutex::new(None),
            user_dicts: Some(dir.path().join("user_dicts")),
        };
        let app = with_commands(mock_builder(), engine)
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let call = |cmd: &str, args: Value| {
            get_ipc_response(
                &webview,
                InvokeRequest {
                    cmd: cmd.into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: "tauri://localhost".parse().unwrap(),
                    body: InvokeBody::Json(args),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.to_string(),
                },
            )
            .map(|body| body.deserialize::<Value>().unwrap())
        };

        call("dict_add", json!({"reading": "みみる", "word": "Mimir"})).unwrap();
        let listing = call("dict_list", json!({})).unwrap();
        assert_eq!(listing["entries"][0]["word"], "Mimir");
        let place = json!({"file": "registered.tsv", "line": 0});
        call("dict_remove", json!({ "entries": [place] })).unwrap();
        assert_eq!(call("dict_list", json!({})).unwrap()["entries"], json!([]));

        let refused = call("rpc", json!({"method": "process_key", "params": {}}));
        assert!(refused.unwrap_err().to_string().contains("呼べません"));
        let no_engine = call("rpc", json!({"method": "get_config", "params": {}}));
        assert!(
            no_engine
                .unwrap_err()
                .to_string()
                .contains("起動できません")
        );
    }
}
