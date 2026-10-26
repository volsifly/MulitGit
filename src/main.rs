use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::{
    App, AppContext as _, Application, Bounds, ClickEvent, Context, FocusHandle, KeyDownEvent,
    ListHorizontalSizingBehavior, MouseButton, PromptButton, PromptLevel, Render, ResizeEdge,
    SharedString, Window, WindowBounds, WindowDecorations, WindowOptions, div, img, px, relative,
    rgb, size, uniform_list,
};
use pulldown_cmark::{Options, Parser};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

const BG: u32 = 0xf4f5f7;
const PANEL: u32 = 0xffffff;
const INK: u32 = 0x25313d;
const MUTED: u32 = 0x788592;
const GREEN: u32 = 0x327b59;
const RED: u32 = 0xc84c51;
const LINE: u32 = 0xe7eaed;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ChangedFile {
    path: String,
    original_path: Option<String>,
    code: String,
    staged: bool,
    status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Repository {
    path: String,
    name: String,
    branch: String,
    dirty: bool,
    #[serde(default)]
    files: Vec<ChangedFile>,
    upstream: Option<String>,
    ahead: usize,
    behind: usize,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    last_modified: i64,
    last_fetch: Option<String>,
    fetch_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Settings {
    base_url: String,
    model: String,
    api_key: String,
    font_size: u32,
    font_family: String,
    word_wrap: bool,
    commit_template: String,
    commit_guidelines: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            model: String::new(),
            api_key: String::new(),
            font_size: 13,
            font_family: "system".into(),
            word_wrap: true,
            commit_template: "<type>(<scope>): <subject>\n\n<body>\n\n<footer>".into(),
            commit_guidelines: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Clone, Debug)]
enum Action {
    Refresh,
    Select(String),
    File(String, String),
    Git(String, String, Value),
    Discard(String, String),
    AddRoot(String),
    RemoveRoot(usize),
    Ai(String, String, String, Vec<String>),
    AiChat(String, String, String, Vec<ChatMessage>),
    History(String, bool, String, usize),
    HistoryDiff(String, String),
    LoadSettings,
    SaveSettings(Settings),
    TestSettings(String),
}

#[derive(Clone, Debug)]
enum Event {
    Repositories(Vec<String>, Vec<Repository>),
    Selected(Repository),
    File(String, String, Value),
    FileError(String, String, String),
    Result(String),
    Settings(Settings),
    Error(String),
    RepoError(String, String),
    Ai(String, String, String),
    History(String, Value, bool),
    HistoryDiff(String, String, Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryScope {
    Repository,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditTarget {
    Commit,
    Chat,
    FileFilter,
    BaseUrl,
    Model,
    ApiKey,
    CommitTemplate,
    CommitGuidelines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Diff,
    Preview,
    Summary,
    History,
    Settings,
    Terminal,
}

fn spawn_backend_if_needed(client: &reqwest::blocking::Client, base: &str) -> Option<Child> {
    if client.get(format!("{base}/api/health")).send().is_ok() {
        return None;
    }
    let configured = env::var_os("MULITGIT_BACKEND").map(PathBuf::from);
    let exe = env::current_exe().ok()?;
    let mut candidates = Vec::new();
    if let Some(path) = configured {
        candidates.push(path);
    }
    if let Some(parent) = exe.parent() {
        candidates.push(parent.join(if cfg!(windows) {
            "mulitgit.exe"
        } else {
            "mulitgit"
        }));
        if let Some(root) = parent.parent().and_then(|p| p.parent()) {
            candidates.push(root.join(if cfg!(windows) {
                "mulitgit.exe"
            } else {
                "mulitgit"
            }));
        }
    }
    let executable = candidates.into_iter().find(|path| path.is_file())?;
    Command::new(executable)
        .env("MULITGIT_ADDR", "127.0.0.1:3210")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn send(tx: &Sender<Action>, action: Action) {
    let _ = tx.send(action);
}

fn request<T: for<'de> Deserialize<'de>>(
    client: &reqwest::blocking::Client,
    method: reqwest::Method,
    url: String,
    body: Option<Value>,
) -> Result<T, String> {
    let mut req = client.request(method, url);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().map_err(|e| e.to_string())?;
    let status = response.status();
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("HTTP {status}")));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("响应格式错误：{e}"))
}

fn worker(rx: Receiver<Action>, tx: Sender<Event>, base: String) {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(95))
        .build()
        .expect("HTTP client");
    let _backend = spawn_backend_if_needed(&client, &base);
    let health = client.get(format!("{base}/api/health")).send();
    if health.is_err() {
        let _ = tx.send(Event::Error(format!(
            "连接不到本地 MulitGit 服务：{base}。请先启动 Go 服务，或设置 MULITGIT_BACKEND 指向服务程序。"
        )));
    }
    // Keep long AI/Git requests off the interactive read lanes. Each lane has
    // one worker so rapid clicks do not create an unbounded number of threads.
    let refreshing = Arc::new(AtomicBool::new(false));
    let mut lanes = Vec::new();
    for name in ["selection", "files", "refresh", "ai", "mutations"] {
        let (lane_tx, lane_rx) = mpsc::channel::<Action>();
        let (client, base, tx, refreshing) =
            (client.clone(), base.clone(), tx.clone(), refreshing.clone());
        thread::Builder::new()
            .name(format!("mulitgit-{name}"))
            .spawn(move || {
                while let Ok(mut action) = lane_rx.recv() {
                    // Only the latest pending file matters after rapid selection.
                    if name == "files" {
                        for pending in lane_rx.try_iter() {
                            action = pending;
                        }
                    }
                    let is_refresh = matches!(&action, Action::Refresh);
                    execute_action(action, &client, &tx, &base);
                    if is_refresh {
                        refreshing.store(false, Ordering::Release);
                    }
                }
            })
            .expect("API lane");
        lanes.push(lane_tx);
    }
    while let Ok(action) = rx.recv() {
        let lane = match &action {
            Action::Refresh => {
                if refreshing.swap(true, Ordering::AcqRel) {
                    continue;
                }
                2
            }
            Action::File(..) => 1,
            Action::Ai(..) | Action::AiChat(..) => 3,
            Action::Git(..)
            | Action::Discard(..)
            | Action::AddRoot(..)
            | Action::RemoveRoot(..)
            | Action::SaveSettings(..)
            | Action::TestSettings(..) => 4,
            Action::Select(..)
            | Action::History(..)
            | Action::HistoryDiff(..)
            | Action::LoadSettings => 0,
        };
        if lanes[lane].send(action).is_err() {
            break;
        }
    }
}

fn execute_action(
    action: Action,
    client: &reqwest::blocking::Client,
    tx: &Sender<Event>,
    base: &str,
) {
    let error_repo = match &action {
        Action::Select(repo)
        | Action::Git(repo, ..)
        | Action::History(repo, ..)
        | Action::HistoryDiff(repo, ..)
        | Action::Discard(repo, _)
        | Action::Ai(_, _, repo, _)
        | Action::AiChat(_, _, repo, _) => Some(repo.clone()),
        _ => None,
    };
    let result: Result<(), String> = (|| {
        match action {
            Action::LoadSettings => {
                let settings: Settings = request(
                    &client,
                    reqwest::Method::GET,
                    format!("{base}/api/settings"),
                    None,
                )?;
                tx.send(Event::Settings(settings))
                    .map_err(|e| e.to_string())?;
            }
            Action::Refresh => {
                let data: Value = request(
                    &client,
                    reqwest::Method::GET,
                    format!("{base}/api/repos"),
                    None,
                )?;
                let roots = data["roots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let repos =
                    serde_json::from_value(data["repos"].clone()).map_err(|e| e.to_string())?;
                tx.send(Event::Repositories(roots, repos))
                    .map_err(|e| e.to_string())?;
            }
            Action::Select(path) => {
                let url = format!("{base}/api/repo?path={}", urlencoding::encode(&path));
                let repo = request(&client, reqwest::Method::GET, url, None)?;
                tx.send(Event::Selected(repo)).map_err(|e| e.to_string())?;
            }
            Action::File(repo, file) => {
                let url = format!(
                    "{base}/api/file?repo={}&path={}",
                    urlencoding::encode(&repo),
                    urlencoding::encode(&file)
                );
                match request::<Value>(&client, reqwest::Method::GET, url, None) {
                    Ok(data) => tx.send(Event::File(repo, file, data)),
                    Err(error) => tx.send(Event::FileError(repo, file, error)),
                }
                .map_err(|e| e.to_string())?;
            }
            Action::Git(path, endpoint, body) => {
                let push_after_commit = endpoint == "commit" && body["pushAfterCommit"] == true;
                let result: Value = request(
                    &client,
                    reqwest::Method::POST,
                    format!("{base}/api/{endpoint}"),
                    Some(body),
                )?;
                let repo = result
                    .get("repo")
                    .cloned()
                    .or_else(|| result.get("path").map(|_| result.clone()));
                if let Some(repo) = repo {
                    if let Ok(repo) = serde_json::from_value::<Repository>(repo) {
                        tx.send(Event::Selected(repo)).map_err(|e| e.to_string())?;
                    }
                }
                if push_after_commit {
                    tx.send(Event::Result(format!(
                        "提交完成 · {} · 正在推送…",
                        result["hash"].as_str().unwrap_or("")
                    )))
                    .map_err(|e| e.to_string())?;
                    let pushed: Value = request(
                        &client,
                        reqwest::Method::POST,
                        format!("{base}/api/push"),
                        Some(json!({"path":path})),
                    )?;
                    if let Some(repo) = pushed.get("repo").cloned()
                        && let Ok(repo) = serde_json::from_value::<Repository>(repo)
                    {
                        tx.send(Event::Selected(repo)).map_err(|e| e.to_string())?;
                    }
                    tx.send(Event::Result("提交并推送完成".into()))
                        .map_err(|e| e.to_string())?;
                }
                let output = result
                    .get("hash")
                    .and_then(Value::as_str)
                    .map(|hash| format!("提交完成 · {hash}"))
                    .or_else(|| {
                        result
                            .get("output")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "操作完成".into());
                if !push_after_commit {
                    tx.send(Event::Result(output)).map_err(|e| e.to_string())?;
                }
                let data: Value = request(
                    &client,
                    reqwest::Method::GET,
                    format!("{base}/api/repos"),
                    None,
                )?;
                let roots = data["roots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let repos =
                    serde_json::from_value(data["repos"].clone()).map_err(|e| e.to_string())?;
                tx.send(Event::Repositories(roots, repos))
                    .map_err(|e| e.to_string())?;
                let repo: Repository = request(
                    &client,
                    reqwest::Method::GET,
                    format!("{base}/api/repo?path={}", urlencoding::encode(&path)),
                    None,
                )?;
                tx.send(Event::Selected(repo)).map_err(|e| e.to_string())?;
            }
            Action::AddRoot(path) => {
                let _: Value = request(
                    &client,
                    reqwest::Method::POST,
                    format!("{base}/api/roots"),
                    Some(json!({"path":path})),
                )?;
                tx.send(Event::Result("已添加监控目录".into()))
                    .map_err(|e| e.to_string())?;
                let data: Value = request(
                    &client,
                    reqwest::Method::GET,
                    format!("{base}/api/repos"),
                    None,
                )?;
                let roots = data["roots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let repos =
                    serde_json::from_value(data["repos"].clone()).map_err(|e| e.to_string())?;
                tx.send(Event::Repositories(roots, repos))
                    .map_err(|e| e.to_string())?;
            }
            Action::RemoveRoot(index) => {
                let _: Value = request(
                    &client,
                    reqwest::Method::DELETE,
                    format!("{base}/api/roots?index={index}"),
                    None,
                )?;
                tx.send(Event::Result("已移除监控目录".into()))
                    .map_err(|e| e.to_string())?;
                let data: Value = request(
                    &client,
                    reqwest::Method::GET,
                    format!("{base}/api/repos"),
                    None,
                )?;
                let roots = data["roots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let repos =
                    serde_json::from_value(data["repos"].clone()).map_err(|e| e.to_string())?;
                tx.send(Event::Repositories(roots, repos))
                    .map_err(|e| e.to_string())?;
            }
            Action::Ai(task, path, repo, files) => {
                let data: Value = request(
                    &client,
                    reqwest::Method::POST,
                    format!("{base}/api/ai/generate"),
                    Some(json!({"task":task,"path":path,"repo":repo,"files":files})),
                )?;
                let content = data["content"]
                    .as_str()
                    .unwrap_or("AI 返回了空内容")
                    .to_owned();
                tx.send(Event::Ai(repo, task, content))
                    .map_err(|e| e.to_string())?;
            }
            Action::AiChat(scope, path, repo, messages) => {
                let data: Value = request(
                    &client,
                    reqwest::Method::POST,
                    format!("{base}/api/ai/generate"),
                    Some(
                        json!({"task":"chat","scope":scope,"path":path,"repo":repo,"messages":messages}),
                    ),
                )?;
                let content = data["content"]
                    .as_str()
                    .unwrap_or("AI 返回了空内容")
                    .to_owned();
                tx.send(Event::Ai(repo, "chat".into(), content))
                    .map_err(|e| e.to_string())?;
            }
            Action::Discard(path, file) => {
                let repo: Repository = request(
                    &client,
                    reqwest::Method::POST,
                    format!("{base}/api/discard"),
                    Some(json!({"path":path,"file":file})),
                )?;
                tx.send(Event::Selected(repo)).map_err(|e| e.to_string())?;
                tx.send(Event::Result(format!("已撤销 {file}")))
                    .map_err(|e| e.to_string())?;
            }
            Action::History(path, file_scope, file, offset) => {
                let mut url = format!(
                    "{base}/api/history?repo={}&offset={offset}",
                    urlencoding::encode(&path)
                );
                if file_scope && !file.is_empty() {
                    url.push_str("&file=");
                    url.push_str(&urlencoding::encode(&file));
                }
                let mut data: Value = request(&client, reqwest::Method::GET, url, None)?;
                let more = data["hasMore"].as_bool().unwrap_or(false);
                data["offset"] = json!(offset);
                tx.send(Event::History(path, data, more))
                    .map_err(|e| e.to_string())?;
            }
            Action::HistoryDiff(repo, hash) => {
                let url = format!(
                    "{base}/api/history/commit?repo={}&hash={}",
                    urlencoding::encode(&repo),
                    urlencoding::encode(&hash)
                );
                let data: Value = request(&client, reqwest::Method::GET, url, None)?;
                tx.send(Event::HistoryDiff(repo, hash, data))
                    .map_err(|e| e.to_string())?;
            }
            Action::SaveSettings(settings) => {
                let saved: Settings = request(
                    &client,
                    reqwest::Method::PUT,
                    format!("{base}/api/settings"),
                    Some(serde_json::to_value(settings).map_err(|e| e.to_string())?),
                )?;
                tx.send(Event::Settings(saved)).map_err(|e| e.to_string())?;
                tx.send(Event::Result("设置已保存".into()))
                    .map_err(|e| e.to_string())?;
            }
            Action::TestSettings(api_key) => {
                let result: Value = request(
                    &client,
                    reqwest::Method::POST,
                    format!("{base}/api/ai/test"),
                    Some(json!({"apiKey":api_key})),
                )?;
                let message = result["message"].as_str().unwrap_or("连接成功").to_owned();
                tx.send(Event::Result(format!("模型连接成功：{message}")))
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let event = match error_repo {
            Some(repo) => Event::RepoError(repo, error),
            None => Event::Error(error),
        };
        let _ = tx.send(event);
    }
}

struct AppView {
    tx: Sender<Action>,
    base_url: String,
    roots: Vec<String>,
    repos: Vec<Repository>,
    repo_sort: usize,
    selected_path: String,
    selected_file: String,
    selected_files: Vec<String>,
    file_data: Option<Value>,
    diff_lines: Vec<SharedString>,
    preview_lines: Vec<SharedString>,
    compare_left: Vec<SharedString>,
    compare_right: Vec<SharedString>,
    diff_measure_row: Option<usize>,
    preview_measure_row: Option<usize>,
    file_error: String,
    tab: Tab,
    status: String,
    error: String,
    commit_input: String,
    settings: Settings,
    history: Vec<Value>,
    history_scope: HistoryScope,
    history_more: bool,
    history_offset: usize,
    history_selected: Option<Value>,
    history_diff: String,
    history_truncated: bool,
    compare_view: bool,
    file_filter: String,
    push_after_commit: bool,
    show_api_key: bool,
    ai_text: String,
    chat_messages: Vec<ChatMessage>,
    chat_input: String,
    input_target: Option<EditTarget>,
    focus: FocusHandle,
}

impl AppView {
    fn update_selected(&mut self, repo: Repository) {
        if repo.path != self.selected_path {
            return;
        }
        self.selected_files
            .retain(|path| repo.files.iter().any(|f| &f.path == path));
        self.repos
            .iter_mut()
            .find(|r| r.path == repo.path)
            .map(|r| *r = repo.clone());
        if self.selected_file.is_empty() || !repo.files.iter().any(|f| f.path == self.selected_file)
        {
            self.selected_file = repo
                .files
                .first()
                .map(|f| f.path.clone())
                .unwrap_or_default();
        }
        self.file_data = None;
        self.diff_lines.clear();
        self.preview_lines.clear();
        self.compare_left.clear();
        self.compare_right.clear();
        self.file_error.clear();
        if !self.selected_file.is_empty() {
            send(
                &self.tx,
                Action::File(repo.path, self.selected_file.clone()),
            );
        }
    }

    fn on_event(&mut self, event: Event) -> bool {
        match event {
            Event::Repositories(roots, repos) => {
                if self.roots == roots && self.repos == repos && self.status != "正在连接…" {
                    return false;
                }
                self.roots = roots;
                self.repos = repos;
                if self.status == "正在连接…" {
                    self.status = "本地服务已连接".into();
                }
                if !self.selected_path.is_empty()
                    && self.selected_path
                        != self
                            .repos
                            .iter()
                            .find(|r| r.path == self.selected_path)
                            .map(|r| r.path.clone())
                            .unwrap_or_default()
                {
                    self.selected_path.clear();
                    self.selected_file.clear();
                    self.selected_files.clear();
                    self.file_data = None;
                    self.diff_lines.clear();
                    self.preview_lines.clear();
                    self.compare_left.clear();
                    self.compare_right.clear();
                    self.file_error.clear();
                    self.history.clear();
                    self.ai_text.clear();
                    self.chat_messages.clear();
                }
            }
            Event::Selected(repo) => {
                if repo.path != self.selected_path {
                    return false;
                }
                self.update_selected(repo);
            }
            Event::File(repo, file, data) => {
                if repo == self.selected_path && file == self.selected_file {
                    self.diff_lines = data["diff"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .map(|line| {
                            SharedString::from(if line.is_empty() { " " } else { line }.to_owned())
                        })
                        .collect();
                    let (left, right) = parse_compare_rows(data["diff"].as_str().unwrap_or(""));
                    self.compare_left = left.into_iter().map(SharedString::from).collect();
                    self.compare_right = right.into_iter().map(SharedString::from).collect();
                    self.preview_lines = data["preview"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .map(|line| {
                            SharedString::from(if line.is_empty() { " " } else { line }.to_owned())
                        })
                        .collect();
                    self.diff_measure_row = self
                        .diff_lines
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, line)| line.len())
                        .map(|(index, _)| index);
                    self.preview_measure_row = self
                        .preview_lines
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, line)| line.len())
                        .map(|(index, _)| index);
                    self.file_data = Some(data);
                    self.file_error.clear();
                } else {
                    return false;
                }
            }
            Event::FileError(repo, file, error) => {
                if repo == self.selected_path && file == self.selected_file {
                    self.file_error = error;
                } else {
                    return false;
                }
            }
            Event::Result(text) => {
                self.status = text;
                self.error.clear();
            }
            Event::Settings(settings) => {
                if self.settings == settings {
                    return false;
                }
                self.settings = settings;
            }
            Event::Ai(repo, task, text) => {
                if repo != self.selected_path {
                    return false;
                }
                if task == "commit" {
                    self.commit_input = text;
                } else if task == "chat" {
                    self.chat_messages.push(ChatMessage {
                        role: "assistant".into(),
                        content: text,
                    });
                } else {
                    self.ai_text = text;
                    self.tab = Tab::Summary;
                }
            }
            Event::History(repo, data, more) => {
                if repo != self.selected_path {
                    return false;
                }
                let offset = data["offset"].as_u64().unwrap_or(0) as usize;
                let commits = data["commits"].as_array().cloned().unwrap_or_default();
                if offset == 0 {
                    self.history.clear();
                }
                self.history.extend(commits);
                self.history_offset = offset;
                self.history_more = more;
            }
            Event::HistoryDiff(repo, hash, data) => {
                if repo != self.selected_path
                    || self
                        .history_selected
                        .as_ref()
                        .and_then(|c| c["hash"].as_str())
                        != Some(hash.as_str())
                {
                    return false;
                }
                self.history_diff = data["diff"].as_str().unwrap_or("").to_owned();
                self.history_truncated = data["truncated"].as_bool().unwrap_or(false);
            }
            Event::Error(error) => self.error = error,
            Event::RepoError(repo, error) => {
                if repo != self.selected_path {
                    return false;
                }
                self.error = error;
            }
        }
        true
    }

    fn select_repo(&mut self, path: String) {
        self.selected_path = path.clone();
        self.selected_file.clear();
        self.file_data = None;
        self.diff_lines.clear();
        self.preview_lines.clear();
        self.compare_left.clear();
        self.compare_right.clear();
        self.file_error.clear();
        self.error.clear();
        self.history.clear();
        self.history_selected = None;
        self.history_diff.clear();
        self.history_offset = 0;
        self.history_more = false;
        self.ai_text.clear();
        self.chat_messages.clear();
        self.chat_input.clear();
        self.commit_input.clear();
        self.selected_files = self
            .selected_repo()
            .map(|r| r.files.iter().map(|f| f.path.clone()).collect())
            .unwrap_or_default();
        send(&self.tx, Action::Select(path));
    }
    fn selected_repo(&self) -> Option<&Repository> {
        self.repos.iter().find(|r| r.path == self.selected_path)
    }
    fn toggle_file(&mut self, path: &str) {
        if self.selected_files.iter().any(|p| p == path) {
            self.selected_files.retain(|p| p != path)
        } else {
            self.selected_files.push(path.to_owned())
        }
    }
    fn ask_ai(&mut self, scope: &str, message: String) {
        let message = message.trim().to_owned();
        if message.is_empty() || self.selected_path.is_empty() {
            return;
        }
        self.chat_messages.push(ChatMessage {
            role: "user".into(),
            content: message,
        });
        send(
            &self.tx,
            Action::AiChat(
                scope.into(),
                self.selected_file.clone(),
                self.selected_path.clone(),
                self.chat_messages.clone(),
            ),
        );
    }
    fn run_git(&mut self, endpoint: &str, body: Value) {
        if !self.selected_path.is_empty() {
            send(
                &self.tx,
                Action::Git(self.selected_path.clone(), endpoint.into(), body),
            );
        }
    }
    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(target) = self.input_target else {
            return;
        };
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => self.input_target = None,
            "backspace" => match target {
                EditTarget::Chat => {
                    self.chat_input.pop();
                }
                EditTarget::FileFilter => {
                    self.file_filter.pop();
                }
                EditTarget::Commit => {
                    self.commit_input.pop();
                }
                EditTarget::BaseUrl => {
                    self.settings.base_url.pop();
                }
                EditTarget::Model => {
                    self.settings.model.pop();
                }
                EditTarget::ApiKey => {
                    self.settings.api_key.pop();
                }
                EditTarget::CommitTemplate => {
                    self.settings.commit_template.pop();
                }
                EditTarget::CommitGuidelines => {
                    self.settings.commit_guidelines.pop();
                }
            },
            "enter" => match target {
                EditTarget::Chat if !self.chat_input.trim().is_empty() => {
                    let message = std::mem::take(&mut self.chat_input);
                    let scope = if self.selected_file.is_empty() {
                        "all"
                    } else {
                        "file"
                    };
                    self.ask_ai(scope, message);
                }
                EditTarget::Commit => self.commit_input.push('\n'),
                EditTarget::CommitTemplate => self.settings.commit_template.push('\n'),
                EditTarget::CommitGuidelines => self.settings.commit_guidelines.push('\n'),
                _ => {}
            },
            "tab" => {
                self.tab = match self.tab {
                    Tab::Diff => Tab::Preview,
                    Tab::Preview => Tab::Summary,
                    Tab::Summary => Tab::History,
                    Tab::History => Tab::Terminal,
                    Tab::Terminal => Tab::Diff,
                    Tab::Settings => Tab::Diff,
                };
            }
            _ => {
                if let Some(ch) = event.keystroke.key_char.as_deref() {
                    if !ch.chars().any(char::is_control) {
                        match target {
                            EditTarget::Chat => self.chat_input.push_str(ch),
                            EditTarget::FileFilter => self.file_filter.push_str(ch),
                            EditTarget::Commit => self.commit_input.push_str(ch),
                            EditTarget::BaseUrl => self.settings.base_url.push_str(ch),
                            EditTarget::Model => self.settings.model.push_str(ch),
                            EditTarget::ApiKey => self.settings.api_key.push_str(ch),
                            EditTarget::CommitTemplate => {
                                self.settings.commit_template.push_str(ch)
                            }
                            EditTarget::CommitGuidelines => {
                                self.settings.commit_guidelines.push_str(ch)
                            }
                        }
                    }
                }
            }
        }
        cx.notify();
    }
}

fn parse_compare_rows(diff: &str) -> (Vec<String>, Vec<String>) {
    let (mut left, mut right, mut removed, mut added) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let flush = |left: &mut Vec<String>,
                 right: &mut Vec<String>,
                 removed: &mut Vec<String>,
                 added: &mut Vec<String>| {
        let count = removed.len().max(added.len());
        for i in 0..count {
            left.push(removed.get(i).cloned().unwrap_or_default());
            right.push(added.get(i).cloned().unwrap_or_default());
        }
        removed.clear();
        added.clear();
    };
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("@@ ") {
            flush(&mut left, &mut right, &mut removed, &mut added);
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        if line.starts_with('-') && !line.starts_with("---") {
            removed.push(line[1..].to_owned());
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added.push(line[1..].to_owned());
        } else if line.starts_with(' ') {
            flush(&mut left, &mut right, &mut removed, &mut added);
            left.push(line[1..].to_owned());
            right.push(line[1..].to_owned());
        } else if !line.starts_with('\\') {
            flush(&mut left, &mut right, &mut removed, &mut added);
            in_hunk = false;
        }
    }
    flush(&mut left, &mut right, &mut removed, &mut added);
    (left, right)
}

fn is_image_path(path: &str) -> bool {
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".bmp", ".svg",
    ]
    .iter()
    .any(|ext| path.to_lowercase().ends_with(ext))
}

fn render_markdown(source: &str) -> gpui::Div {
    let mut blocks = Vec::new();
    let mut text = String::new();
    let mut heading = false;
    let mut code = false;
    for event in Parser::new_ext(source, Options::all()) {
        match event {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Heading { .. }) => heading = true,
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_)) => code = true,
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Item) => text.push_str("• "),
            pulldown_cmark::Event::Text(value) | pulldown_cmark::Event::Code(value) => {
                text.push_str(&value)
            }
            pulldown_cmark::Event::SoftBreak | pulldown_cmark::Event::HardBreak => text.push('\n'),
            pulldown_cmark::Event::End(
                pulldown_cmark::TagEnd::Paragraph
                | pulldown_cmark::TagEnd::Heading(_)
                | pulldown_cmark::TagEnd::CodeBlock
                | pulldown_cmark::TagEnd::Item,
            ) => {
                if !text.is_empty() {
                    blocks.push(
                        div()
                            .mb_3()
                            .when(heading, |el| {
                                el.text_size(px(20.)).font_weight(gpui::FontWeight::BOLD)
                            })
                            .when(code, |el| {
                                el.font_family("monospace")
                                    .p_3()
                                    .bg(rgb(0xf4f5f7))
                                    .rounded_md()
                            })
                            .child(std::mem::take(&mut text)),
                    );
                }
                heading = false;
                code = false;
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        blocks.push(div().child(text));
    }
    div().flex().flex_col().children(blocks)
}

fn badge(value: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xeef2f3))
        .text_color(rgb(MUTED))
        .text_sm()
        .child(value.into())
}
impl AppView {
    fn render_sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut roots = self.roots.clone();
        let mut repos = self.repos.clone();
        match self.repo_sort {
            0 => roots.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase())),
            sort_field => roots.sort_by(|a, b| {
                let latest = |root: &str| {
                    repos
                        .iter()
                        .filter(|repo| {
                            repo.path == root || repo.path.starts_with(&format!("{root}/"))
                        })
                        .map(|repo| {
                            if sort_field == 1 {
                                repo.created_at
                            } else {
                                repo.last_modified
                            }
                        })
                        .max()
                        .unwrap_or_default()
                };
                latest(b).cmp(&latest(a))
            }),
        }
        match self.repo_sort {
            0 => repos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            1 => repos.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            _ => repos.sort_by(|a, b| b.last_modified.cmp(&a.last_modified)),
        }
        let active = self.selected_path.clone();
        div()
            .w(relative(0.17))
            .min_w(px(145.))
            .flex_shrink()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .rounded_lg()
            .border_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("监控目录"),
                    )
                    .child(button(
                        cx,
                        match self.repo_sort {
                            0 => "名称 ▾",
                            1 => "创建时间 ▾",
                            _ => "修改时间 ▾",
                        },
                        |this, _, _, cx| {
                            this.repo_sort = (this.repo_sort + 1) % 3;
                            cx.notify();
                        },
                    ))
                    .child(button(cx, "＋ 添加", |this, _, _, cx| {
                        let tx = this.tx.clone();
                        let path = cx.prompt_for_paths(gpui::PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: false,
                            prompt: Some("选择监控目录".into()),
                        });
                        cx.spawn(async move |_, _| {
                            if let Ok(Ok(Some(paths))) = path.await {
                                if let Some(path) = paths.first() {
                                    let _ = tx
                                        .send(Action::AddRoot(path.to_string_lossy().to_string()));
                                }
                            }
                        })
                        .detach();
                    })),
            )
            .child(
                div()
                    .id("root-list")
                    .p_2()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(roots.iter().enumerate().map(|(index, root)| {
                        let root_name = root.rsplit('/').next().unwrap_or(root).to_owned();
                        let root_path = root.clone();
                        let tx = self.tx.clone();
                        let children =
                            repos
                                .iter()
                                .filter(|repo| {
                                    repo.path == root_path
                                        || repo.path.starts_with(&format!("{}/", root_path))
                                })
                                .map(|repo| {
                                    let path = repo.path.clone();
                                    let name = repo.name.clone();
                                    let branch = repo.branch.clone();
                                    let is_active = path == active;
                                    let dirty = repo.dirty;
                                    let ahead = repo.ahead;
                                    let behind = repo.behind;
                                    div()
                                .id(SharedString::from(path.clone()))
                                .my_0p5()
                                .ml_3()
                                .px_2()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgb(if is_active { 0xeaf3ee } else { PANEL }))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_repo(path.clone());
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div().w(px(7.)).h(px(7.)).rounded_full().bg(
                                                        rgb(if dirty { RED } else { 0xc6cdd2 }),
                                                    ),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .child(
                                                            div()
                                                                .font_weight(
                                                                    gpui::FontWeight::SEMIBOLD,
                                                                )
                                                                .child(name),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(MUTED))
                                                                .child(branch),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div().flex().gap_1().children(
                                                [
                                                    (behind > 0)
                                                        .then(|| badge(format!("↓{behind}"))),
                                                    (ahead > 0).then(|| badge(format!("↑{ahead}"))),
                                                ]
                                                .into_iter()
                                                .flatten(),
                                            ),
                                        ),
                                )
                                });
                        div()
                            .mb_1()
                            .child(
                                div()
                                    .px_2()
                                    .py_2()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .bg(rgb(0xf7f8f9))
                                    .rounded_md()
                                    .child(
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_color(rgb(MUTED))
                                            .child(format!("▾  {root_name}")),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("remove-root-{index}")))
                                            .px_2()
                                            .text_color(rgb(MUTED))
                                            .cursor_pointer()
                                            .on_click(move |_, _, _| {
                                                let _ = tx.send(Action::RemoveRoot(index));
                                            })
                                            .child("×"),
                                    ),
                            )
                            .children(children)
                    })),
            )
    }

    fn render_files(&self, cx: &Context<Self>) -> impl IntoElement {
        let branch = self
            .selected_repo()
            .map(|repo| repo.branch.clone())
            .unwrap_or_else(|| "选择仓库".into());
        let selected_repo = self.selected_repo().cloned();
        let files: Vec<_> = self
            .selected_repo()
            .map(|repo| {
                repo.files
                    .iter()
                    .filter(|file| {
                        file.path
                            .to_lowercase()
                            .contains(&self.file_filter.to_lowercase())
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let active = self.selected_file.clone();
        let selected = self.selected_path.clone();
        let checks = self.selected_files.clone();
        div()
            .w(relative(0.25))
            .min_w(px(190.))
            .flex_shrink()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .rounded_lg()
            .border_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(branch))
                    .child(div().mt_1().flex().gap_2().children(
                        selected_repo.as_ref().into_iter().map(|repo| {
                            let path = repo.path.clone();
                            let pull_label = if repo.behind > 0 {
                                format!("↓ 拉取 {}", repo.behind)
                            } else {
                                "↓ 拉取".into()
                            };
                            let push_label = if repo.upstream.is_none() {
                                "↑ 发布分支".into()
                            } else if repo.ahead > 0 {
                                format!("↑ 推送 {}", repo.ahead)
                            } else {
                                "↑ 推送".into()
                            };
                            let pull_path = path.clone();
                            let push_path = path.clone();
                            let pull = button(cx, pull_label, move |this, _, _, _| {
                                this.run_git("pull", json!({ "path": pull_path }));
                            });
                            let push = button(cx, push_label, move |this, _, _, _| {
                                this.run_git("push", json!({ "path": push_path }));
                            });
                            let fetch = button(cx, "↻ 远端", move |this, _, _, _| {
                                this.run_git("fetch", json!({ "path": path }));
                            });
                            div().flex().gap_2().child(fetch).child(pull).child(push)
                        }),
                    ))
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(format!("变更文件  {}", files.len())),
                            )
                            .child(button(cx, "刷新", |this, _, _, _| {
                                send(&this.tx, Action::Refresh)
                            })),
                    ),
            )
            .child(
                div()
                    .id("file-filter")
                    .mx_3()
                    .mt_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(LINE))
                    .text_color(rgb(MUTED))
                    .cursor_text()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.input_target = Some(EditTarget::FileFilter);
                        window.focus(&this.focus);
                        cx.notify();
                    }))
                    .child(if self.file_filter.is_empty() {
                        "⌕  筛选文件…".to_owned()
                    } else {
                        format!("⌕  {}", self.file_filter)
                    }),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_color(rgb(MUTED))
                    .child(button(
                        cx,
                        if !files.is_empty() && files.iter().all(|file| checks.contains(&file.path))
                        {
                            "取消全选"
                        } else {
                            "全选"
                        },
                        |this, _, _, cx| {
                            let visible: Vec<_> = this
                                .selected_repo()
                                .map(|repo| {
                                    repo.files
                                        .iter()
                                        .filter(|file| {
                                            file.path
                                                .to_lowercase()
                                                .contains(&this.file_filter.to_lowercase())
                                        })
                                        .map(|file| file.path.clone())
                                        .collect()
                                })
                                .unwrap_or_default();
                            if !visible.is_empty()
                                && visible
                                    .iter()
                                    .all(|path| this.selected_files.contains(path))
                            {
                                this.selected_files.retain(|path| !visible.contains(path));
                            } else {
                                for path in visible {
                                    if !this.selected_files.contains(&path) {
                                        this.selected_files.push(path);
                                    }
                                }
                            }
                            cx.notify();
                        },
                    ))
                    .child(format!("{} 已选", checks.len())),
            )
            .child(
                div()
                    .id("changed-files")
                    .p_2()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(files.iter().map(|file| {
                        let path = file.path.clone();
                        let display_path = SharedString::from(path.clone());
                        let row_path = path.clone();
                        let repo = selected.clone();
                        let is_active = path == active;
                        let checked = checks.iter().any(|item| item == &path);
                        let status = file.status.clone();
                        let conflict = status == "冲突";
                        let row_status = status.clone();
                        let code = file.code.clone();
                        let staged = file.staged;
                        let open_path = path.clone();
                        let open_repo = repo.clone();
                        div()
                            .id(SharedString::from(path.clone()))
                            .my_0p5()
                            .w_full()
                            .px_2()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(if is_active { 0xf0f5f2 } else { PANEL }))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_file = open_path.clone();
                                this.file_data = None;
                                this.diff_lines.clear();
                                this.preview_lines.clear();
                                this.compare_left.clear();
                                this.compare_right.clear();
                                this.file_error.clear();
                                send(&this.tx, Action::File(open_repo.clone(), open_path.clone()));
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(button(
                                        cx,
                                        if checked { "☑" } else { "□" },
                                        move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_file(&row_path);
                                            cx.notify();
                                        },
                                    ))
                                    .child(div().w(px(22.)).text_color(rgb(RED)).child(code))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .child(div().overflow_hidden().child(display_path)),
                                    )
                                    .child(button(cx, "↶", move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        if conflict {
                                            return;
                                        }
                                        let prompt_message = format!(
                                            "撤销 {} 的修改？此操作无法通过 MulitGit 撤回。",
                                            path
                                        );
                                        let prompt = window.prompt(
                                            PromptLevel::Warning,
                                            &prompt_message,
                                            None,
                                            &[
                                                PromptButton::ok("撤销修改"),
                                                PromptButton::cancel("取消"),
                                            ],
                                            cx,
                                        );
                                        let repo = repo.clone();
                                        let file = path.clone();
                                        let tx = this.tx.clone();
                                        cx.spawn(async move |_, _| {
                                            if prompt.await.unwrap_or(1) == 0 {
                                                let _ = tx.send(Action::Discard(repo, file));
                                            }
                                        })
                                        .detach();
                                    }))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(if staged { GREEN } else { MUTED }))
                                            .child(row_status),
                                    ),
                            )
                    })),
            )
    }

    fn edit_row(
        &self,
        cx: &Context<Self>,
        label: &'static str,
        value: String,
        target: EditTarget,
    ) -> impl IntoElement {
        div()
            .id(SharedString::from(format!("settings-{}", label)))
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(rgb(MUTED)).child(label))
            .child(
                div()
                    .id(SharedString::from(format!("settings-input-{}", label)))
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(LINE))
                    .cursor_text()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.input_target = Some(target);
                        window.focus(&this.focus);
                        cx.notify();
                    }))
                    .child(if value.is_empty() {
                        format!("点击输入{label}…")
                    } else {
                        value
                    }),
            )
    }

    fn render_settings(&self, cx: &Context<Self>) -> impl IntoElement {
        let key = if self.show_api_key {
            self.settings.api_key.clone()
        } else {
            "•".repeat(self.settings.api_key.chars().count())
        };
        div().flex().flex_col().gap_3()
            .child(div().font_weight(gpui::FontWeight::BOLD).text_size(px(20.)).child("设置"))
            .child(div().text_color(rgb(MUTED)).child("模型连接、提交规范与阅读偏好。设置保存在本机配置中。"))
            .child(self.edit_row(cx, "API Base URL", self.settings.base_url.clone(), EditTarget::BaseUrl))
            .child(self.edit_row(cx, "模型名称", self.settings.model.clone(), EditTarget::Model))
            .child(self.edit_row(cx, "API Key", key, EditTarget::ApiKey))
            .child(div().flex().gap_2()
                .child(button(cx, if self.show_api_key { "隐藏 API Key" } else { "显示 API Key" }, |this, _, _, cx| { this.show_api_key = !this.show_api_key; cx.notify(); }))
                .child(button(cx, "测试模型连接", |this, _, _, _cx| { send(&this.tx, Action::TestSettings(this.settings.api_key.clone())); }))
                .child(button(cx, "保存模型设置", |this, _, _, _cx| { send(&this.tx, Action::SaveSettings(this.settings.clone())); }))
            )
            .child(self.edit_row(cx, "Commit message 模板", self.settings.commit_template.clone(), EditTarget::CommitTemplate))
            .child(div().text_xs().text_color(rgb(MUTED)).child("占位符：<type> <scope> <subject> <body> <footer>；默认规范遵循 Conventional Commits。"))
            .child(self.edit_row(cx, "提交规范说明", self.settings.commit_guidelines.clone(), EditTarget::CommitGuidelines))
            .child(div().flex().gap_2()
                .child(button(cx, "保存模板与规范", |this, _, _, _cx| { send(&this.tx, Action::SaveSettings(this.settings.clone())); }))
                .child(button(cx, "字号 −", |this, _, _, cx| { this.settings.font_size = this.settings.font_size.saturating_sub(1).max(10); cx.notify(); }))
                .child(div().py_1().child(format!("{} px", self.settings.font_size)))
                .child(button(cx, "字号 +", |this, _, _, cx| { this.settings.font_size = (this.settings.font_size + 1).min(24); cx.notify(); }))
                .child(button(cx, if self.settings.font_family == "mono" { "字体：等宽" } else { "字体：系统" }, |this, _, _, cx| { this.settings.font_family = if this.settings.font_family == "mono" { "system".into() } else { "mono".into() }; cx.notify(); }))
                .child(button(cx, if self.settings.word_wrap { "☑ 代码换行" } else { "□ 代码换行" }, |this, _, _, cx| { this.settings.word_wrap = !this.settings.word_wrap; cx.notify(); }))
                .child(button(cx, "保存外观设置", |this, _, _, _cx| { send(&this.tx, Action::SaveSettings(this.settings.clone())); }))
            )
    }

    fn render_compare_lines(&self, cx: &Context<Self>) -> impl IntoElement {
        uniform_list(
            SharedString::from(format!(
                "compare-{}-{}",
                self.selected_path, self.selected_file
            )),
            self.compare_left.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                range
                    .map(|index| {
                        let left = this.compare_left.get(index).cloned().unwrap_or_default();
                        let right = this.compare_right.get(index).cloned().unwrap_or_default();
                        div()
                            .h(px(24.))
                            .flex()
                            .font_family("monospace")
                            .text_size(px(this.settings.font_size.clamp(10, 24) as f32))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .px_2()
                                    .child(left),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .px_2()
                                    .border_l_1()
                                    .border_color(rgb(LINE))
                                    .child(right),
                            )
                    })
                    .collect()
            }),
        )
        .size_full()
        .min_h_0()
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    }

    fn render_text_lines(&self, diff: bool, cx: &Context<Self>) -> impl IntoElement {
        let font_size = self.settings.font_size.clamp(10, 24) as f32;
        let lines = if diff {
            &self.diff_lines
        } else {
            &self.preview_lines
        };
        uniform_list(
            SharedString::from(format!(
                "text-{}-{}-{diff}",
                self.selected_path, self.selected_file
            )),
            lines.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                let lines = if diff {
                    &this.diff_lines
                } else {
                    &this.preview_lines
                };
                range
                    .filter_map(|index| lines.get(index))
                    .map(|line| {
                        let color = if diff && line.starts_with('+') && !line.starts_with("+++") {
                            0x24764d
                        } else if diff && line.starts_with('-') && !line.starts_with("---") {
                            0xb9404d
                        } else {
                            INK
                        };
                        div()
                            .h(px(22.))
                            .font_family("monospace")
                            .text_size(px(font_size))
                            .whitespace_nowrap()
                            .text_color(rgb(color))
                            .child(line.clone())
                    })
                    .collect()
            }),
        )
        .size_full()
        .min_h_0()
        .with_width_from_item(if diff {
            self.diff_measure_row
        } else {
            self.preview_measure_row
        })
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    }

    fn render_right(&self, cx: &Context<Self>) -> impl IntoElement {
        let selected_repo = self.selected_repo();
        let file = self.selected_file.clone();
        let data = &self.file_data;
        let current_tab = self.tab;
        let tabs = [
            (Tab::Diff, "Diff"),
            (Tab::Preview, "文件预览"),
            (Tab::Summary, "AI 总结"),
            (Tab::History, "历史"),
            (Tab::Settings, "设置"),
            (Tab::Terminal, "命令行"),
        ];
        let tabs_ui = div()
            .min_h(px(42.))
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap(px(11.))
            .px_3()
            .border_b_1()
            .border_color(rgb(LINE))
            .children(tabs.into_iter().map(|(tab, label)| {
                div()
                    .id(SharedString::from(format!("tab-{label}")))
                    .px_2()
                    .py_2()
                    .cursor_pointer()
                    .text_color(rgb(if current_tab == tab { GREEN } else { MUTED }))
                    .border_b_2()
                    .border_color(rgb(if current_tab == tab { GREEN } else { PANEL }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        this.input_target = None;
                        if tab == Tab::History && !this.selected_path.is_empty() {
                            send(
                                &this.tx,
                                Action::History(
                                    this.selected_path.clone(),
                                    this.history_scope == HistoryScope::File,
                                    this.selected_file.clone(),
                                    0,
                                ),
                            );
                        }
                        if tab == Tab::Settings {
                            send(&this.tx, Action::LoadSettings);
                        }
                        cx.notify();
                    }))
                    .child(label)
            }));
        let panel =
            match self.tab {
                Tab::Diff => {
                    if self.compare_view {
                        self.render_compare_lines(cx).into_any_element()
                    } else {
                        self.render_text_lines(true, cx).into_any_element()
                    }
                }
                Tab::Preview => {
                    let preview = data
                        .as_ref()
                        .and_then(|value| value["preview"].as_str())
                        .unwrap_or("选择一个文本文件以预览内容。");
                    if is_image_path(&file) {
                        let url = format!(
                            "{}/api/asset?repo={}&path={}",
                            self.base_url.trim_end_matches('/'),
                            urlencoding::encode(&self.selected_path),
                            urlencoding::encode(&file)
                        );
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(img(url).max_w(relative(1.)).max_h(relative(1.)))
                            .into_any_element()
                    } else if data
                        .as_ref()
                        .is_some_and(|d| d["binary"].as_bool() == Some(true))
                    {
                        div().child("此二进制文件暂不支持预览。").into_any_element()
                    } else if file.to_lowercase().ends_with(".md")
                        || file.to_lowercase().ends_with(".markdown")
                    {
                        render_markdown(preview).into_any_element()
                    } else {
                        self.render_text_lines(false, cx).into_any_element()
                    }
                }
                Tab::Summary => div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(render_markdown(&self.ai_text))
                    .children(self.chat_messages.iter().map(|message| {
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(rgb(if message.role == "user" {
                                0xf0f5f2
                            } else {
                                0xf7f8f9
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child(if message.role == "user" { "你" } else { "AI" }),
                            )
                            .child(render_markdown(&message.content))
                    }))
                    .child(button(cx, "✦ 总结当前文件", |this, _, _, cx| {
                        let path = this.selected_file.clone();
                        let repo = this.selected_path.clone();
                        if !path.is_empty() {
                            send(&this.tx, Action::Ai("summary".into(), path, repo, vec![]));
                        }
                        cx.notify();
                    }))
                    .child(button(cx, "总结全部变更", |this, _, _, cx| {
                        this.chat_messages.clear();
                        this.ask_ai(
                            "all",
                            "请总结这个仓库的全部变更，按功能、影响和需要关注的风险归纳。".into(),
                        );
                        cx.notify();
                    }))
                    .child(
                        div()
                            .px_2()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(LINE))
                            .text_color(rgb(MUTED))
                            .id("chat-input")
                            .cursor_text()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.input_target = Some(EditTarget::Chat);
                                window.focus(&this.focus);
                                cx.notify();
                            }))
                            .child(if self.chat_input.is_empty() {
                                "输入问题并按 Enter 追问…".to_owned()
                            } else {
                                self.chat_input.clone()
                            }),
                    )
                    .into_any_element(),
                Tab::History => div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(button(
                                        cx,
                                        if self.history_scope == HistoryScope::Repository {
                                            "当前分支 ✓"
                                        } else {
                                            "当前分支"
                                        },
                                        |this, _, _, cx| {
                                            this.history_scope = HistoryScope::Repository;
                                            this.history.clear();
                                            this.history_selected = None;
                                            this.history_diff.clear();
                                            send(
                                                &this.tx,
                                                Action::History(
                                                    this.selected_path.clone(),
                                                    false,
                                                    String::new(),
                                                    0,
                                                ),
                                            );
                                            cx.notify();
                                        },
                                    ))
                                    .child(button(
                                        cx,
                                        if self.history_scope == HistoryScope::File {
                                            "当前文件 ✓"
                                        } else {
                                            "当前文件"
                                        },
                                        |this, _, _, cx| {
                                            if !this.selected_file.is_empty() {
                                                this.history_scope = HistoryScope::File;
                                                this.history.clear();
                                                this.history_selected = None;
                                                this.history_diff.clear();
                                                send(
                                                    &this.tx,
                                                    Action::History(
                                                        this.selected_path.clone(),
                                                        true,
                                                        this.selected_file.clone(),
                                                        0,
                                                    ),
                                                );
                                            }
                                            cx.notify();
                                        },
                                    )),
                            )
                            .child(button(cx, "刷新", |this, _, _, cx| {
                                this.history.clear();
                                this.history_offset = 0;
                                send(
                                    &this.tx,
                                    Action::History(
                                        this.selected_path.clone(),
                                        this.history_scope == HistoryScope::File,
                                        this.selected_file.clone(),
                                        0,
                                    ),
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("history-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_scroll()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .children(self.history.iter().map(|item| {
                                let hash = item["hash"].as_str().unwrap_or("").to_owned();
                                let active = self
                                    .history_selected
                                    .as_ref()
                                    .and_then(|selected| selected["hash"].as_str())
                                    == Some(hash.as_str());
                                div()
                                    .id(SharedString::from(format!("history-{hash}")))
                                    .px_2()
                                    .py_2()
                                    .rounded_md()
                                    .border_b_1()
                                    .border_color(rgb(LINE))
                                    .bg(rgb(if active { 0xeaf3ee } else { PANEL }))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.history_selected = this
                                            .history
                                            .iter()
                                            .find(|commit| {
                                                commit["hash"].as_str() == Some(hash.as_str())
                                            })
                                            .cloned();
                                        this.history_diff.clear();
                                        this.history_truncated = false;
                                        send(
                                            &this.tx,
                                            Action::HistoryDiff(
                                                this.selected_path.clone(),
                                                hash.clone(),
                                            ),
                                        );
                                        cx.notify();
                                    }))
                                    .child(
                                        div().font_weight(gpui::FontWeight::SEMIBOLD).child(
                                            item["subject"]
                                                .as_str()
                                                .unwrap_or("(提交说明为空)")
                                                .to_owned(),
                                        ),
                                    )
                                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                                        "{}  {}  {}",
                                        item["shortHash"].as_str().unwrap_or(""),
                                        item["author"].as_str().unwrap_or(""),
                                        item["date"].as_str().unwrap_or("")
                                    )))
                            })),
                    )
                    .when(self.history_more, |el| {
                        el.child(button(cx, "加载更多历史", |this, _, _, cx| {
                            let offset = this.history_offset + 30;
                            send(
                                &this.tx,
                                Action::History(
                                    this.selected_path.clone(),
                                    this.history_scope == HistoryScope::File,
                                    this.selected_file.clone(),
                                    offset,
                                ),
                            );
                            cx.notify();
                        }))
                    })
                    .when(self.history_selected.is_some(), |el| {
                        el.child(
                            div()
                                .id("history-detail")
                                .flex()
                                .flex_col()
                                .gap_2()
                                .max_h(px(300.))
                                .overflow_scroll()
                                .child(
                                    div().font_weight(gpui::FontWeight::BOLD).child(
                                        self.history_selected
                                            .as_ref()
                                            .and_then(|c| c["subject"].as_str())
                                            .unwrap_or("提交详情")
                                            .to_owned(),
                                    ),
                                )
                                .child(
                                    div().text_xs().text_color(rgb(MUTED)).child(
                                        self.history_selected
                                            .as_ref()
                                            .and_then(|c| c["body"].as_str())
                                            .unwrap_or("")
                                            .to_owned(),
                                    ),
                                )
                                .child(div().flex().flex_col().children(
                                    self.history_diff.lines().map(|line| {
                                        let color = if line.starts_with('+')
                                            && !line.starts_with("+++")
                                        {
                                            0x24764d
                                        } else if line.starts_with('-') && !line.starts_with("---")
                                        {
                                            0xb9404d
                                        } else {
                                            INK
                                        };
                                        div()
                                            .font_family("monospace")
                                            .text_color(rgb(color))
                                            .child(line.to_owned())
                                    }),
                                ))
                                .when(self.history_truncated, |el| {
                                    el.child(
                                        div()
                                            .text_color(rgb(MUTED))
                                            .child("差异较大，仅展示前 512 KB。"),
                                    )
                                }),
                        )
                    })
                    .into_any_element(),
                Tab::Settings => self.render_settings(cx).into_any_element(),
                Tab::Terminal => div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("仓库终端"),
                    )
                    .child(
                        div()
                            .text_color(rgb(MUTED))
                            .child("当前系统终端支持 Codex CLI 和交互操作。"),
                    )
                    .child(button(
                        cx,
                        "在系统终端中打开仓库",
                        |this, _, _, _| {
                            if let Some(repo) = this.selected_repo() {
                                let path = repo.path.clone();
                                let _ = open_terminal(&path);
                            }
                        },
                    ))
                    .into_any_element(),
            };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .rounded_lg()
            .border_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child("FILE INSPECTOR"),
                            )
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(
                                if file.is_empty() {
                                    selected_repo
                                        .as_ref()
                                        .map(|repo| repo.name.clone())
                                        .unwrap_or_else(|| "尚未选择仓库".into())
                                } else {
                                    file
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(selected_repo.as_ref().is_some_and(|repo| repo.behind > 0), |el| el.child(button(cx, "↓ 拉取", |this, _, _, cx| {
                        if !this.selected_path.is_empty() {
                            this.run_git("pull", json!({"path":this.selected_path}));
                        }
                        cx.notify();
                    })))
                    .when(selected_repo.as_ref().is_some_and(|repo| repo.ahead > 0 || repo.upstream.is_none()), |el| el.child(button(cx, "↑ 推送", |this, _, _, cx| {
                        if !this.selected_path.is_empty() {
                            this.run_git("push", json!({"path":this.selected_path}));
                        }
                        cx.notify();
                    })))
                    .child(button(cx, "检查远端", |this, _, _, cx| {
                        if !this.selected_path.is_empty() {
                            this.run_git("fetch", json!({"path":this.selected_path}));
                        }
                        cx.notify();
                    })),
            )
            .child(
                div().flex().items_center().gap_2().px_4().py_1().border_b_1().border_color(rgb(LINE))
                    .text_xs().text_color(rgb(MUTED))
                    .child(format!("⑂ {}", selected_repo.as_ref().map(|repo| repo.branch.as_str()).unwrap_or("")))
                    .child(selected_repo.as_ref().and_then(|repo| repo.upstream.as_deref()).map(|remote| format!("跟踪 {remote}")).unwrap_or_else(|| "尚未设置 upstream".into()))
                    .child(selected_repo.as_ref().map(|repo| format!("↓{} ↑{}", repo.behind, repo.ahead)).unwrap_or_default()),
            )
            .child(tabs_ui)
            .when(matches!(current_tab, Tab::Diff), |el| el.child(
                div().flex().items_center().justify_between().px_3().py_1()
                    .child(div().text_xs().text_color(rgb(MUTED)).child("比较 HEAD 与工作区"))
                    .child(div().flex().gap_2()
                        .child(button(cx, if self.compare_view { "并排" } else { "统一" }, |this, _, _, cx| { this.compare_view = !this.compare_view; cx.notify(); }))
                    )
            ))
            .child(
                div()
                    .id("inspector-panel")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .flex()
                    .flex_col()
                    .p(px(16.))
                    .when(!self.error.is_empty(), |el| {
                        el.child(
                            div()
                                .mb_3()
                                .p_3()
                                .rounded_md()
                                .bg(rgb(0xffeded))
                                .text_color(rgb(RED))
                                .child(self.error.clone()),
                        )
                    })
                    .when(selected_repo.is_none(), |el| {
                        el.child(
                            div()
                                .text_color(rgb(MUTED))
                                .child("从左侧选择一个 Git 仓库。"),
                        )
                    })
                    .when(
                        !self.file_error.is_empty()
                            && matches!(current_tab, Tab::Diff | Tab::Preview),
                        |el| el.child(div().text_color(rgb(RED)).child(self.file_error.clone())),
                    )
                    .when(
                        self.file_error.is_empty()
                            && data.is_none()
                            && !self.selected_file.is_empty()
                            && matches!(current_tab, Tab::Diff | Tab::Preview),
                        |el| el.child(div().text_color(rgb(MUTED)).child("正在加载文件…")),
                    )
                    .when(
                        !matches!(current_tab, Tab::Diff | Tab::Preview)
                            || data.is_some()
                            || self.selected_file.is_empty(),
                        |el| el.child(panel),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child("COMMIT MESSAGE · 输入提交说明后点击提交"),
                    )
                    .child(
                        div()
                            .id("commit-message")
                            .cursor_text()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.input_target = Some(EditTarget::Commit);
                                window.focus(&this.focus);
                                cx.notify();
                            }))
                            .h(px(100.))
                            .p_2()
                            .overflow_y_scroll()
                            .border_1()
                            .border_color(rgb(LINE))
                            .rounded_md()
                            .child(
                                div().flex().flex_col().children(
                                    if self.commit_input.is_empty() {
                                        "点击输入提交说明，Enter 换行"
                                    } else {
                                        &self.commit_input
                                    }
                                    .split('\n')
                                    .map(|line| {
                                        div().child(
                                            if line.is_empty() { " " } else { line }.to_owned(),
                                        )
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(button(cx, "AI 生成提交说明", |this, _, _, cx| {
                                let files = this.selected_files.clone();
                                let repo = this.selected_path.clone();
                                if files.is_empty() {
                                    this.error = "请先选择要提交的文件".into();
                                } else {
                                    send(
                                        &this.tx,
                                        Action::Ai("commit".into(), String::new(), repo, files),
                                    );
                                }
                                cx.notify();
                            }))
                            .child(button(cx, if self.push_after_commit { "☑ 提交后同时推送" } else { "□ 提交后同时推送" }, |this, _, _, cx| { this.push_after_commit = !this.push_after_commit; cx.notify(); }))
                            .child(button(cx, "提交选中文件", |this, _, _, cx| {
                                let path = this.selected_path.clone();
                                let files = this.selected_files.clone();
                                let message = this.commit_input.clone();
                                if !files.is_empty() && !message.trim().is_empty() {
                                    this.run_git(
                                        "commit",
                                        json!({"path":path,"files":files,"message":message,"pushAfterCommit":this.push_after_commit}),
                                    );
                                    this.commit_input.clear();
                                } else {
                                    this.error = "请先选择文件并输入 commit message".into();
                                }
                                cx.notify();
                            })),
                    ),
            )
    }
}

fn button(
    cx: &Context<AppView>,
    label: impl Into<SharedString> + 'static,
    action: impl Fn(&mut AppView, &ClickEvent, &mut Window, &mut Context<AppView>) + 'static,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(SharedString::from(format!("button-{label}")))
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(LINE))
        .bg(rgb(PANEL))
        .text_color(rgb(INK))
        .text_sm()
        .cursor_pointer()
        .on_click(cx.listener(action))
        .child(label)
}
fn open_terminal(path: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        Command::new("x-terminal-emulator")
            .current_dir(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("wt").arg("-d").arg(path).spawn().map(|_| ())
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let indicator = if self.error.is_empty() { GREEN } else { RED };
        let message = if self.error.is_empty() {
            if self.status.is_empty() {
                "本地服务".to_owned()
            } else {
                self.status.clone()
            }
        } else {
            self.error.clone()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(INK))
            .text_sm()
            .text_size(px(self.settings.font_size.clamp(10, 24) as f32))
            .child(
                div()
                    .h(px(38.))
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .bg(rgb(PANEL))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .on_mouse_down(MouseButton::Left, |_, window, _| {
                                window.start_window_move()
                            })
                            .gap_3()
                            .child(
                                div()
                                    .w(px(30.))
                                    .h(px(26.))
                                    .rounded_lg()
                                    .bg(rgb(0x23384a))
                                    .text_color(rgb(PANEL))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("M"),
                            )
                            .child(div().flex().flex_col().child(
                                div().font_weight(gpui::FontWeight::BOLD).child("MulitGit"),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button(cx, "—", |_, _, window, _| {
                                window.minimize_window()
                            }))
                            .child(button(cx, "□", |_, _, window, _| window.zoom_window()))
                            .child(button(cx, "×", |_, _, window, _| window.remove_window())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(rgb(indicator)))
                    .px_5()
                    .py_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(message),
                    )
                    .child(button(cx, "刷新", |this, _, _, _| {
                        send(&this.tx, Action::Refresh)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(10.))
                    .p(px(12.))
                    .child(self.render_sidebar(cx))
                    .child(self.render_files(cx))
                    .child(self.render_right(cx)),
            )
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _window, cx| this.handle_key(event, cx)))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                let bounds = window.window_bounds().get_bounds();
                let size = bounds.size;
                let p = event.position;
                let edge = if p.x <= px(5.) && p.y <= px(5.) {
                    Some(ResizeEdge::TopLeft)
                } else if p.x >= size.width - px(5.) && p.y <= px(5.) {
                    Some(ResizeEdge::TopRight)
                } else if p.x <= px(5.) && p.y >= size.height - px(5.) {
                    Some(ResizeEdge::BottomLeft)
                } else if p.x >= size.width - px(5.) && p.y >= size.height - px(5.) {
                    Some(ResizeEdge::BottomRight)
                } else if p.x <= px(5.) {
                    Some(ResizeEdge::Left)
                } else if p.x >= size.width - px(5.) {
                    Some(ResizeEdge::Right)
                } else if p.y <= px(5.) {
                    Some(ResizeEdge::Top)
                } else if p.y >= size.height - px(5.) {
                    Some(ResizeEdge::Bottom)
                } else {
                    None
                };
                if let Some(edge) = edge {
                    window.start_window_resize(edge);
                }
            })
    }
}

fn main() {
    let base = env::var("MULITGIT_URL").unwrap_or_else(|_| "http://127.0.0.1:3210".into());
    let (tx, rx) = mpsc::channel::<Action>();
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let worker_tx = event_tx.clone();
    let worker_base = base.clone();
    thread::Builder::new()
        .name("mulitgit-api".into())
        .spawn(move || worker(rx, worker_tx, worker_base))
        .expect("API thread");
    send(&tx, Action::Refresh);
    let view_rx = event_rx;
    Application::new().run(move |cx: &mut App| {
        let display_size = cx
            .primary_display()
            .map(|display| display.bounds().size)
            .unwrap_or(size(px(1440.), px(900.)));
        let bounds = Bounds::centered(
            None,
            size(
                px(1280.).min(display_size.width - px(80.)),
                px(820.).min(display_size.height - px(80.)),
            ),
            cx,
        );
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    is_movable: true,
                    is_resizable: true,
                    window_min_size: Some(size(px(700.), px(460.))),
                    #[cfg(target_os = "linux")]
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| AppView {
                        tx: tx.clone(),
                        base_url: base.clone(),
                        roots: vec![],
                        repos: vec![],
                        repo_sort: 2,
                        selected_path: String::new(),
                        selected_file: String::new(),
                        selected_files: vec![],
                        file_data: None,
                        diff_lines: vec![],
                        preview_lines: vec![],
                        compare_left: vec![],
                        compare_right: vec![],
                        diff_measure_row: None,
                        preview_measure_row: None,
                        file_error: String::new(),
                        tab: Tab::Diff,
                        status: "正在连接…".into(),
                        error: String::new(),
                        commit_input: String::new(),
                        settings: Settings::default(),
                        history: vec![],
                        history_scope: HistoryScope::Repository,
                        history_more: false,
                        history_offset: 0,
                        history_selected: None,
                        history_diff: String::new(),
                        history_truncated: false,
                        compare_view: true,
                        file_filter: String::new(),
                        push_after_commit: false,
                        show_api_key: false,
                        ai_text: String::new(),
                        chat_messages: vec![],
                        chat_input: String::new(),
                        input_target: None,
                        focus: cx.focus_handle(),
                    })
                },
            )
            .expect("open GPUI window");
        let _ = window;
        cx.spawn(async move |cx| {
            let mut last_refresh = std::time::Instant::now();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let events: Vec<_> = view_rx.try_iter().collect();
                if !events.is_empty() {
                    let _ = window.update(cx, |view, _window, cx| {
                        let mut changed = false;
                        for event in events {
                            changed |= view.on_event(event);
                        }
                        if changed {
                            cx.notify();
                        }
                    });
                }
                if last_refresh.elapsed() >= Duration::from_secs(5) {
                    let _ = tx.send(Action::Refresh);
                    last_refresh = std::time::Instant::now();
                }
            }
        })
        .detach();
    });
}
