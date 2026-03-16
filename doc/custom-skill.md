# `/create-skill` 使用说明

`/create-skill` 会打开一个单独的编辑页，不会和聊天输入框混在一起。

## 1. 交互方式

在 TUI 里输入：

```text
/create-skill
```

随后会进入创建页，分两步输入：

- 第一步：输入 skill 名字
- 第二步：描述这个 skill 要做什么

保存快捷键：

- `Ctrl+G`：自动生成 skill 简介预览
- `Ctrl+S`：保存
- `Esc`：取消

## 2. 生成位置

创建成功后，会自动生成：

```text
~/.mylittlebotty/skill/<name>.json
```

其中 `<name>` 会自动规范化：

- 转成小写
- 空格、下划线、斜杠会转成 `-`
- 连续分隔符会折叠

## 3. 自动生成内容

TUI 会自动补全这些字段：

- `name`
- `description`
- `usage`
- 默认单字段 `input_schema`
- `action: "prompt"`
- `prompt_template`

也就是说，现在不需要手工填写旧的 5 个字段表单，只写“名字 + 做什么”即可。

创建页会同时展示这些预览信息：

- 规范化后的 skill 名字
- 自动生成的 skill 简介
- 最终目标文件路径

## 4. 注意事项

- 生成的是旧 custom skill JSON，不是 `.codex/skills/.../SKILL.md`
- 如果同名文件已存在，`~/.mylittlebotty/skill/<name>.json` 会被覆盖
