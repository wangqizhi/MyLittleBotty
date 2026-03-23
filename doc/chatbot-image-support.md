# Chatbot Image Support

## Goal

Chatbot image support is now implemented for `Telegram`、`Feishu`、`Weixin`.

The runtime behavior is:

1. Chatbot accepts inbound image messages from the three providers.
2. If an image arrives, Botty waits a short window for follow-up text from the same user/session.
3. If follow-up text arrives in time, image and text are merged into one request.
4. If only an image arrives, Botty uses a default image-only prompt.
5. If the active text provider and image provider are different, Botty first calls the image provider to summarize the image, extract visible text, and infer likely intent, then forwards that text result to the active provider.
6. If the active text provider and image provider are the same, Botty sends the image directly as a multimodal request to the active provider.
7. If no image-capable provider is configured, chatbot replies:

`暂不支持图像识别，请配置支持图像的 provider。`

## Provider Selection Rules

Image routing follows the existing `vision=true` profile selection rules:

1. `ai.provider.active` selects the active text profile.
2. If the active profile has `vision=true`, it is also the image profile.
3. Otherwise Botty uses the first non-active profile with `vision=true` as the image profile.
4. If no profile has `vision=true`, image requests are rejected with the unsupported message above.

Relevant runtime code:

1. `src/botty/botty-brain.rs`
2. `src/frontend/frontend_service.rs`

## End-To-End Flow

### 1. Inbound Provider Parsing

Each provider converts inbound media into a shared shape:

```rust
struct InboundMessage {
    message_id: String,
    target: String,
    user_id: String,
    text: String,
    images: Vec<InboundImage>,
}

struct InboundImage {
    kind: String,
    local_path: Option<String>,
    mime_type: Option<String>,
}
```

The image file is downloaded to a local temp path before it enters the Botty pipeline.

### 2. Pending Image Wait Window

`src/botty/botty-guy.rs` keeps a pending image buffer inside `run_input_provider_loop`.

Current behavior:

1. Image-only inbound messages are buffered for `4` seconds.
2. If a matching text message arrives before timeout, Botty merges text into the pending image request.
3. If timeout expires first, Botty enqueues an image-only request.
4. The merged request is encoded as an internal message envelope with prefix:

`__botty_image_request__`

### 3. Input Parsing In BottyBody

`src/botty/botty-body.rs` parses the internal image envelope and chooses one of two paths:

1. `active == image provider`
   Send a multimodal `ProviderMessage::User { parts }` with:
   - one text part
   - one or more `ImageBase64` parts
2. `active != image provider`
   Call built-in `image` skill first, then forward the image analysis result as plain text to the active provider

If the user sends only an image, the default prompt is:

`总结图片的内容，并猜测用户的需求。`

## Built-In Image Skill

When the active provider and image provider differ, Botty automatically loads built-in skill `image`.

Relevant files:

1. `src/skill/buildin-image.rs`
2. `src/skill/mod.rs`
3. `src/botty/botty-body.rs`

Skill responsibilities:

1. Read local image files from inbound temp paths
2. Encode them as base64
3. Call `BottyBrain::from_image_setup()`
4. Ask the image provider to:
   - summarize image content
   - extract visible text
   - infer likely user intent

The active provider then receives a forwarded text prompt:

1. If user also sent text:
   - include the user request
   - include the image analysis result
   - ask active provider to continue handling the request
2. If user only sent an image:
   - include the image analysis result
   - tell active provider that no extra text was provided
   - ask active provider to infer likely intent and help directly

## Multimodal Provider Messages

`src/llm_provider/mod.rs` now supports structured multimodal user messages.

Current content types:

```rust
pub enum ProviderContentPart {
    Text(String),
    ImageBase64 {
        media_type: String,
        data: String,
    },
}
```

Implemented provider adapters:

1. `src/llm_provider/provider-openai.rs`
2. `src/llm_provider/provider-anthropic.rs`
3. `src/llm_provider/provider-glm.rs`
4. `src/llm_provider/provider-minimax.rs`

## Provider-Specific Image Inbound Flow

### Telegram

Relevant file:

1. `src/infra/chatbot-telegram.rs`

Flow:

1. `getUpdates` is parsed with `serde_json`
2. `text` comes from `message.text` or `message.caption`
3. `photo` array is inspected
4. Botty chooses the largest photo variant
5. Botty calls Telegram `getFile`
6. Botty downloads the image to local temp storage
7. The resulting `local_path + mime_type` are attached to `InboundMessage.images`

Notes:

1. Telegram is the simplest path because the Bot API exposes a direct file download flow.
2. Captions are merged into the same inbound message, so image + caption is already one request before the 4-second pending merge logic.

### Feishu

Relevant file:

1. `src/infra/chatbot-feishu.rs`

Flow:

1. Long-connection events accept both `text` and `image` messages
2. For image messages, Botty parses `image_key` or compatible key fields from message content JSON
3. Botty uses Feishu message resources API:

`/im/v1/messages/{message_id}/resources/{image_key}?type=image`

4. The file is downloaded to local temp storage
5. The downloaded image is attached to `InboundMessage.images`

Notes:

1. Feishu text-only behavior stays unchanged.
2. Image messages without text still flow through the shared pending-image wait window.

### Weixin

Relevant file:

1. `src/infra/chatbot-weixin.rs`

Weixin differs from Telegram and Feishu. It does not reliably expose a direct image URL for inbound media.

Current implemented flow follows the protocol used by `@tencent-weixin/openclaw-weixin`:

1. Parse `item_list`
2. Find `image_item` or `pic_item`
3. Prefer encrypted CDN media fields:
   - `image_item.media.encrypt_query_param`
   - `image_item.aeskey`
   - `image_item.media.aes_key`
4. Build CDN download URL:

`{cdn_base}/download?encrypted_query_param=...`

5. Download encrypted bytes from Weixin CDN
6. If AES key exists, decrypt with `AES-128-ECB`
7. Save plaintext image bytes to local temp storage
8. Attach the resulting local image path to `InboundMessage.images`

Fallbacks still kept in code:

1. If a provider payload already contains `base64`, Botty writes it directly
2. If a real downloadable URL exists, Botty can still use the older direct-download path

Weixin config now uses two different base URLs:

1. `chatbot.weixin.api_base`
   Default: `https://ilinkai.weixin.qq.com`
2. `chatbot.weixin.cdn_base`
   Default: `https://novac2c.cdn.weixin.qq.com/c2c`

Notes:

1. `api_base` is for `getupdates` / `sendmessage`
2. `cdn_base` is for media upload/download
3. Weixin image extraction errors are isolated per message and should not crash the whole fetch loop

## Temp File Strategy

All three providers currently download images into:

`$TMPDIR/mylittlebotty-chatbot-images`

This is shared temp storage used by the inbound image pipeline.

Current status:

1. Files are created as needed
2. There is no cleanup strategy yet

## API Key Handling For Local Image Providers

`src/botty/botty-brain.rs` has one extra runtime rule for image providers:

1. Public endpoints still require non-empty API keys
2. Local or private HTTP endpoints do not require API keys

Current no-key exceptions include:

1. `http://localhost`
2. `http://127.0.0.1`
3. `http://[::1]`
4. `http://10.x.x.x`
5. `http://192.168.x.x`
6. `http://172.16.x.x` to `http://172.31.x.x`

This is mainly for local OCR / vision models such as LAN-hosted OpenAI-compatible services.

## Current Limitations

The current implementation is intentionally narrow:

1. First-class path is still single-request image understanding
2. Multi-image batching is not specially optimized, even though the shared message format already uses `Vec`
3. Temp image files are not automatically cleaned up
4. Weixin image support depends on current CDN/AES protocol assumptions and may need small field adjustments if the upstream gateway changes

## Main Code Files

Core routing:

1. `src/botty/botty-body.rs`
2. `src/botty/botty-brain.rs`
3. `src/botty/botty-guy.rs`

Built-in skill:

1. `src/skill/buildin-image.rs`
2. `src/skill/mod.rs`

Provider adapters:

1. `src/infra/chatbot-telegram.rs`
2. `src/infra/chatbot-feishu.rs`
3. `src/infra/chatbot-weixin.rs`

LLM provider multimodal serialization:

1. `src/llm_provider/mod.rs`
2. `src/llm_provider/provider-openai.rs`
3. `src/llm_provider/provider-anthropic.rs`
4. `src/llm_provider/provider-glm.rs`
5. `src/llm_provider/provider-minimax.rs`
