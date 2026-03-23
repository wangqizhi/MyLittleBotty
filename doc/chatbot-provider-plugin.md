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
- Weixin: `WeixinProviderPlugin`

这三个插件都复用了同一个 `run_input_provider_loop`。

## 配置约定
建议至少提供以下键：

- `chatbot.<provider>.enabled`
- `chatbot.<provider>.api_base`
- `chatbot.<provider>.apikey`
- `chatbot.<provider>.poll_interval_seconds`

如果供应商还需要额外路由信息（例如 Feishu 需要 `chat_id`），再加：

- `chatbot.<provider>.chat_id`

微信个人号还需要补充一些 provider 自己的状态键：

- `chatbot.weixin.account_id`
- `chatbot.weixin.user_id`
- `chatbot.weixin.long_poll_timeout_ms`
- `chatbot.weixin.whitelist_user_ids`

说明：

- `chatbot.weixin.apikey` 存的是扫码登录后换到的 `bot_token`
- `chatbot.weixin.account_id` 对应登录成功后返回的 `ilink_bot_id`
- `chatbot.weixin.user_id` 是扫码绑定的微信用户 ID，可选
- `chatbot.weixin.long_poll_timeout_ms` 控制 `getupdates` 长轮询超时

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

## Weixin 额外注意点

个人微信的最小文本链路比 Telegram/Feishu 多两个关键状态：

1. `get_updates_buf`
2. `context_token`

推荐做法：

- `getupdates` 返回的新 `get_updates_buf` 要落盘保存，避免进程重启后从头拉历史消息
- 每条入站消息里的 `context_token` 要按 `target/user_id` 缓存在插件内部
- `sendmessage` 回复时必须原样带回对应的 `context_token`

如果缺少 `context_token`，文本回复会失败；因此 Weixin provider 一般不能只靠统一的 `InboundMessage` 字段，还需要插件内部自管上下文状态。
