# Chatbot Provider Plugin 开发说明

本项目已经把输入型 chatbot（`Botty-input-*`）的通用流程抽象为插件运行框架。

## 目标
新增一个供应商时，复用同一套链路：

1. 轮询供应商消息
2. 转发给 leader Guy
3. 把 leader 回复回发到供应商

## 通用流程
统一循环在 `src/botty/botty-guy.rs` 的 `run_input_provider_loop`：

- 拉取消息（`fetch_messages`）
- 去重与首轮历史消息跳过
- 通过插件接口获取 `user_id`（`user_id(message)`）
- 通过插件接口校验用户权限（`is_user_allowed(user_id)`），不通过时统一回复 `非法用户`
- 拼接前缀（`provider: <text>`）
- 通过 `ask_leader_guy` 发给 Boss/leader
- 调用 `send_reply` 回发

## 插件接口
在 `src/botty/botty-guy.rs` 实现 trait：

- `ChatbotProviderPlugin`
- `provider_name(&self)`
- `poll_interval(&self)`
- `fetch_messages(&mut self)`
- `user_id(&self, message)`
- `is_user_allowed(&self, user_id)`（可选覆盖，默认允许）
- `send_reply(&mut self, target, text)`

消息统一结构：`InboundMessage`。

## 已有参考实现

- Telegram: `TelegramProviderPlugin`
- Feishu: `FeishuProviderPlugin`

这两个插件都复用了同一个 `run_input_provider_loop`。

## 配置约定
建议至少提供以下键：

- `chatbot.<provider>.enabled`
- `chatbot.<provider>.api_base`
- `chatbot.<provider>.apikey`
- `chatbot.<provider>.poll_interval_seconds`

如果供应商还需要额外路由信息（例如 Feishu 需要 `chat_id`），再加：

- `chatbot.<provider>.chat_id`

## 启动入口与进程注册

1. 在 `src/main.rs` 增加入口参数（如 `--input-xxx`）并调用 `run_xxx_input()`。
2. 在 `src/botty/botty-boss.rs` 的 `input_process_specs()` 注册：
   - 进程名（`Botty-input-xxx`）
   - 启动参数（`--input-xxx`）
   - 启用条件（读取配置判断）

Boss 会统一按 `input_process_specs` 启动/停止这些输入进程。

## 最小新增模板

1. 在 `botty-guy.rs` 增加 `XxxProviderPlugin` 并实现 `ChatbotProviderPlugin`。
2. 在 `run_xxx_input()` 中加载配置并创建插件实例。
3. 在插件里实现 `user_id(message)`；若有白名单需求，实现 `is_user_allowed(user_id)`。
4. 调用 `run_input_provider_loop(&mut plugin)`。
5. 在 `main.rs` + `botty-boss.rs` 完成入口与注册。

完成后即拥有与 Telegram/Feishu 一样的输入对话能力。
