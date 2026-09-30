# MulitGit

本地多仓库 Git 工作台。后端使用 Go，前端使用 Vue 3 + TypeScript；Git 状态和写操作通过系统 Git CLI 完成。

## 环境要求

- Go 1.26+
- Node.js 20+
- Git 2.30+

## 开发运行

```sh
npm install
npm run dev:server
```

另开一个终端：

```sh
npm run dev
```

访问 http://127.0.0.1:5173。Go API 默认监听 127.0.0.1:3210。

## 构建与跨平台打包

```sh
npm run build:all
./mulitgit
```

交叉构建示例（当前环境需安装 Go 和 Node）：

```sh
npm run build
GOOS=windows GOARCH=amd64 go build -o MulitGit.exe ./cmd/mulitgit
GOOS=darwin GOARCH=arm64 go build -o MulitGit-macos-arm64 ./cmd/mulitgit
GOOS=linux GOARCH=amd64 go build -o MulitGit-linux-amd64 ./cmd/mulitgit
```

Go 二进制内嵌前端静态资源。目标系统需安装 Git；macOS 签名和 Windows 安装包可在后续打包阶段补充。

Go 服务默认只监听本机回环地址。需要从局域网访问时，可显式绑定所有网卡：

```sh
MULITGIT_ADDR=0.0.0.0:3210 ./mulitgit
```

PM2 管理时可更新进程环境并保存：

```sh
MULITGIT_ADDR=0.0.0.0:3210 pm2 restart MulitGit --update-env
pm2 save
```

服务目前没有登录认证，绑定 `0.0.0.0` 会允许可访问本机端口的设备读取监控仓库并调用 Git 操作，请只在可信网络中启用。监控目录配置保存在用户配置目录的 `mulitgit/config.json` 中。目标机器需安装 Git。

## 当前进度

当前版本支持添加多个监控目录并递归发现仓库和 submodule；仓库列表可按名称、创建时间或修改时间排序。服务端每 15 秒刷新并缓存仓库状态，工作台展示变更文件和 diff，支持 Markdown 预览、代码高亮和选择文件提交。打开仓库时会检查远端更新，并允许用户确认后快进拉取。

设置兼容 OpenAI Chat Completions API 的模型后，可生成文件修改总结和 commit message；未配置模型时，会提供基于所选文件名的可编辑本地草稿。界面包含模型连接、提交模板和规范、字号及编辑器偏好设置。应用标识同时用作浏览器 favicon。

设计说明见 [DESIGN.md](DESIGN.md)。
