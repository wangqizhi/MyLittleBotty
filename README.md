# MyLittleBotty

[中文说明 / Chinese README](./README_ZN.md)

MyLittleBotty is a local AI assistant that runs as a background service. Its current architecture is built around `Botty-Boss` as the supervisor daemon, `Botty-Guy` as the chat worker, and `Botty-crond` as the reminder scheduler. The current implementation focuses on local chat, TUI-based setup, Telegram/Feishu message integration, reminder scheduling, self-update, and process management.

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

### 4. Local tool usage

Botty currently exposes two built-in tools:

- `watch`: read a local text file when the user asks to inspect a file
- `crond`: query, create, and edit reminder records

### 5. Scheduled reminders

- Reminder data is stored in `~/.mylittlebotty/reminder.rec`.
- `Botty-crond` polls and executes due reminders.
- The only actually implemented task type is `ask_guy`.
- `run_script` is currently only a reserved task shape and does not execute scripts yet.
- Completed reminders are marked as `done`.
- If Telegram or Feishu output is enabled, reminder results are pushed back to those channels.

### 6. Telegram / Feishu integration

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

### 7. Long-term memory summary

- `/remember` triggers long-term memory summarization.
- The summary is written to `~/.mylittlebotty/memory/summary/remember.md`.
- The last summary checkpoint is written to `~/.mylittlebotty/memory/summary/rec.time`.

### 8. Self-update

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
- `~/.mylittlebotty/log/`: log directory
- `~/.mylittlebotty/run/`: runtime files such as pid, socket, and interrupt flags
- `~/.mylittlebotty/reminder.rec`: reminder records
- `~/.mylittlebotty/memory/summary/remember.md`: long-term memory summary

In debug builds, some runtime files use a `-dev` suffix.

## Recommended Flow

1. Install or build the program.
2. Run `mylittlebotty`.
3. Run `mylittlebotty tui`.
4. Open `/setup` in the TUI and finish provider/channel configuration.
5. Save the config and continue using the TUI, Telegram, or Feishu.
