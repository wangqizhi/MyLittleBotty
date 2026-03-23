# MyLittleBotty

[中文说明 / Chinese README](./README_ZN.md)

MyLittleBotty is a local AI assistant that runs as a background service. Its current architecture is built around `Botty-Boss` as the supervisor daemon, `Botty-Guy` as the chat worker, and `Botty-crond` as the reminder and system scheduler. The current implementation focuses on local chat, TUI-based setup, Telegram/Feishu/Weixin message integration, image-aware chatbot routing, reminder scheduling, built-in system cron tasks, self-update, and process management.

## Recent Updates

- `2026-03-23`: chatbot inbound image support is now implemented for Telegram, Feishu, and Weixin. Botty waits briefly for follow-up text, routes image requests through `vision=true` AI profiles, uses a built-in `image` skill when image and active providers differ, and supports direct multimodal requests when they are the same.

Older release notes are tracked in [doc/release.md](./doc/release.md).

## Implemented Features

### 1. Background service

- Running `mylittlebotty` starts the `Botty-Boss` daemon in the background.
- Re-running the command does not start a duplicate daemon.
- You can manage the service with `status`, `stop`, and `restart`.

### 2. TUI chat interface

- `mylittlebotty tui` starts the terminal chat interface.
- The TUI connects to the local `Botty-Boss` automatically.
- It supports normal chat input and AI replies.
- It supports command suggestions and basic session controls.
- `Ctrl+C` interrupts the active request while a reply is in progress.

Built-in TUI commands:

- `/setup`: open the setup editor for AI provider and chatbot settings
- `AI profiles` in `/setup`: manage multiple named AI provider profiles from an overlay panel; supports create, edit, activate, and delete
- `/restart-server`: restart local Botty background services
- `/new`: start a new chat session
- `/remember`: trigger long-term memory summarization and refresh all configured role-specific experience memory files
- `/set-guy-env`: open the TUI editor for persisted `Botty-Guy` environment variables
- `/set-guy-env KEY=VALUE`: validate the key, save the variable to disk, and try to hot-apply it to the running `Botty-Guy` process
- `/list-guy-env`: open a read-only list page for persisted `Botty-Guy` environment variables
- `/exit`: exit the TUI
- `/quit`: exit the TUI

### 3. AI provider integration

The code currently supports these provider adapters:

- OpenAI-compatible API
- Anthropic
- MiniMax
- GLM Anthropic-compatible endpoint (`open.bigmodel.cn/api/anthropic`)

Testing status:

- Only MiniMax has been tested in practice.
- GLM Anthropic-compatible access has been verified in practice with `glm-4.7`.
- OpenAI-compatible and generic Anthropic adapters still exist in code, but have not been broadly verified in real use yet.

Runtime behavior depends on these config keys:

- `ai.provider.active`
- `ai.provider.<profile>.endpoint`
- `ai.provider.<profile>.apikey`
- `ai.provider.<profile>.model`
- `ai.provider.<profile>.debug`
- `ai.provider.<profile>.vision`

The loader remains backward-compatible with legacy single-provider keys such as `ai.provider.endpoint`, `ai.provider.apikey`, `ai.provider.model`, and `ai.provider.debug`.

When the active profile has `debug=true`, request and response payloads are written to the debug log.

### 4. Role-based agents

`Botty-Guy` now runs with role-specific capability bundles:

- `leader`: default role. Keeps memory context, handles reminder coordination, and can delegate work to other roles.
- `paperwork`: a focused role for document-style tasks with reduced context.
- `all-in-one`: a fallback execution role that can use the general built-in tools directly.
- `info-searcher`: a browser-driven role for webpage navigation, web research, and information extraction.
- `coder`: a coding-focused role that delegates repository execution to the built-in `terminal` skill and an external terminal agent such as Codex CLI.

Role-specific experience memory is currently enabled for:

- `coder`: stores active project records such as `项目名:... 项目路径:... 项目简介:...`
- `info-searcher`: stores stable mappings such as `应用名:... url地址:... leader称呼:...`

The delegation flow is implemented through a built-in `leader` skill:

- the leader chooses a target role
- it spawns a new `Botty-Guy` subprocess with `BOTTY_GUY_ROLE=<role>`
- it forwards only the minimal task context instead of the full chat history
- it returns the delegated result back to the original conversation

Additional design notes are documented in `doc/role-agent.md`.
Role memory rules are documented in `doc/role-memory.md`.

### 5. Local tool usage

Botty currently exposes seven built-in tools, though role access differs:

- `list`: list the content of a local directory. Directories are suffixed with `/`, symlinks with `@`, and access can be restricted with `~/.mylittlebotty/config/list.conf` via `list.blacklist=...`. The default blacklist blocks `~/.mylittlebotty/`.
- `watch`: read a local file. Text files are truncated to at most 16 KiB, large files over 500 KiB return only the recent tail, binary files return a printable preview, and access can be restricted with `~/.mylittlebotty/config/watch.conf` via `watch.blacklist=...`.
- `write`: write or append text to a local file, automatically creating parent directories when needed. It is strictly rooted at Botty's configured work dir, which defaults to `~/opt/mylittlebotty-workdir` and can be changed through setup. Any user-provided path is treated as a path under that root, so even absolute-looking inputs are remapped inside the work dir instead of writing to arbitrary host locations.
- `remember`: when `memory/summary/remember.md` and the recent conversation context are not enough, Botty first asks the model to extract a few high-signal search keywords from the current user topic, then searches `~/.mylittlebotty/memory/deep` with local text search and returns matching lines with surrounding context for the model to continue reasoning.
- `crond`: query, create, and edit reminder records stored in `~/.mylittlebotty/reminder.rec`.
- `leader`: available to the `leader` role only. It delegates a task to another role-specific `Botty-Guy` subprocess with minimal context.
- `terminal`: available to the `coder` role and custom roles that bind it. It can start or continue a PTY-backed coding-agent session, inspect status/transcripts, interrupt, terminate, restart, or list active sessions.
- `browser`: available to the `info-searcher` role and custom roles that bind it. It can start or reuse a Chrome remote-debugging session, navigate pages, inspect snapshots, click, fill, evaluate page JavaScript, wait for elements, and capture screenshots.

### 6. Delegated task queues and recovery

- Delegated work is now persisted as queue jobs per role instead of only in memory.
- A parent role can enter a waiting state while a delegated child role runs.
- When the child finishes, Botty resumes the parent's pending tool call automatically with the child's result or error text.
- `mylittlebotty watchjobs` prints the current queue snapshot once.
- `mylittlebotty watchjobs -f` clears and refreshes the queue view once per second.
- Queue output shows per-role counts for `queued`, `running`, `waiting`, `done`, and `failed`, plus the current job and up to five queued jobs.

### 7. Scheduled reminders

- Reminder data is stored in `~/.mylittlebotty/reminder.rec`.
- `Botty-crond` polls and executes due reminders.
- `Botty-crond` also runs hardcoded system tasks that are built into the binary and start automatically with the service.
- `mylittlebotty crond` prints the current reminders whose status is still `pending`.
- `mylittlebotty crond -list` is kept as a compatible alias for the same pending-only view.
- `mylittlebotty crond -list -a` shows all reminders, including `done` ones.
- Reminders support `once`, `every_minute`, `every_hour`, `every_day`, `every_week`, and `every_month`.
- Recurring reminders can be limited with `window_start` / `window_end`, for example "every day during 2026" or "every hour for one month".
- Implemented reminder task types are `ask_guy` and `assign_tasks`.
- `assign_tasks` first pushes a short "task started" notice, then hands `task_text` to leader so it can delegate to the appropriate role and finally pushes the result back through the normal reminder reply flow.
- `run_script` is currently only a reserved task shape and does not execute scripts yet.
- One-time reminders are marked as `done` after execution. Recurring reminders stay active until their time window ends, then are marked as `done`.
- If Telegram or Feishu output is enabled, reminder results are pushed back to those channels.
- The first built-in system task is `remember-hourly`, which triggers `/remember` once per hour.
- Built-in system task execution is logged to `~/.mylittlebotty/log/system-crond.log` and deduplicated across restarts with `~/.mylittlebotty/run/system-crond-state.json`.

### 8. Telegram / Feishu / Weixin integration

Three input channels are currently implemented:

- Telegram polling and reply
- Feishu long-connection receive and reply
- Weixin polling and reply

Support status:

- Telegram is supported for polling, reply, whitelist control, reminder push, and inbound image handling.
- Feishu is supported for long-connection message receive, in-chat reply, proactive push to a configured chat, and inbound image handling.
- Weixin is supported for polling, in-chat reply with preserved `context_token`, whitelist control, and inbound image handling through the current CDN/AES media flow.

Supported behavior:

- Telegram user whitelist
- Weixin user whitelist
- Configurable Telegram / Feishu / Weixin polling interval
- Feishu `chat_id` targeting for proactive push such as reminders
- Incoming external messages are forwarded to local `Botty-Guy` and replies are sent back to the source chat
- Telegram / Feishu / Weixin inbound images are downloaded to local temp storage before entering the shared Botty image pipeline
- If a user sends an image, Botty waits `4` seconds for follow-up text; otherwise it handles the image alone
- If the active AI profile and image profile differ, Botty first calls the built-in `image` skill with the image-capable provider, then forwards the summarized result to the active provider
- If the active profile also has `vision=true`, Botty sends the image directly as a multimodal request to the active provider

### 9. Long-term memory summary

- `/remember` triggers long-term memory summarization.
- The summary is written to `~/.mylittlebotty/memory/summary/remember.md`.
- The last summary checkpoint is written to `~/.mylittlebotty/memory/summary/rec.time`.
- The summary should explicitly preserve and highlight anything the user clearly said Botty should remember for future conversations.
- During normal chat, Botty first relies on `memory/summary/remember.md` and recent conversation history.
- `/remember` also updates every configured role experience file under `~/.mylittlebotty/memory/summary/experience/`.
- When a role starts and its `~/.mylittlebotty/memory/summary/experience/<role>-exp.md` exists, that file is injected into the role's system prompt.
- If the current topic does not appear there, the built-in `remember` tool may extract search keywords with the model and then search `~/.mylittlebotty/memory/deep` locally, returning matching snippets with surrounding context.

### 10. Terminal coding agent integration

- The `terminal` skill starts a PTY session under `~/.mylittlebotty/app/terminal/sessions/<session_id>/`.
- Each session stores a `session.json` metadata file and a `transcript.log` output log.
- The default terminal agent provider is `codex`; `claude` is reserved in config but not yet wired into the workflow beyond process spawning.
- Before using the `codex` provider, `codex login status` must succeed.
- Codex sessions are started with sandbox mode `workspace-write`, approval mode `never`, and `-C <work_dir>`, so the agent works inside Botty's configured work dir.
- `mylittlebotty watchapp -n terminal` continuously renders the latest running terminal session transcript.

### 11. Browser agent integration

- The `browser` skill starts a Chrome/Chromium session under `~/.mylittlebotty/app/browser/sessions/<session_id>/`.
- Each session stores a `session.json` metadata file, a `transcript.log`, and screenshots under the session directory unless an explicit output path is provided.
- The default launch mode is visible Chrome with `--remote-debugging-port`; if a page requires login, CAPTCHA, or MFA, the user can finish it in that Chrome window and the same session can continue.
- `browser.chrome.user_data_dir` can be set to a persistent Chrome profile directory so login state survives across Botty browser sessions.
- When a reply includes a browser screenshot attachment marker, Telegram output sends the image with `sendPhoto` instead of only returning a file path in text.

### 12. Self-update

- `mylittlebotty update` checks the latest GitHub release.
- If a newer version exists, it prompts for confirmation, downloads it, and replaces the local binary.
- If services are already running, it prompts to stop them first and restarts them after the upgrade.

## Not Implemented Yet

- `mylittlebotty webui`: entry exists, but the frontend is not implemented
- `mylittlebotty app`: entry exists, but the frontend is not implemented
- `crond` `assign_tasks`: executes a scheduled leader task and supports downstream role delegation at trigger time
- `crond` `run_script`: schema exists, but execution is not implemented

## Install

Install the latest release binary:

```bash
curl -LsSf https://raw.githubusercontent.com/wangqizhi/MyLittleBotty/main/startup/install.sh | bash && source ~/.zshrc
```

Notes:

- The install script currently targets macOS.
- The binary is installed to `~/.mylittlebotty/bin`.
- The script appends that directory to your shell `PATH`.

For local development you can also build directly:

```bash
cargo build --release
./target/release/mylittlebotty
```

## Uninstall

```bash
curl -LsSf https://raw.githubusercontent.com/wangqizhi/MyLittleBotty/main/startup/uninstall.sh | bash
```

Notes:

- Removes `~/.mylittlebotty`
- Removes the `PATH` lines previously added by the installer

## Basic Usage

### 1. Start the background service

```bash
mylittlebotty
```

Expected output:

- On first start: `Botty-Boss started as daemon`
- If already running: `Botty-Boss is already running, skip duplicate start`

### 2. Open the TUI chat

```bash
mylittlebotty tui
```

Inside the TUI you can:

- chat directly
- run `/setup` to configure AI and chatbot channels
- run `/set-guy-env` to open the env editor, or `/set-guy-env KEY=VALUE` to update one variable directly
- run `/list-guy-env` to inspect persisted `Botty-Guy` env vars
- run `/remember` to summarize long-term memory
- run `/quit` or `/exit` to leave the TUI

### 3. Check process status

```bash
mylittlebotty status
```

This prints:

- whether Boss is running
- Boss PID list
- Guy process count and PIDs
- Crond process count and PIDs

### 4. Stop or restart services

Stop:

```bash
mylittlebotty stop
```

Restart:

```bash
mylittlebotty restart
```

### 5. Check for updates

```bash
mylittlebotty update
```

This is interactive. It will ask:

- whether to continue the upgrade
- whether running services should be stopped before upgrading

### 6. Show version

```bash
mylittlebotty version
```

### 7. Show help

```bash
mylittlebotty help
```

You can also use:

```bash
mylittlebotty --help
mylittlebotty -h
```

### 8. Inspect logs

```bash
mylittlebotty log
```

Optional follow mode:

```bash
mylittlebotty log -f
```

This command summarizes recent debug, boss, and system-crond logs. Debug output now includes `role=...`, so delegated role traffic can be distinguished from leader traffic, and built-in system task runs can be inspected from the same command.

### 9. Inspect delegated job queues

One-shot snapshot:

```bash
mylittlebotty watchjobs
```

Continuous refresh:

```bash
mylittlebotty watchjobs -f
```

This view is useful when the leader is delegating to `paperwork`, `all-in-one`, `coder`, or custom sub-agents.

### 10. Inspect terminal coding sessions

```bash
mylittlebotty watchapp -n terminal
```

This continuously shows the transcript tail of the newest running terminal-agent session.

### 11. Inspect pending reminders

Default view:

```bash
mylittlebotty crond
```

Compatible alias:

```bash
mylittlebotty crond -list
```

This command prints only reminders whose status is still `pending`.

Show all reminders:

```bash
mylittlebotty crond -list -a
```

## Configuration

The simplest way to configure the app is from the TUI:

```text
/setup
```

The config file is stored at:

```text
~/.mylittlebotty/config/setup.conf
```

Current config keys:

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
browser.chrome.max_tabs=10
work_dir=
chatbot.provider=telegram
ai.provider.default.vision=false
chatbot.telegram.api_base=https://api.telegram.org
chatbot.telegram.apikey=
chatbot.feishu.api_base=https://open.feishu.cn/open-apis
chatbot.feishu.app_id=
chatbot.feishu.app_secret=
chatbot.weixin.api_base=https://ilinkai.weixin.qq.com
chatbot.weixin.cdn_base=https://novac2c.cdn.weixin.qq.com/c2c
chatbot.weixin.apikey=
chatbot.telegram.enabled=true
chatbot.feishu.enabled=false
chatbot.weixin.enabled=false
chatbot.telegram.whitelist_user_ids=
chatbot.weixin.whitelist_user_ids=
chatbot.telegram.poll_interval_seconds=1
chatbot.feishu.poll_interval_seconds=1
chatbot.weixin.poll_interval_seconds=1
chatbot.weixin.long_poll_timeout_ms=35000
chatbot.feishu.chat_id=
```

Common meanings:

- `ai.provider.active`: active AI profile name used at runtime
- `ai.provider.<profile>.endpoint`: model API endpoint for a named profile
- `ai.provider.<profile>.apikey`: model API key for a named profile
- `ai.provider.<profile>.model`: model name for a named profile
- `ai.provider.<profile>.debug`: enable request/response debug logging for a named profile
- `ai.provider.<profile>.vision`: whether this profile can be selected for image understanding
- `agent.provider`: terminal-agent provider used by the `terminal` skill, currently `codex` by default
- `agent.codex.command`: executable name or path for Codex CLI
- `agent.claude.command`: executable name or path reserved for a Claude terminal agent
- `browser.chrome.command`: executable name or path for Chrome/Chromium; leave empty to auto-detect
- `browser.chrome.headless`: whether the browser skill launches Chrome in headless mode by default
- `browser.chrome.user_data_dir`: optional persistent Chrome user-data directory; relative paths are resolved under `~/.mylittlebotty/`
- `browser.chrome.max_tabs`: max number of Chrome page tabs to keep when connecting to CDP; if the count exceeds this limit, extra tabs are closed automatically; set `0` to disable the limit
- `work_dir`: root directory used by the `write` tool; changing it from the TUI migrates existing work dir content
- `chatbot.provider`: enabled chatbot providers, currently `telegram`, `feishu`, and `weixin`
- `chatbot.telegram.enabled`: enable Telegram input worker
- `chatbot.telegram.apikey`: Telegram bot token
- `chatbot.feishu.enabled`: enable Feishu input worker
- `chatbot.feishu.app_id`: Feishu app ID used to obtain a tenant access token
- `chatbot.feishu.app_secret`: Feishu app secret used to obtain a tenant access token
- `chatbot.weixin.enabled`: enable Weixin input worker
- `chatbot.weixin.api_base`: Weixin bot API base used for `getupdates` and `sendmessage`
- `chatbot.weixin.cdn_base`: Weixin CDN base used for media upload/download, default `https://novac2c.cdn.weixin.qq.com/c2c`
- `chatbot.weixin.apikey`: Weixin `bot_token`
- `chatbot.telegram.whitelist_user_ids`: comma-separated Telegram user IDs allowed to access the bot
- `chatbot.weixin.whitelist_user_ids`: comma-separated Weixin user IDs allowed to access the bot
- `chatbot.weixin.long_poll_timeout_ms`: Weixin long-poll timeout for `getupdates`
- `chatbot.feishu.chat_id`: target Feishu chat ID for proactive push such as reminders

Notes:

- The TUI `AI profiles` manager supports create, edit, activate, and delete.
- You cannot delete the currently active AI profile; activate another one first.
- The config loader is backward-compatible with legacy single-provider `ai.provider.*` keys.
- For GLM Claude-compatible access, use `https://open.bigmodel.cn/api/anthropic` with a model such as `glm-4.7`.
- If no `vision=true` profile exists, chatbot image requests return `暂不支持图像识别，请配置支持图像的 provider。`
- For private/local HTTP model endpoints such as `localhost`, `127.0.0.1`, `10.x`, `192.168.x`, or `172.16-31.x`, Botty does not force a non-empty API key.

Saving from the TUI automatically triggers a service restart.

## CLI Arguments

The table below reflects everything currently implemented in `src/main.rs`.

### User-facing commands

| Command | Purpose | Usage |
| --- | --- | --- |
| `mylittlebotty` | Start the `Botty-Boss` background daemon | `mylittlebotty` |
| `mylittlebotty help` | Show CLI help | `mylittlebotty help` |
| `mylittlebotty version` | Print the current version | `mylittlebotty version` |
| `mylittlebotty log` | Show recent runtime and debug logs | `mylittlebotty log` |
| `mylittlebotty crond` | Show reminders whose status is still `pending`; `mylittlebotty crond -list -a` shows all | `mylittlebotty crond` |
| `mylittlebotty watchjobs` | Inspect delegated role job queues | `mylittlebotty watchjobs` |
| `mylittlebotty watchapp` | Inspect app output, currently terminal sessions only | `mylittlebotty watchapp -n terminal` |
| `mylittlebotty status` | Show service status and PID information | `mylittlebotty status` |
| `mylittlebotty stop` | Stop Botty-related processes | `mylittlebotty stop` |
| `mylittlebotty restart` | Restart background services | `mylittlebotty restart` |
| `mylittlebotty update` | Check for updates and perform self-update | `mylittlebotty update` |
| `mylittlebotty tui` | Start the TUI frontend | `mylittlebotty tui` |
| `mylittlebotty webui` | Reserved WebUI entry, not implemented yet | `mylittlebotty webui` |
| `mylittlebotty app` | Reserved app frontend entry, not implemented yet | `mylittlebotty app` |

Short help flags:

- `mylittlebotty -h`
- `mylittlebotty --help`
- `mylittlebotty crond -list`
- `mylittlebotty crond -list -a`

### Internal flags

These flags are mainly used by the supervisor and are not intended for normal manual use:

| Flag | Purpose | Notes |
| --- | --- | --- |
| `--boss-daemon` | Run `Botty-Boss` supervisor in the foreground | Usually spawned automatically by `mylittlebotty` |
| `--guy` | Start the `Botty-Guy` chat worker | Internal process |
| `--crond` | Start the `Botty-crond` scheduler | Internal process |
| `--input-telegram` | Start the Telegram polling worker | Internal process |
| `--input-feishu` | Start the Feishu polling worker | Internal process |
| `--input-weixin` | Start the Weixin polling worker | Internal process |

## Runtime Paths and Data Files

The program uses these paths by default:

- `~/.mylittlebotty/bin`: installed executable
- `~/.mylittlebotty/config/setup.conf`: main config file
- `~/.mylittlebotty/config/guy-env.conf`: persisted environment variables injected into `Botty-Guy`
- `~/.mylittlebotty/config/list.conf`: optional blacklist config for the `list` tool
- `~/.mylittlebotty/config/watch.conf`: optional blacklist config for the `watch` tool
- `~/.mylittlebotty/app/terminal/sessions/`: PTY-backed terminal agent session metadata and transcripts
- `~/.mylittlebotty/log/`: log directory
- `~/.mylittlebotty/run/`: runtime files such as pid, socket, and interrupt flags
- `~/.mylittlebotty/log/system-crond.log`: built-in system task execution log
- `~/.mylittlebotty/run/system-crond-state.json`: last successful/executed schedule slot per built-in system task
- `~/.mylittlebotty/run/guy-role-map*.conf`: runtime mapping from spawned `Botty-Guy` pid to role
- `~/.mylittlebotty/run/jobs/`: persisted delegated job queues and worker state by role
- `~/.mylittlebotty/reminder.rec`: reminder records
- `~/.mylittlebotty/memory/summary/remember.md`: long-term memory summary
- `~/.mylittlebotty/memory/summary/experience/`: role-specific experience memory files
- `~/.mylittlebotty/memory/summary/experience/coder-exp.md`: coder role memory
- `~/.mylittlebotty/memory/summary/experience/info-searcher-exp.md`: info-searcher role memory

In development runs, some generated config, runtime, or log files use a `-dev` suffix. Normal production use does not add `-dev`.

Built-in system-crond implementation notes are documented in `doc/system-crond.md`.

## Recommended Flow

1. Install or build the program.
2. Run `mylittlebotty`.
3. Run `mylittlebotty tui`.
4. Open `/setup` in the TUI and finish provider/channel configuration.
5. Save the config and continue using the TUI, Telegram, Feishu, or Weixin.
