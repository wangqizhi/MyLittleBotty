# MyLittleBotty

[中文说明 / Chinese README](./README_ZN.md)

MyLittleBotty is a local AI assistant that runs as a background service. Its current architecture is built around `Botty-Boss` as the supervisor daemon, `Botty-Guy` as the chat worker, and `Botty-crond` as the reminder scheduler. The current implementation focuses on local chat, TUI-based setup, Telegram/Feishu message integration, reminder scheduling, self-update, and process management.

## Recent Updates

- `2026-03-09`: released `0.0.9`.
- `2026-03-09`: added role-based `Botty-Guy` execution. The default `leader` role can delegate focused tasks to `paperwork` and `all-in-one` subprocess roles with reduced context.
- `2026-03-09`: added the built-in `leader` skill and role-specific system prompts, so task routing and role behavior are now explicit instead of implicit.
- `2026-03-09`: added `mylittlebotty log` and role-aware debug log rendering, making it easier to distinguish leader traffic from delegated workers.
- `2026-03-09`: added persisted `Botty-Guy` env editing and configurable `work_dir` migration in the TUI.

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
- `/restart-server`: restart local Botty background services
- `/new`: start a new chat session
- `/remember`: trigger long-term memory summarization
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

Testing status:

- Only MiniMax has been tested in practice.
- OpenAI-compatible and Anthropic adapters exist in code, but have not been verified in real use yet.

Runtime behavior depends on these config keys:

- `ai.provider.endpoint`
- `ai.provider.apikey`
- `ai.provider.model`
- `ai.provider.debug`

When `ai.provider.debug=true`, request and response payloads are written to the debug log.

### 4. Role-based agents

`Botty-Guy` now runs with role-specific capability bundles:

- `leader`: default role. Keeps memory context, handles reminder coordination, and can delegate work to other roles.
- `paperwork`: a focused role for document-style tasks with reduced context.
- `all-in-one`: a fallback execution role that can use the general built-in tools directly.

The delegation flow is implemented through a built-in `leader` skill:

- the leader chooses a target role
- it spawns a new `Botty-Guy` subprocess with `BOTTY_GUY_ROLE=<role>`
- it forwards only the minimal task context instead of the full chat history
- it returns the delegated result back to the original conversation

Additional design notes are documented in `doc/role-agent.md`.

### 5. Local tool usage

Botty currently exposes six built-in tools, though role access differs:

- `list`: list the content of a local directory. Directories are suffixed with `/`, symlinks with `@`, and access can be restricted with `~/.mylittlebotty/config/list.conf` via `list.blacklist=...`. The default blacklist blocks `~/.mylittlebotty/`.
- `watch`: read a local file. Text files are truncated to at most 16 KiB, large files over 500 KiB return only the recent tail, binary files return a printable preview, and access can be restricted with `~/.mylittlebotty/config/watch.conf` via `watch.blacklist=...`.
- `write`: write or append text to a local file, automatically creating parent directories when needed. It is strictly rooted at Botty's configured work dir, which defaults to `~/opt/mylittlebotty-workdir` and can be changed through setup. Any user-provided path is treated as a path under that root, so even absolute-looking inputs are remapped inside the work dir instead of writing to arbitrary host locations.
- `remember`: when `memory/summary/remember.md` and the recent conversation context are not enough, Botty first asks the model to extract a few high-signal search keywords from the current user topic, then searches `~/.mylittlebotty/memory/deep` with local text search and returns matching lines with surrounding context for the model to continue reasoning.
- `crond`: query, create, and edit reminder records stored in `~/.mylittlebotty/reminder.rec`.
- `leader`: available to the `leader` role only. It delegates a task to another role-specific `Botty-Guy` subprocess with minimal context.

### 6. Scheduled reminders

- Reminder data is stored in `~/.mylittlebotty/reminder.rec`.
- `Botty-crond` polls and executes due reminders.
- Reminders support `once`, `every_minute`, `every_hour`, `every_day`, `every_week`, and `every_month`.
- Recurring reminders can be limited with `window_start` / `window_end`, for example "every day during 2026" or "every hour for one month".
- The only actually implemented task type is `ask_guy`.
- `run_script` is currently only a reserved task shape and does not execute scripts yet.
- One-time reminders are marked as `done` after execution. Recurring reminders stay active until their time window ends, then are marked as `done`.
- If Telegram or Feishu output is enabled, reminder results are pushed back to those channels.

### 7. Telegram / Feishu integration

Two input channels are currently implemented:

- Telegram polling and reply
- Feishu polling and reply

Testing status:

- Telegram is the primary implemented channel.
- Feishu is currently best treated as a placeholder integration and has not been fully tested yet.

Supported behavior:

- Telegram user whitelist
- Configurable Telegram / Feishu polling interval
- Feishu `chat_id` targeting
- Incoming external messages are forwarded to local `Botty-Guy`

### 8. Long-term memory summary

- `/remember` triggers long-term memory summarization.
- The summary is written to `~/.mylittlebotty/memory/summary/remember.md`.
- The last summary checkpoint is written to `~/.mylittlebotty/memory/summary/rec.time`.
- During normal chat, Botty first relies on `memory/summary/remember.md` and recent conversation history.
- If the current topic does not appear there, the built-in `remember` tool may extract search keywords with the model and then search `~/.mylittlebotty/memory/deep` locally, returning matching snippets with surrounding context.

### 9. Self-update

- `mylittlebotty update` checks the latest GitHub release.
- If a newer version exists, it prompts for confirmation, downloads it, and replaces the local binary.
- If services are already running, it prompts to stop them first and restarts them after the upgrade.

## Not Implemented Yet

- `mylittlebotty webui`: entry exists, but the frontend is not implemented
- `mylittlebotty app`: entry exists, but the frontend is not implemented
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

This command summarizes recent debug and boss logs. Debug output now includes `role=...`, so delegated role traffic can be distinguished from leader traffic.

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

Common meanings:

- `ai.provider.endpoint`: model API endpoint
- `ai.provider.apikey`: model API key
- `ai.provider.model`: model name
- `ai.provider.debug`: enable request/response debug logging
- `work_dir`: root directory used by the `write` tool; changing it from the TUI migrates existing work dir content
- `chatbot.provider`: selected chatbot provider, currently `telegram` or `feishu`
- `chatbot.telegram.enabled`: enable Telegram input worker
- `chatbot.feishu.enabled`: enable Feishu input worker placeholder
- `chatbot.telegram.whitelist_user_ids`: comma-separated Telegram user IDs allowed to access the bot
- `chatbot.feishu.chat_id`: target Feishu chat ID placeholder

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

### Internal flags

These flags are mainly used by the supervisor and are not intended for normal manual use:

| Flag | Purpose | Notes |
| --- | --- | --- |
| `--boss-daemon` | Run `Botty-Boss` supervisor in the foreground | Usually spawned automatically by `mylittlebotty` |
| `--guy` | Start the `Botty-Guy` chat worker | Internal process |
| `--crond` | Start the `Botty-crond` scheduler | Internal process |
| `--input-telegram` | Start the Telegram polling worker | Internal process |
| `--input-feishu` | Start the Feishu polling worker | Internal process |

## Runtime Paths and Data Files

The program uses these paths by default:

- `~/.mylittlebotty/bin`: installed executable
- `~/.mylittlebotty/config/setup.conf`: main config file
- `~/.mylittlebotty/config/guy-env.conf`: persisted environment variables injected into `Botty-Guy`
- `~/.mylittlebotty/config/list.conf`: optional blacklist config for the `list` tool
- `~/.mylittlebotty/config/watch.conf`: optional blacklist config for the `watch` tool
- `~/.mylittlebotty/log/`: log directory
- `~/.mylittlebotty/run/`: runtime files such as pid, socket, and interrupt flags
- `~/.mylittlebotty/run/guy-role-map*.conf`: runtime mapping from spawned `Botty-Guy` pid to role
- `~/.mylittlebotty/reminder.rec`: reminder records
- `~/.mylittlebotty/memory/summary/remember.md`: long-term memory summary

In development runs, some generated config, runtime, or log files use a `-dev` suffix. Normal production use does not add `-dev`.

## Recommended Flow

1. Install or build the program.
2. Run `mylittlebotty`.
3. Run `mylittlebotty tui`.
4. Open `/setup` in the TUI and finish provider/channel configuration.
5. Save the config and continue using the TUI, Telegram, or Feishu.
