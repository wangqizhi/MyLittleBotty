# Custom Skill 使用说明

本文档说明 MyLittleBotty 里自定义 skill 的创建方式、JSON 字段含义，以及后续让 Codex 直接帮你生成 skill 时应提供的信息。

## 1. skill 放在哪里

自定义 skill 文件存放在：

```text
~/.mylittlebotty/skill/<name>.json
```

如果 `action` 是 `script`，对应脚本放在：

```text
~/.mylittlebotty/skill/scripts/<name>.sh
```

`<name>` 必须和 JSON 里的 `name` 一致。

## 2. 创建方式

有两种方式：

### 2.1 在 TUI 里创建

输入：

```text
/create-skill
```

然后填写 5 个字段：

- `Name`
- `Description`
- `Usage`
- `Action (prompt/script)`
- `Prompt Template (use {{input}})`

保存后会生成对应 JSON 文件。

### 2.2 直接手改 JSON

你也可以直接编辑：

```text
~/.mylittlebotty/skill/<name>.json
```

当前代码已经支持下一条消息自动重新加载 skill，不需要重启服务。

## 3. JSON 格式

一个完整示例：

```json
{
  "name": "smart-egg",
  "description": "Reply to the teasing phrase 你是个聪明蛋.",
  "usage": "Use this when the user says 你是个聪明蛋.",
  "input_schema": {
    "type": "object",
    "properties": {
      "input": {
        "type": "string",
        "description": "The input for this skill"
      }
    },
    "required": ["input"]
  },
  "action": "prompt",
  "prompt_template": "If the user says 你是个聪明蛋, reply exactly with: 你才是个聪明蛋"
}
```

## 4. 字段作用

### `name`

skill 的唯一标识。

用途：

- 角色通过这个名字绑定 skill
- 运行时通过这个名字找到 skill
- `script` 模式下也用它匹配脚本文件名

要求：

- 必填
- 建议只用小写字母、数字、中划线
- 应避免和内置 skill 重名

### `description`

给大模型看的 skill 描述。

用途：

- 告诉模型这个 skill 是干什么的
- 帮助模型判断要不要调用它

建议：

- 写清触发场景
- 写清 skill 产出什么结果
- 不要写空泛描述

推荐写法：

```text
Use this when the user asks for X. Return Y.
```

### `usage`

当前会被保存到 JSON，但运行时没有实际使用。

也就是说：

- TUI 会让你填
- 文件里会保存
- 当前代码不会把它发给模型，也不会影响执行

现阶段可以把它当备注字段。

### `input_schema`

定义这个 skill 接收什么 JSON 输入。

用途：

- 告诉模型调用 skill 时该传什么参数
- 约束工具调用的入参结构

当前 TUI 不提供单独编辑 `input_schema` 的界面。
如果你需要复杂参数，只能手改 JSON。

默认值是：

```json
{
  "type": "object",
  "properties": {
    "input": {
      "type": "string",
      "description": "The input for this skill"
    }
  },
  "required": ["input"]
}
```

如果你不确定，就先保持默认。

### `action`

决定 skill 的执行方式。

当前只支持两个值：

- `prompt`
- `script`

#### `action = "prompt"`

表示这个 skill 不直接执行脚本，而是把 `prompt_template` 和输入拼起来，生成一个结果字符串返回。

适合：

- 轻量规则
- 固定格式改写
- 把输入包装成明确指令

#### `action = "script"`

表示执行：

```text
~/.mylittlebotty/skill/scripts/<name>.sh
```

并把调用参数通过环境变量 `SKILL_INPUT` 传给脚本。

适合：

- 真正需要本地逻辑处理
- 调 shell 命令
- 访问外部程序

### `prompt_template`

给 `prompt` 模式用的模板。

支持占位符：

- `{{input}}`

运行时会把输入里的 `input` 字段替换进去。

例如：

```json
{
  "prompt_template": "Summarize this text in one sentence: {{input}}"
}
```

如果输入是：

```json
{"input":"hello world"}
```

则渲染结果相当于：

```text
Summarize this text in one sentence: hello world
```

## 5. skill 是怎么被使用的

skill 不是“创建完就自动全局生效”的。

它需要先被某个角色绑定，模型才有机会调用。

对自定义角色来说，角色配置里有一个：

- `skills`

这个列表里填 skill 的 `name`。

也就是说：

- 角色决定“有哪些 skill 可用”
- 模型根据 skill 的 `description` 和 `input_schema` 决定“要不要调用”
- 真正执行时用 `name` 找到 skill

## 6. prompt 型 skill 的边界

`prompt` 型 skill 更像“把输入包装成一段指令文本再返回”，不是硬编码规则引擎。

这意味着：

- 它适合辅助模型
- 不适合做必须 100% 命中的固定触发词回复

如果你想做“用户说 A，系统必须回 B”这种确定性逻辑，更适合直接写代码拦截，而不是依赖 skill。

## 7. script 型 skill 的输入输出

当 `action = "script"` 时：

- 程序会执行 `~/.mylittlebotty/skill/scripts/<name>.sh`
- 入参放在环境变量 `SKILL_INPUT`
- 脚本标准输出会作为 skill 结果返回
- 脚本非 0 退出时会视为失败

最小示例：

```sh
#!/bin/sh
echo "skill input: $SKILL_INPUT"
```

## 8. 推荐写法

### 8.1 简单文本处理

适合 `prompt`

```json
{
  "name": "rewrite-friendly",
  "description": "Rewrite the user's text into a warmer and friendlier tone.",
  "usage": "Use this when the user asks to rewrite text more warmly.",
  "action": "prompt",
  "prompt_template": "Rewrite the following text in a warm and friendly tone. Keep the meaning unchanged:\\n\\n{{input}}"
}
```

### 8.2 需要本地处理

适合 `script`

```json
{
  "name": "word-count",
  "description": "Count words from the provided input text.",
  "usage": "Use this when the user asks to count words.",
  "action": "script",
  "prompt_template": ""
}
```

对应脚本：

```sh
#!/bin/sh
printf '%s' "$SKILL_INPUT" | wc -w
```

## 9. 后续让 Codex 帮你生成 skill 时，最好直接给这些信息

你下次可以直接按这个模板提要求：

```text
帮我生成一个 custom skill：
1. skill 名字：
2. 用途：
3. 希望在哪些场景触发：
4. 希望输入是什么：
5. 希望输出是什么：
6. 用 prompt 还是 script：
7. 如果是 prompt，给出你想要的 prompt 风格：
8. 如果是 script，说明脚本要做什么：
```

如果你懒得写这么全，最少给这三项：

- skill 名字
- 触发场景
- 希望返回什么

## 10. 当前实现注意事项

- 改完 skill JSON 后，下一条消息会自动生效
- `usage` 当前只是存档字段，不参与运行
- TUI 默认只生成单字段输入 schema，也就是 `input`
- `prompt` skill 不是强规则，不保证一定被模型调用
- `script` skill 依赖本地脚本文件存在且可正常执行

