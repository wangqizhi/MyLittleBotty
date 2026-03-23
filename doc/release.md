# Release History

## 2026-03-23

- Added chatbot inbound image support for Telegram, Feishu, and Weixin.
- Added `ai.provider.<profile>.vision` routing, runtime image-profile selection, and direct multimodal requests when the active profile already supports vision.
- Added the built-in `image` skill for the split-provider path, so an image-capable provider can summarize image content and OCR text before forwarding the result to the active provider.
- Added a 4-second pending-image merge window so follow-up text can be combined with an inbound image into one request.
- Added Weixin inbound image download and decrypt flow based on CDN `encrypt_query_param` plus AES-128-ECB, and introduced `chatbot.weixin.cdn_base` with default `https://novac2c.cdn.weixin.qq.com/c2c`.

## 2026-03-18

- `/setup` now supports multiple AI provider profiles. AI profiles are managed in an overlay panel with create, edit, activate, and delete actions; deleting the active profile is blocked until another profile is activated.
- AI setup config now supports `ai.provider.active` plus named `ai.provider.<profile>.*` entries, while remaining backward-compatible with legacy single-provider `ai.provider.*` config files.
- Added a dedicated GLM provider adapter for `https://open.bigmodel.cn/api/anthropic`. BigModel Anthropic-compatible endpoints are now routed to `provider-glm`, and live verification succeeded with `glm-4.7`.

## 2026-03-11

- Added hardcoded system-crond tasks inside `Botty-crond`, plus `system-crond(-dev).log`. The first built-in task runs `/remember` automatically once per hour.
- Added role-specific experience memory files, `/remember` updates for `coder` and `info-searcher`, and role prompt injection from `memory/summary/experience/<role>-exp.md`.

## 2026-03-10

- Added queue-based delegated task recovery. Parent jobs can now pause on tool delegation, resume after child completion, and be inspected with `mylittlebotty watchjobs`.
- Added the built-in `terminal` skill, `coder` role, ACP terminal session management, and PTY-backed coding-agent execution inside the configured work dir.
- Added `mylittlebotty watchapp -n terminal` to inspect live terminal-agent transcripts from the newest running terminal session.
- Added the built-in `browser` skill, `info-searcher` role, Chrome remote-debugging sessions, Telegram screenshot attachment forwarding, and configurable persistent Chrome profile directories.

## 2026-03-09

- Released `0.0.9`.
- Added role-based `Botty-Guy` execution. The default `leader` role can delegate focused tasks to `paperwork` and `all-in-one` subprocess roles with reduced context.
- Added the built-in `leader` skill and role-specific system prompts, so task routing and role behavior are now explicit instead of implicit.
- Added `mylittlebotty log` and role-aware debug log rendering, making it easier to distinguish leader traffic from delegated workers.
- Added persisted `Botty-Guy` env editing and configurable `work_dir` migration in the TUI.
