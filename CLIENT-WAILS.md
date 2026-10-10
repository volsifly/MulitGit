# Wails 桌面客户端

MulitGit 同时提供 Web 服务和 Wails 桌面客户端。两个入口共享 `internal/workbench` 中的 Git、监控、AI、历史与终端逻辑，以及 `web/` 中的 Vue 界面。

## 运行桌面版

构建后的程序位于 `build/bin/mulitgit-desktop`。直接运行即可，不需要启动 Web 服务或配置 PM2。桌面版使用系统标题栏，可以拖动、缩放、最小化和关闭窗口。

桌面版在进程内部启动仅监听 `127.0.0.1` 的服务，端口自动分配，所有请求需要本次运行生成的令牌。Web 版的 3210 端口可同时运行。退出桌面窗口后，本地服务、仓库刷新任务和终端连接一起关闭。

首次启动会从 Web 版配置导入监控目录和模型设置，之后独立保存到操作系统用户配置目录的 `mulitgit-desktop/config.json`，避免多个实例相互覆盖。Linux 对应 `~/.config/mulitgit-desktop/config.json`。

Linux 下可以运行 `npm run install:desktop` 安装到用户目录，并在应用菜单中搜索 **MulitGit** 打开。

## 开发与构建

安装 Node.js、Go 和 Wails CLI：

```sh
go install github.com/wailsapp/wails/v2/cmd/wails@v2.15.0
export PATH="$(go env GOPATH)/bin:$PATH"
npm ci
```

Ubuntu/Debian 需要 GTK 3 和 WebKitGTK 开发依赖：

```sh
sudo apt install build-essential pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev
npm run build:desktop -- -tags webkit2_41
./build/bin/mulitgit-desktop
```

开发模式：

```sh
npm run dev:desktop -- -tags webkit2_41
```

Windows、macOS 使用对应平台的构建环境并运行 `npm run build:desktop`。macOS 需要 Xcode Command Line Tools，Windows 需要 WebView2；Windows 安装程序可通过 Wails 的 `-nsis` 选项生成。版本和系统依赖见 [Wails 安装文档](https://wails.io/docs/gettingstarted/installation/)。Linux 构建产物依赖目标系统的 GTK 3／WebKitGTK 4.1 运行库，不是完全静态的程序。

## 功能

共用现有仓库树、变更选择、Diff、代码／Markdown／图片预览、历史、AI 对话、提交与推送。桌面版“添加监控目录”使用原生目录选择窗口，内置终端继续复用 xterm 和 Go PTY，可运行 Codex CLI。

Linux 和 macOS 支持内置终端；当前 Windows 终端仍受已有后端限制，需要后续实现 ConPTY。桌面端单实例运行，再次打开会显示已有窗口。

## Web 版

现有命令保持可用：

```sh
npm run build:all
./mulitgit
```

Vite 输出目录改为 `internal/webui/dist`，两个 Go 入口嵌入同一份构建产物。桌面端的连接信息通过 Wails 原生绑定初始化；浏览器中继续使用相对路径访问原有 API。
