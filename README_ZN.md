# MyLittleBotty

MyLittleBotty 是一个本地常驻的 AI 助手程序，核心由 `Botty-Boss` 守护进程、`Botty-Guy` 对话执行进程、`Botty-crond` 定时提醒进程组成。当前版本主要提供本地聊天、TUI 配置、Telegram/飞书消息接入、提醒调度、版本更新和进程管理能力。

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
- `/exit`：退出 TUI。
- `/quit`：退出 TUI。

### 3. AI Provider 调用

当前代码已支持以下 Provider 适配：

- OpenAI 兼容接口
- Anthropic
- MiniMax

实际使用依赖配置文件中的：

- `ai.provider.endpoint`
- `ai.provider.apikey`
- `ai.provider.model`
- `ai.provider.debug`

当 `ai.provider.debug=true` 时，会把请求和响应写入调试日志。

### 4. 本地工具能力

当前 Botty 已接入两个内置工具：

- `watch`：读取本地文本文件内容，适合“查看某个文件”的请求。
- `crond`：查询、创建、编辑提醒任务。

### 5. 定时提醒

- 提醒数据保存在 `~/.mylittlebotty/reminder.rec`。
- `Botty-crond` 会轮询到期提醒并执行。
- 当前已真正实现的任务类型是 `ask_guy`。
- `run_script` 目前仅保留字段，尚未真正执行脚本。
- 执行完成后会把提醒状态改为 `done`。
- 如果启用了 Telegram/飞书推送，提醒结果会回发到对应聊天渠道。

### 6. Telegram / 飞书接入

当前实现了两个输入通道：

- Telegram 轮询收消息并回消息
- 飞书群聊轮询收消息并回消息

支持能力：

- Telegram 用户白名单
- Telegram / 飞书轮询间隔配置
- 飞书 chat_id 指定
- 接收到外部消息后转发给本地 `Botty-Guy` 处理

### 7. 长期记忆摘要

- 通过 `/remember` 触发整理长期记忆。
- 摘要结果写入 `~/.mylittlebotty/memory/summary/remember.md`。
- 最近整理时间写入 `~/.mylittlebotty/memory/summary/rec.time`。

### 8. 自更新

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
- `chatbot.provider`：当前聊天渠道，代码中支持 `telegram` 或 `feishu`
- `chatbot.telegram.enabled`：是否启用 Telegram 输入通道
- `chatbot.feishu.enabled`：是否启用飞书输入通道
- `chatbot.telegram.whitelist_user_ids`：Telegram 允许访问的用户 ID，多个值用逗号分隔
- `chatbot.feishu.chat_id`：飞书目标会话 ID

修改完配置后，TUI 保存时会自动触发一次服务重启。

## CLI 参数与作用

下面是当前 `src/main.rs` 中实际实现的全部 CLI 入口。

### 用户可直接使用的命令

| 命令 | 作用 | 用法 |
| --- | --- | --- |
| `mylittlebotty` | 启动后台守护进程 `Botty-Boss` | `mylittlebotty` |
| `mylittlebotty help` | 显示 CLI 帮助 | `mylittlebotty help` |
| `mylittlebotty version` | 输出版本号 | `mylittlebotty version` |
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
- `~/.mylittlebotty/log/`：日志目录
- `~/.mylittlebotty/run/`：pid、socket、flag 等运行时文件
- `~/.mylittlebotty/reminder.rec`：提醒任务记录
- `~/.mylittlebotty/memory/summary/remember.md`：长期记忆摘要

在 debug 构建下，部分运行时文件会带 `-dev` 后缀。

## 当前推荐使用流程

1. 安装或本地编译程序
2. 运行 `mylittlebotty`
3. 执行 `mylittlebotty tui`
4. 在 TUI 中输入 `/setup` 完成模型和聊天渠道配置
5. 保存配置后继续在 TUI 中聊天，或接入 Telegram / 飞书使用
