use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::{
    App, AppContext as _, Application, Bounds, ClickEvent, Context, FocusHandle, KeyDownEvent,
    MouseButton, Render, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    px, rgb, size,
};
use pulldown_cmark::{Options, Parser};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
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

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ChangedFile {
    path: String,
    original_path: Option<String>,
    code: String,
    staged: bool,
    status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
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
    last_fetch: Option<String>,
    fetch_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
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
    AddRoot(String),
    RemoveRoot(usize),
    Ai(String, String, String, Vec<String>),
    AiChat(String, String, String, Vec<ChatMessage>),
    History(String),
    LoadSettings,
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
    Ai(String, String),
    History(Value),
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
    while let Ok(action) = rx.recv() {
        if let Action::File(repo, file) = &action {
            let (repo, file, client, base, tx) = (
                repo.clone(),
                file.clone(),
                client.clone(),
                base.clone(),
                tx.clone(),
            );
            thread::spawn(move || {
                let url = format!(
                    "{base}/api/file?repo={}&path={}",
                    urlencoding::encode(&repo),
                    urlencoding::encode(&file)
                );
                let event = match request::<Value>(&client, reqwest::Method::GET, url, None) {
                    Ok(data) => Event::File(repo, file, data),
                    Err(error) => Event::FileError(repo, file, error),
                };
                let _ = tx.send(event);
            });
            continue;
        }
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
                    tx.send(Event::Result(output)).map_err(|e| e.to_string())?;
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
                    tx.send(Event::Ai(task, content))
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
                    tx.send(Event::Ai("chat".into(), content))
                        .map_err(|e| e.to_string())?;
                }
                Action::History(path) => {
                    let url = format!(
                        "{base}/api/history?repo={}&offset=0",
                        urlencoding::encode(&path)
                    );
                    let data: Value = request(&client, reqwest::Method::GET, url, None)?;
                    tx.send(Event::History(data)).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = tx.send(Event::Error(error));
        }
    }
}

struct AppView {
    tx: Sender<Action>,
    roots: Vec<String>,
    repos: Vec<Repository>,
    selected_path: String,
    selected_file: String,
    selected_files: Vec<String>,
    file_data: Option<Value>,
    file_error: String,
    tab: Tab,
    status: String,
    error: String,
    commit_input: String,
    settings: Settings,
    history: Vec<Value>,
    ai_text: String,
    chat_messages: Vec<ChatMessage>,
    chat_input: String,
    input_target: Option<Tab>,
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
        self.file_error.clear();
        if !self.selected_file.is_empty() {
            send(
                &self.tx,
                Action::File(repo.path, self.selected_file.clone()),
            );
        }
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Repositories(roots, repos) => {
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
                }
            }
            Event::Selected(repo) => self.update_selected(repo),
            Event::File(repo, file, data) => {
                if repo == self.selected_path && file == self.selected_file {
                    self.file_data = Some(data);
                    self.file_error.clear();
                }
            }
            Event::FileError(repo, file, error) => {
                if repo == self.selected_path && file == self.selected_file {
                    self.file_error = error;
                }
            }
            Event::Result(text) => {
                self.status = text;
                self.error.clear();
            }
            Event::Settings(settings) => self.settings = settings,
            Event::Ai(task, text) => {
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
            Event::History(data) => {
                self.history = data["commits"].as_array().cloned().unwrap_or_default();
            }
            Event::Error(error) => self.error = error,
        }
        cx.notify();
    }

    fn select_repo(&mut self, path: String) {
        self.selected_path = path.clone();
        self.selected_file.clear();
        self.file_data = None;
        self.file_error.clear();
        self.error.clear();
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
        if self.input_target.is_none() {
            return;
        }
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => {
                self.commit_input.clear();
                self.chat_input.clear();
            }
            "backspace" => {
                if self.input_target == Some(Tab::Summary) {
                    self.chat_input.pop();
                } else if !self.commit_input.is_empty() {
                    self.commit_input.pop();
                }
            }
            "enter" => {
                if self.input_target == Some(Tab::Summary) && !self.chat_input.trim().is_empty() {
                    let message = std::mem::take(&mut self.chat_input);
                    let scope = if self.selected_file.is_empty() {
                        "all"
                    } else {
                        "file"
                    };
                    self.ask_ai(scope, message);
                } else if !self.commit_input.is_empty() {
                    self.commit_input.push('\n');
                }
            }
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
                        if self.input_target == Some(Tab::Summary) {
                            self.chat_input.push_str(ch)
                        } else if !self.selected_path.is_empty() && self.tab != Tab::Settings {
                            self.commit_input.push_str(ch)
                        }
                    }
                }
            }
        }
        cx.notify();
    }
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
        let roots = self.roots.clone();
        let repos = self.repos.clone();
        let active = self.selected_path.clone();
        div()
            .w(px(270.))
            .min_w(px(180.))
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
        let files = self
            .selected_repo()
            .map(|repo| repo.files.clone())
            .unwrap_or_default();
        let active = self.selected_file.clone();
        let selected = self.selected_path.clone();
        let checks = self.selected_files.clone();
        div()
            .w(px(325.))
            .min_w(px(210.))
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
                        let code = file.code.clone();
                        let staged = file.staged;
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
                                this.selected_file = path.clone();
                                this.file_data = None;
                                this.file_error.clear();
                                send(&this.tx, Action::File(repo.clone(), path.clone()));
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
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(if staged { GREEN } else { MUTED }))
                                            .child(status),
                                    ),
                            )
                    })),
            )
    }

    fn render_right(&self, cx: &Context<Self>) -> impl IntoElement {
        let selected_repo = self.selected_repo().cloned();
        let file = self.selected_file.clone();
        let data = self.file_data.clone();
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
                            send(&this.tx, Action::History(this.selected_path.clone()));
                        }
                        if tab == Tab::Settings {
                            send(&this.tx, Action::LoadSettings);
                        }
                        cx.notify();
                    }))
                    .child(label)
            }));
        let panel = match self.tab {
            Tab::Diff => {
                let diff = data
                    .as_ref()
                    .and_then(|value| value["diff"].as_str())
                    .unwrap_or("选择变更文件以查看 diff")
                    .to_owned();
                div()
                    .flex()
                    .flex_col()
                    .children(diff.lines().map(|line| {
                        let color = if line.starts_with('+') && !line.starts_with("+++") {
                            0x24764d
                        } else if line.starts_with('-') && !line.starts_with("---") {
                            0xb9404d
                        } else {
                            INK
                        };
                        div()
                            .font_family("monospace")
                            .text_size(px(14.))
                            .text_color(rgb(color))
                            .child(line.to_owned())
                    }))
                    .into_any_element()
            }
            Tab::Preview => {
                let preview = data
                    .as_ref()
                    .and_then(|value| value["preview"].as_str())
                    .unwrap_or("选择一个文本文件以预览内容。");
                if data
                    .as_ref()
                    .is_some_and(|d| d["binary"].as_bool() == Some(true))
                {
                    div()
                        .child("此文件是二进制文件，暂不支持文本预览。")
                        .into_any_element()
                } else if file.to_lowercase().ends_with(".md")
                    || file.to_lowercase().ends_with(".markdown")
                {
                    render_markdown(preview).into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .font_family("monospace")
                        .text_size(px(14.))
                        .children(preview.lines().map(|line| {
                            div().child(if line.is_empty() {
                                " ".to_owned()
                            } else {
                                line.to_owned()
                            })
                        }))
                        .into_any_element()
                }
            }
            Tab::Summary => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().whitespace_normal().child(self.ai_text.clone()))
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
                        .child(div().whitespace_normal().child(message.content.clone()))
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
                            this.input_target = Some(Tab::Summary);
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
                .children(self.history.iter().map(|item| {
                    div()
                        .p_2()
                        .rounded_md()
                        .border_b_1()
                        .border_color(rgb(LINE))
                        .child(
                            div().font_weight(gpui::FontWeight::SEMIBOLD).child(
                                item["subject"]
                                    .as_str()
                                    .unwrap_or("(提交说明为空)")
                                    .to_owned(),
                            ),
                        )
                        .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                            "{}  {}",
                            item["shortHash"].as_str().unwrap_or(""),
                            item["author"].as_str().unwrap_or("")
                        )))
                }))
                .into_any_element(),
            Tab::Settings => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("AI 模型设置"),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child(if self.settings.model.is_empty() {
                            "尚未配置模型，请在 Web 设置页配置后保存。".to_owned()
                        } else {
                            format!("{} · {}", self.settings.model, self.settings.base_url)
                        }),
                )
                .into_any_element(),
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
                    .child(button(cx, "↓ 拉取", |this, _, _, cx| {
                        if !this.selected_path.is_empty() {
                            this.run_git("pull", json!({"path":this.selected_path}));
                        }
                        cx.notify();
                    }))
                    .child(button(cx, "↑ 推送", |this, _, _, cx| {
                        if !this.selected_path.is_empty() {
                            this.run_git("push", json!({"path":this.selected_path}));
                        }
                        cx.notify();
                    }))
                    .child(button(cx, "检查远端", |this, _, _, cx| {
                        if !this.selected_path.is_empty() {
                            this.run_git("fetch", json!({"path":this.selected_path}));
                        }
                        cx.notify();
                    })),
            )
            .child(tabs_ui)
            .child(
                div()
                    .id("inspector-panel")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
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
                                this.input_target = Some(Tab::Diff);
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
                            .child(button(cx, "提交选中文件", |this, _, _, cx| {
                                let path = this.selected_path.clone();
                                let files = this.selected_files.clone();
                                let message = this.commit_input.clone();
                                if !files.is_empty() && !message.trim().is_empty() {
                                    this.run_git(
                                        "commit",
                                        json!({"path":path,"files":files,"message":message}),
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
    label: &'static str,
    action: impl Fn(&mut AppView, &ClickEvent, &mut Window, &mut Context<AppView>) + 'static,
) -> impl IntoElement {
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
            .child(
                div()
                    .h(px(58.))
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
                                    .h(px(30.))
                                    .rounded_lg()
                                    .bg(rgb(0x23384a))
                                    .text_color(rgb(PANEL))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("M"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div().font_weight(gpui::FontWeight::BOLD).child("MulitGit"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(MUTED))
                                            .child("RUST · GPUI CLIENT"),
                                    ),
                            ),
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
                    titlebar: Some(TitlebarOptions {
                        title: Some("MulitGit".into()),
                        ..Default::default()
                    }),
                    window_min_size: Some(size(px(900.), px(600.))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| AppView {
                        tx: tx.clone(),
                        roots: vec![],
                        repos: vec![],
                        selected_path: String::new(),
                        selected_file: String::new(),
                        selected_files: vec![],
                        file_data: None,
                        file_error: String::new(),
                        tab: Tab::Diff,
                        status: "正在连接…".into(),
                        error: String::new(),
                        commit_input: String::new(),
                        settings: Settings::default(),
                        history: vec![],
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
                for event in view_rx.try_iter() {
                    let _ = window.update(cx, |view, _window, cx| view.on_event(event, cx));
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
