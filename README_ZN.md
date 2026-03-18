# MyLittleBotty

MyLittleBotty 是一个本地常驻的 AI 助手程序，核心由 `Botty-Boss` 守护进程、`Botty-Guy` 对话执行进程、`Botty-crond` 定时提醒与系统调度进程组成。当前版本主要提供本地聊天、TUI 配置、Telegram/飞书消息接入、提醒调度、内置系统定时任务、版本更新和进程管理能力。

## 最近更新

- `2026-03-18`：`/setup` 现在支持多个 AI provider profile。AI profile 在覆盖式弹层中管理，支持创建、编辑、激活和删除；当前激活的 profile 不能直接删除，必须先切换。
- `2026-03-18`：AI 配置文件现在支持 `ai.provider.active` 和命名的 `ai.provider.<profile>.*` 项，同时兼容老版本单 provider 的 `ai.provider.*` 配置格式。
- `2026-03-18`：新增 `https://open.bigmodel.cn/api/anthropic` 专用的 GLM provider 适配器。BigModel 的 Anthropic 兼容 endpoint 现在会走 `provider-glm`，并已用 `glm-4.7` 做过真实调用验证。
- `2026-03-11`：`Botty-crond` 新增硬编码的 system-crond 任务机制，并新增 `system-crond(-dev).log`；首个内置任务会每小时自动执行一次 `/remember`。
- `2026-03-11`：新增 role 专属经验记忆文件；`/remember` 现在会为 `coder` 和 `info-searcher` 同步整理 role memory，并在角色启动时自动注入对应 `memory/summary/experience/<role>-exp.md`。
- `2026-03-10`：新增基于队列的委派任务恢复机制。父任务在工具委派后可以进入等待态，子任务完成后自动恢复，并可通过 `mylittlebotty watchjobs` 观察队列状态。
- `2026-03-10`：新增内置 `terminal` skill、`coder` 角色、ACP 终端会话管理，以及基于 PTY 的编码代理执行能力，执行目录受当前 work dir 限制。
- `2026-03-10`：新增 `mylittlebotty watchapp -n terminal`，可直接查看最新运行中的 terminal 会话输出。
- `2026-03-10`：新增内置 `browser` skill、`info-searcher` 角色、基于 Chrome remote debugging 的浏览器会话、Telegram 截图附件回传，以及可配置的持久 Chrome profile 目录。
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
- `/setup` 中的 `AI profiles`：在覆盖式弹层里管理多个命名 AI profile，支持创建、编辑、激活和删除。
- `/restart-server`：重启本地 Botty 后台服务。
- `/new`：开始新会话。
- `/remember`：触发长期记忆摘要整理，并同步刷新所有已配置的 role 经验记忆。
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
- GLM 的 Anthropic 兼容 endpoint（`open.bigmodel.cn/api/anthropic`）

测试状态：

- 实际只测试过 MiniMax。
- `glm-4.7` 已在智谱 Anthropic 兼容接口上做过真实调用验证。
- OpenAI 兼容接口和通用 Anthropic 适配器目前仍然只是代码里有适配，尚未广泛验证。

实际使用依赖配置文件中的：

- `ai.provider.active`
- `ai.provider.<profile>.endpoint`
- `ai.provider.<profile>.apikey`
- `ai.provider.<profile>.model`
- `ai.provider.<profile>.debug`

配置读取仍兼容老版本单 provider 的 `ai.provider.endpoint`、`ai.provider.apikey`、`ai.provider.model`、`ai.provider.debug`。

当当前激活 profile 的 `debug=true` 时，会把请求和响应写入调试日志。

### 4. 角色化 Agent 能力

`Botty-Guy` 现在会按角色装配能力：

- `leader`：默认角色，保留记忆上下文，负责提醒协调和任务分派。
- `paperwork`：面向文书类任务的轻量角色，使用更小的上下文执行。
- `all-in-one`：兜底执行角色，可以直接使用通用内置工具。
- `info-searcher`：面向网页浏览、在线检索和网页信息提取的浏览器角色。
- `coder`：面向代码和仓库任务的角色，通过内置 `terminal` skill 驱动外部终端编码代理执行实际工作。

当前已启用 role 专属经验记忆的角色：

- `coder`：记录 `项目名:... 项目路径:... 项目简介:...`
- `info-searcher`：记录 `应用名:... url地址:... leader称呼:...`

分派能力由内置 `leader` skill 实现：

- leader 判断任务适合哪个角色
- 通过 `BOTTY_GUY_ROLE=<role>` 拉起新的 `Botty-Guy` 子进程
- 只把最小必要任务信息发给子进程，而不是整段历史对话
- 再把子进程结果回传给原始会话

更详细的设计说明见 `doc/role-agent.md`，role memory 规则见 `doc/role-memory.md`。

### 5. 本地工具能力

当前 Botty 已接入七个内置工具，但不同角色可用的工具不同：

- `list`：列出本地目录内容，目录会追加 `/`，符号链接会追加 `@`。可通过 `~/.mylittlebotty/config/list.conf` 中的 `list.blacklist=...` 配置访问黑名单，默认禁止访问 `~/.mylittlebotty/`。
- `watch`：读取本地文件内容。文本文件最多返回 16 KiB，大于 500 KiB 的大文件只返回最近一段尾部内容，二进制文件返回可打印片段预览。可通过 `~/.mylittlebotty/config/watch.conf` 中的 `watch.blacklist=...` 配置访问黑名单。
- `write`：向本地文件写入或追加文本，必要时自动创建父目录。它始终以 Botty 配置的 work dir 作为根目录，默认是 `~/opt/mylittlebotty-workdir`，也可以在 setup 中修改。用户提供的任何路径都会被当成这个根目录下的路径处理，因此即使看起来是绝对路径，也不会写到宿主机任意位置。
- `remember`：当 `memory/summary/remember.md` 和最近对话上下文都不够时，Botty 会先让模型从当前用户话题里提炼少量高信号关键词，再对 `~/.mylittlebotty/memory/deep` 做本地文本搜索，并把命中行及其上下文返回给模型继续判断。
- `crond`：查询、创建、编辑保存在 `~/.mylittlebotty/reminder.rec` 中的提醒任务。
- `leader`：仅 `leader` 角色可用，用于把任务转交给其它角色专用的 `Botty-Guy` 子进程执行。
- `terminal`：供 `coder` 角色和绑定它的自定义角色使用，可启动或继续一个基于 PTY 的编码代理会话，也可查询状态、读取 transcript、发送中断、终止、重启、列出活跃会话。
- `browser`：供 `info-searcher` 角色和绑定它的自定义角色使用，可启动或复用一个 Chrome remote-debugging 会话，执行页面打开、快照抓取、点击、填表、页面 JavaScript 执行、元素等待和截图。

### 6. 委派任务队列与恢复

- 委派任务现在会按角色持久化为本地队列，而不只是停留在内存里。
- 父角色在等待子角色执行时会进入 `waiting` 状态。
- 子任务完成或失败后，Botty 会把结果或错误文本自动回填给父任务，并恢复原来的工具调用流程。
- `mylittlebotty watchjobs` 会打印一次当前队列快照。
- `mylittlebotty watchjobs -f` 会每秒刷新一次队列视图。
- 输出中会展示每个角色的 `queued`、`running`、`waiting`、`done`、`failed` 数量，以及当前任务和最多 5 个排队任务。

### 7. 定时提醒

- 提醒数据保存在 `~/.mylittlebotty/reminder.rec`。
- `Botty-crond` 会轮询到期提醒并执行。
- `Botty-crond` 也会执行编译进程序的 system-crond 内置任务，服务启动后自动生效，不走 `reminder.rec` 配置。
- `mylittlebotty crond` 会列出当前状态仍为 `pending` 的提醒任务。
- `mylittlebotty crond -list` 作为兼容写法，效果与上面相同，也只看 `pending`。
- `mylittlebotty crond -list -a` 会显示全部提醒任务，包括已经 `done` 的。
- 提醒支持 `once`、`every_minute`、`every_hour`、`every_day`、`every_week`、`every_month`。
- 重复提醒支持 `window_start` / `window_end` 生效时间窗，例如“2026 年内每天执行”或“1 个月内每小时执行”。
- 当前已真正实现的任务类型是 `ask_guy`。
- `run_script` 目前仅保留字段，尚未真正执行脚本。
- 单次提醒执行后会变成 `done`；重复提醒会持续生效，直到超出生效时间窗后再变成 `done`。
- 如果启用了 Telegram/飞书推送，提醒结果会回发到对应聊天渠道。
- 当前第一个内置 system 任务是 `remember-hourly`，会每小时触发一次 `/remember`。
- 内置 system 任务执行日志写入 `~/.mylittlebotty/log/system-crond.log`，并通过 `~/.mylittlebotty/run/system-crond-state.json` 避免服务重启后在同一时间槽重复执行。

### 8. Telegram / 飞书接入

当前实现了两个输入通道：

- Telegram 轮询收消息并回消息
- 飞书长连接收消息并回消息

支持状态：

- Telegram 已正式支持轮询收发、用户白名单和提醒推送。
- 飞书已正式支持长连接收消息、原会话回消息，以及按配置会话主动推送。

支持能力：

- Telegram 用户白名单
- Telegram / 飞书轮询间隔配置
- 飞书 `chat_id` 可用于提醒等主动推送目标会话
- 接收到外部消息后转发给本地 `Botty-Guy` 处理，并把回复发回原会话

### 9. 长期记忆摘要

- 通过 `/remember` 触发整理长期记忆。
- 摘要结果写入 `~/.mylittlebotty/memory/summary/remember.md`。
- 最近整理时间写入 `~/.mylittlebotty/memory/summary/rec.time`。
- 正常对话时，Botty 会优先使用 `memory/summary/remember.md` 和最近对话历史。
- `/remember` 还会同步更新 `~/.mylittlebotty/memory/summary/experience/` 下所有已配置的 role 经验记忆文件。
- role 启动时，如果存在 `~/.mylittlebotty/memory/summary/experience/<role>-exp.md`，会把内容注入该 role 的 system prompt。
- 如果当前话题在这些内容里没有出现，内置 `remember` 工具会先让模型提炼搜索关键词，再在本地搜索 `~/.mylittlebotty/memory/deep`，并返回命中片段及其上下文。

### 10. Terminal 编码代理集成

- `terminal` skill 会把会话运行在 `~/.mylittlebotty/app/terminal/sessions/<session_id>/` 下。
- 每个会话会保存 `session.json` 元数据和 `transcript.log` 输出日志。
- 默认 terminal provider 是 `codex`；`claude` 已预留配置项，但目前除了进程启动入口外还没有完整接入工作流。
- 使用 `codex` provider 前，需要先确保 `codex login status` 成功。
- Codex 会话会以 `workspace-write` sandbox、`never` approval，并附带 `-C <work_dir>` 启动，因此执行范围限定在当前配置的工作目录内。
- `mylittlebotty watchapp -n terminal` 会持续渲染最新一个运行中 terminal 会话的 transcript 尾部。

### 11. Browser 浏览器代理集成

- `browser` skill 会把会话运行在 `~/.mylittlebotty/app/browser/sessions/<session_id>/` 下。
- 每个会话会保存 `session.json` 元数据、`transcript.log`，以及默认保存在会话目录里的截图文件；如果显式指定输出路径，则按指定位置保存。
- 默认会启动一个可见的 Chrome 并开启 `--remote-debugging-port`；如果页面要求登录、验证码或 MFA，用户可以直接在该 Chrome 窗口内完成，然后继续复用同一会话。
- 可通过 `browser.chrome.user_data_dir` 配置持久 Chrome user-data 目录，让登录状态跨 Botty 浏览器会话保留。
- 当回复中带有浏览器截图附件标记时，Telegram 输出会自动走 `sendPhoto` 发送图片，而不是只返回本地文件路径。

### 12. 自更新

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

这个命令会汇总最近的 debug、boss 和 system-crond 日志。现在 debug 输出里会带 `role=...`，方便区分 leader 和被分派角色的流量，也可以直接看到内置系统任务的执行情况。

### 9. 查看委派任务队列

单次查看：

```bash
mylittlebotty watchjobs
```

持续刷新：

```bash
mylittlebotty watchjobs -f
```

当 leader 正在把任务委派给 `paperwork`、`all-in-one`、`coder` 或自定义子角色时，这个视图尤其有用。

### 10. 查看 terminal 编码会话

```bash
mylittlebotty watchapp -n terminal
```

这个命令会持续显示最新运行中的 terminal 代理会话输出尾部。

### 11. 查看待执行提醒

默认写法：

```bash
mylittlebotty crond
```

兼容写法：

```bash
mylittlebotty crond -list
```

这个命令只会显示当前状态仍为 `pending` 的提醒任务。

查看全部提醒：

```bash
mylittlebotty crond -list -a
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
ai.provider.active=default
ai.provider.default.endpoint=
ai.provider.default.apikey=
ai.provider.default.model=
ai.provider.default.debug=false
agent.provider=codex
agent.codex.command=codex
agent.claude.command=claude
browser.chrome.command=
browser.chrome.headless=false
browser.chrome.user_data_dir=~/.mylittlebotty/app/browser/user_dir
work_dir=
chatbot.provider=telegram
chatbot.telegram.api_base=https://api.telegram.org
chatbot.telegram.apikey=
chatbot.feishu.api_base=https://open.feishu.cn/open-apis
chatbot.feishu.app_id=
chatbot.feishu.app_secret=
chatbot.telegram.enabled=true
chatbot.feishu.enabled=false
chatbot.telegram.whitelist_user_ids=
chatbot.telegram.poll_interval_seconds=1
chatbot.feishu.poll_interval_seconds=1
chatbot.feishu.chat_id=
```

常见说明：

- `ai.provider.active`：运行时使用的当前 AI profile 名称
- `ai.provider.<profile>.endpoint`：某个命名 profile 的模型接口地址
- `ai.provider.<profile>.apikey`：某个命名 profile 的模型 API Key
- `ai.provider.<profile>.model`：某个命名 profile 的模型名
- `ai.provider.<profile>.debug`：某个命名 profile 是否记录调试日志
- `agent.provider`：`terminal` skill 使用的终端代理提供方，当前默认是 `codex`
- `agent.codex.command`：Codex CLI 的可执行文件名或路径
- `agent.claude.command`：预留给 Claude 终端代理的可执行文件名或路径
- `browser.chrome.command`：Chrome/Chromium 的可执行文件名或路径；留空时自动探测
- `browser.chrome.headless`：`browser` skill 默认是否以 headless 模式启动 Chrome
- `browser.chrome.user_data_dir`：可选的持久 Chrome user-data 目录；相对路径会解析到 `~/.mylittlebotty/` 下
- `work_dir`：`write` 工具使用的根目录；在 TUI 中修改时会迁移旧工作目录内容
- `chatbot.provider`：当前聊天渠道，代码中支持 `telegram` 或 `feishu`
- `chatbot.telegram.enabled`：是否启用 Telegram 输入通道
- `chatbot.telegram.apikey`：Telegram 机器人 token
- `chatbot.feishu.enabled`：是否启用飞书输入通道
- `chatbot.feishu.app_id`：用于换取 tenant access token 的飞书 app id
- `chatbot.feishu.app_secret`：用于换取 tenant access token 的飞书 app secret
- `chatbot.telegram.whitelist_user_ids`：Telegram 允许访问的用户 ID，多个值用逗号分隔
- `chatbot.feishu.chat_id`：飞书主动推送目标会话 ID，例如提醒回发

补充说明：

- TUI 中的 `AI profiles` 支持创建、编辑、激活和删除。
- 当前激活的 AI profile 不能直接删除，需要先切换到其他 profile。
- 配置读取仍兼容老版本单 provider 的 `ai.provider.*` 键。
- 如果要接入智谱 Claude 兼容接口，可使用 `https://open.bigmodel.cn/api/anthropic`，模型建议填 `glm-4.7`。

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
| `mylittlebotty crond` | 查看当前仍为 `pending` 的提醒任务；`mylittlebotty crond -list -a` 可查看全部 | `mylittlebotty crond` |
| `mylittlebotty watchjobs` | 查看各角色的委派任务队列 | `mylittlebotty watchjobs` |
| `mylittlebotty watchapp` | 查看 app 输出，当前仅支持 terminal 会话 | `mylittlebotty watchapp -n terminal` |
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
- `mylittlebotty crond -list`
- `mylittlebotty crond -list -a`

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
- `~/.mylittlebotty/app/terminal/sessions/`：terminal 代理会话的元数据和 transcript
- `~/.mylittlebotty/log/`：日志目录
- `~/.mylittlebotty/run/`：pid、socket、flag 等运行时文件
- `~/.mylittlebotty/log/system-crond.log`：内置 system 任务执行日志
- `~/.mylittlebotty/run/system-crond-state.json`：各内置 system 任务上次已执行时间槽状态
- `~/.mylittlebotty/run/guy-role-map*.conf`：运行中的 `Botty-Guy` 进程与角色映射文件
- `~/.mylittlebotty/run/jobs/`：按角色保存的委派任务队列和 worker 状态
- `~/.mylittlebotty/reminder.rec`：提醒任务记录
- `~/.mylittlebotty/memory/summary/remember.md`：长期记忆摘要
- `~/.mylittlebotty/memory/summary/experience/`：role 专属经验记忆目录
- `~/.mylittlebotty/memory/summary/experience/coder-exp.md`：`coder` 角色经验记忆
- `~/.mylittlebotty/memory/summary/experience/info-searcher-exp.md`：`info-searcher` 角色经验记忆

在开发环境运行时，部分生成的配置、运行时或日志文件会带 `-dev` 后缀；正式程序不会带 `-dev`。

内置 system-crond 的实现和扩展方法见 `doc/system-crond.md`。

## 当前推荐使用流程

1. 安装或本地编译程序
2. 运行 `mylittlebotty`
3. 执行 `mylittlebotty tui`
4. 在 TUI 中输入 `/setup` 完成模型和聊天渠道配置
5. 保存配置后继续在 TUI 中聊天，或接入 Telegram / 飞书使用
