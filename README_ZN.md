# MyLittleBotty

MyLittleBotty 是一个本地常驻的 AI 助手程序，核心由 `Botty-Boss` 守护进程、`Botty-Guy` 对话执行进程、`Botty-crond` 定时提醒进程组成。当前版本主要提供本地聊天、TUI 配置、Telegram/飞书消息接入、提醒调度、版本更新和进程管理能力。

## 最近更新

- `2026-03-09`：发布 `0.0.9`。
- `2026-03-09`：新增基于角色的 `Botty-Guy` 运行方式。默认 `leader` 角色可以把任务分派给 `paperwork` 和 `all-in-one` 子角色，并尽量缩小传递上下文。
- `2026-03-09`：新增内置 `leader` skill 与角色专用系统提示词，任务分派和各角色职责变成显式机制。
- `2026-03-09`：新增 `mylittlebotty log` 命令，并在调试日志里展示 `role=...`，更容易区分 leader 与被分派子进程。
- `2026-03-09`：TUI 新增 `Botty-Guy` 持久化环境变量编辑，以及 `work_dir` 配置和迁移能力。

## 当前已实现功能

### 1. 本地常驻服务

- 直接运行 `mylittlebotty` 会启动 `Botty-Boss` 后台守护进程。
- 已启动时再次执行不会重复启动。
- 可通过 `status`、`stop`、`restart` 管理后台服务。

### 2. TUI 聊天界面

- `mylittlebotty tui` 启动终端聊天界面。
- TUI 会自动连接本地 `Botty-Boss`。
- 支持输入普通消息与 AI 对话。
- 支持命令补全和基础会话控制。
- 支持在请求处理中用 `Ctrl+C` 中断当前请求。

TUI 内置命令：

- `/setup`：进入配置界面，编辑 AI Provider 和聊天机器人配置。
- `/restart-server`：重启本地 Botty 后台服务。
- `/new`：开始新会话。
- `/remember`：触发长期记忆摘要整理。
- `/set-guy-env`：打开 `Botty-Guy` 持久化环境变量的 TUI 编辑页。
- `/set-guy-env KEY=VALUE`：校验变量名后落盘，并尝试热更新到当前运行中的 `Botty-Guy` 进程。
- `/list-guy-env`：打开 `Botty-Guy` 已持久化环境变量的只读列表页。
- `/exit`：退出 TUI。
- `/quit`：退出 TUI。

### 3. AI Provider 调用

当前代码已支持以下 Provider 适配：

- OpenAI 兼容接口
- Anthropic
- MiniMax

测试状态：

- 实际只测试过 MiniMax。
- OpenAI 兼容接口和 Anthropic 目前只是代码里有适配，尚未经过真实使用验证。

实际使用依赖配置文件中的：

- `ai.provider.endpoint`
- `ai.provider.apikey`
- `ai.provider.model`
- `ai.provider.debug`

当 `ai.provider.debug=true` 时，会把请求和响应写入调试日志。

### 4. 角色化 Agent 能力

`Botty-Guy` 现在会按角色装配能力：

- `leader`：默认角色，保留记忆上下文，负责提醒协调和任务分派。
- `paperwork`：面向文书类任务的轻量角色，使用更小的上下文执行。
- `all-in-one`：兜底执行角色，可以直接使用通用内置工具。

分派能力由内置 `leader` skill 实现：

- leader 判断任务适合哪个角色
- 通过 `BOTTY_GUY_ROLE=<role>` 拉起新的 `Botty-Guy` 子进程
- 只把最小必要任务信息发给子进程，而不是整段历史对话
- 再把子进程结果回传给原始会话

更详细的设计说明见 `doc/role-agent.md`。

### 5. 本地工具能力

当前 Botty 已接入六个内置工具，但不同角色可用的工具不同：

- `list`：列出本地目录内容，目录会追加 `/`，符号链接会追加 `@`。可通过 `~/.mylittlebotty/config/list.conf` 中的 `list.blacklist=...` 配置访问黑名单，默认禁止访问 `~/.mylittlebotty/`。
- `watch`：读取本地文件内容。文本文件最多返回 16 KiB，大于 500 KiB 的大文件只返回最近一段尾部内容，二进制文件返回可打印片段预览。可通过 `~/.mylittlebotty/config/watch.conf` 中的 `watch.blacklist=...` 配置访问黑名单。
- `write`：向本地文件写入或追加文本，必要时自动创建父目录。它始终以 Botty 配置的 work dir 作为根目录，默认是 `~/opt/mylittlebotty-workdir`，也可以在 setup 中修改。用户提供的任何路径都会被当成这个根目录下的路径处理，因此即使看起来是绝对路径，也不会写到宿主机任意位置。
- `remember`：当 `memory/summary/remember.md` 和最近对话上下文都不够时，Botty 会先让模型从当前用户话题里提炼少量高信号关键词，再对 `~/.mylittlebotty/memory/deep` 做本地文本搜索，并把命中行及其上下文返回给模型继续判断。
- `crond`：查询、创建、编辑保存在 `~/.mylittlebotty/reminder.rec` 中的提醒任务。
- `leader`：仅 `leader` 角色可用，用于把任务转交给其它角色专用的 `Botty-Guy` 子进程执行。

### 6. 定时提醒

- 提醒数据保存在 `~/.mylittlebotty/reminder.rec`。
- `Botty-crond` 会轮询到期提醒并执行。
- 当前已真正实现的任务类型是 `ask_guy`。
- `run_script` 目前仅保留字段，尚未真正执行脚本。
- 执行完成后会把提醒状态改为 `done`。
- 如果启用了 Telegram/飞书推送，提醒结果会回发到对应聊天渠道。

### 7. Telegram / 飞书接入

当前实现了两个输入通道：

- Telegram 轮询收消息并回消息
- 飞书群聊轮询收消息并回消息

测试状态：

- Telegram 是当前主要使用和验证过的接入方式。
- 飞书目前更适合视为占位接入，尚未完整测试。

支持能力：

- Telegram 用户白名单
- Telegram / 飞书轮询间隔配置
- 飞书 chat_id 指定
- 接收到外部消息后转发给本地 `Botty-Guy` 处理

### 8. 长期记忆摘要

- 通过 `/remember` 触发整理长期记忆。
- 摘要结果写入 `~/.mylittlebotty/memory/summary/remember.md`。
- 最近整理时间写入 `~/.mylittlebotty/memory/summary/rec.time`。
- 正常对话时，Botty 会优先使用 `memory/summary/remember.md` 和最近对话历史。
- 如果当前话题在这些内容里没有出现，内置 `remember` 工具会先让模型提炼搜索关键词，再在本地搜索 `~/.mylittlebotty/memory/deep`，并返回命中片段及其上下文。

### 9. 自更新

- `mylittlebotty update` 会检查 GitHub 最新 release。
- 如果发现新版本，会提示确认后下载并替换本地二进制。
- 如果更新前检测到服务正在运行，会提示先停止，再在更新后自动重启。

## 尚未实现或仅保留入口

- `mylittlebotty webui`：当前未实现，执行会报错。
- `mylittlebotty app`：当前未实现，执行会报错。
- `crond` 的 `run_script` 实际执行逻辑：当前未实现。

## 安装

安装最新 release：

```bash
curl -LsSf https://raw.githubusercontent.com/wangqizhi/MyLittleBotty/main/startup/install.sh | bash && source ~/.zshrc
```

说明：

- 当前安装脚本面向 macOS。
- 默认安装到 `~/.mylittlebotty/bin`。
- 安装脚本会把该目录加入 shell 的 `PATH`。

如果是本地开发，可直接使用 Cargo：

```bash
cargo build --release
./target/release/mylittlebotty
```

## 卸载

```bash
curl -LsSf https://raw.githubusercontent.com/wangqizhi/MyLittleBotty/main/startup/uninstall.sh | bash
```

说明：

- 会删除 `~/.mylittlebotty`
- 会移除安装脚本追加到 shell 配置中的 PATH 片段

## 基本使用方法

### 1. 启动后台服务

```bash
mylittlebotty
```

输出：

- 首次启动通常会显示 `Botty-Boss started as daemon`
- 已启动时会显示 `Botty-Boss is already running, skip duplicate start`

### 2. 打开 TUI 聊天

```bash
mylittlebotty tui
```

进入后可以：

- 直接输入消息聊天
- 输入 `/setup` 配置 AI 和聊天渠道
- 输入 `/set-guy-env` 打开环境变量编辑页，或用 `/set-guy-env KEY=VALUE` 直接修改单个变量
- 输入 `/list-guy-env` 查看已持久化的 `Botty-Guy` 环境变量
- 输入 `/remember` 整理长期记忆
- 输入 `/quit` 或 `/exit` 退出

### 3. 查看进程状态

```bash
mylittlebotty status
```

会输出：

- Boss 是否在运行
- Boss 进程 PID 列表
- Guy 进程数量和 PID
- Crond 进程数量和 PID

### 4. 停止或重启服务

停止：

```bash
mylittlebotty stop
```

重启：

```bash
mylittlebotty restart
```

### 5. 检查并更新版本

```bash
mylittlebotty update
```

这是交互式命令，会询问：

- 是否继续升级
- 若当前有进程在运行，是否先停止再升级

### 6. 查看版本

```bash
mylittlebotty version
```

### 7. 查看帮助

```bash
mylittlebotty help
```

也可以使用：

```bash
mylittlebotty --help
mylittlebotty -h
```

### 8. 查看日志

```bash
mylittlebotty log
```

持续跟随输出：

```bash
mylittlebotty log -f
```

这个命令会汇总最近的 debug 与 boss 日志。现在 debug 输出里会带 `role=...`，方便区分 leader 和被分派角色的流量。

## 配置方法

最简单的方式是进入 TUI 后执行：

```text
/setup
```

配置保存位置：

```text
~/.mylittlebotty/config/setup.conf
```

当前支持的配置项如下：

```ini
ai.provider.endpoint=
ai.provider.apikey=
ai.provider.model=MiniMax-M2.1
ai.provider.debug=false
work_dir=
chatbot.provider=telegram
chatbot.telegram.api_base=https://api.telegram.org
chatbot.telegram.apikey=
chatbot.feishu.api_base=https://open.feishu.cn/open-apis
chatbot.feishu.apikey=
chatbot.telegram.enabled=true
chatbot.feishu.enabled=false
chatbot.telegram.whitelist_user_ids=
chatbot.telegram.poll_interval_seconds=1
chatbot.feishu.poll_interval_seconds=1
chatbot.feishu.chat_id=
```

常见说明：

- `ai.provider.endpoint`：模型接口地址
- `ai.provider.apikey`：模型 API Key
- `ai.provider.model`：模型名
- `ai.provider.debug`：是否记录调试日志
- `work_dir`：`write` 工具使用的根目录；在 TUI 中修改时会迁移旧工作目录内容
- `chatbot.provider`：当前聊天渠道，代码中支持 `telegram` 或 `feishu`
- `chatbot.telegram.enabled`：是否启用 Telegram 输入通道
- `chatbot.feishu.enabled`：是否启用飞书输入通道占位配置
- `chatbot.telegram.whitelist_user_ids`：Telegram 允许访问的用户 ID，多个值用逗号分隔
- `chatbot.feishu.chat_id`：飞书目标会话 ID 占位配置

修改完配置后，TUI 保存时会自动触发一次服务重启。

## CLI 参数与作用

下面是当前 `src/main.rs` 中实际实现的全部 CLI 入口。

### 用户可直接使用的命令

| 命令 | 作用 | 用法 |
| --- | --- | --- |
| `mylittlebotty` | 启动后台守护进程 `Botty-Boss` | `mylittlebotty` |
| `mylittlebotty help` | 显示 CLI 帮助 | `mylittlebotty help` |
| `mylittlebotty version` | 输出版本号 | `mylittlebotty version` |
| `mylittlebotty log` | 查看最近运行日志和调试日志 | `mylittlebotty log` |
| `mylittlebotty status` | 查看后台服务状态和 PID 信息 | `mylittlebotty status` |
| `mylittlebotty stop` | 停止 Botty 相关进程 | `mylittlebotty stop` |
| `mylittlebotty restart` | 重启后台服务 | `mylittlebotty restart` |
| `mylittlebotty update` | 检查新版本并执行自更新 | `mylittlebotty update` |
| `mylittlebotty tui` | 启动 TUI 前端 | `mylittlebotty tui` |
| `mylittlebotty webui` | 预留 WebUI 入口，当前未实现 | `mylittlebotty webui` |
| `mylittlebotty app` | 预留 App 前端入口，当前未实现 | `mylittlebotty app` |

简写帮助参数：

- `mylittlebotty -h`
- `mylittlebotty --help`

### 内部参数

这些参数主要由守护进程自动拉起，不建议普通用户手动执行：

| 参数 | 作用 | 说明 |
| --- | --- | --- |
| `--boss-daemon` | 以前台方式运行 `Botty-Boss` supervisor | 通常由 `mylittlebotty` 自动派生 |
| `--guy` | 启动 `Botty-Guy` 对话执行进程 | 内部进程 |
| `--crond` | 启动 `Botty-crond` 定时提醒进程 | 内部进程 |
| `--input-telegram` | 启动 Telegram 输入轮询进程 | 内部进程 |
| `--input-feishu` | 启动飞书输入轮询进程 | 内部进程 |

## 运行目录与数据文件

程序默认使用以下目录：

- `~/.mylittlebotty/bin`：安装后的可执行文件
- `~/.mylittlebotty/config/setup.conf`：主配置文件
- `~/.mylittlebotty/config/guy-env.conf`：注入到 `Botty-Guy` 的持久化环境变量
- `~/.mylittlebotty/config/list.conf`：`list` 内置工具的可选黑名单配置
- `~/.mylittlebotty/config/watch.conf`：`watch` 内置工具的可选黑名单配置
- `~/.mylittlebotty/log/`：日志目录
- `~/.mylittlebotty/run/`：pid、socket、flag 等运行时文件
- `~/.mylittlebotty/run/guy-role-map*.conf`：运行中的 `Botty-Guy` 进程与角色映射文件
- `~/.mylittlebotty/reminder.rec`：提醒任务记录
- `~/.mylittlebotty/memory/summary/remember.md`：长期记忆摘要

在 debug 构建下，部分运行时文件会带 `-dev` 后缀。

## 当前推荐使用流程

1. 安装或本地编译程序
2. 运行 `mylittlebotty`
3. 执行 `mylittlebotty tui`
4. 在 TUI 中输入 `/setup` 完成模型和聊天渠道配置
5. 保存配置后继续在 TUI 中聊天，或接入 Telegram / 飞书使用
