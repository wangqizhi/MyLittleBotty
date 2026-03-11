# Role Agent 定义说明

## 目标

`Botty-Guy` 现在按 `role` 装配能力。

- `leader`：负责分派任务，加载基础技能组 + `crond` + `leader`
- `paperwork`：负责文书类工作，加载基础技能组
- `all-in-one`：兜底角色，加载基础技能组 + 其它内建技能

其中 leader 会通过 `buildin-leader` 启动对应角色的子 `Botty-Guy`，并只传递最小必要信息，不带旧聊天记录和 summary，避免上下文持续膨胀。

## 角色定义位置

角色配置写在 [src/botty/botty-guy.rs](/Users/wangqizhi/Project/MyLittleBotty/src/botty/botty-guy.rs)。

核心结构：

```rust
pub(crate) struct BottyGuyRoleSpec {
    pub role: &'static str,
    pub description: &'static str,
    pub system_instruction_prompt: &'static str,
    pub skill_groups: &'static [&'static str],
    pub skills: &'static [&'static str],
    pub include_memory_context: bool,
    pub experience_memory_rule: Option<&'static str>,
}
```

字段含义：

- `role`：角色名
- `description`：给 prompt 用的角色概述
- `system_instruction_prompt`：从 `src/prompt/*.md` 读取的角色系统约束
- `skill_groups`：技能组
- `skills`：单独追加的技能
- `include_memory_context`：是否注入 `remember summary` 和最近聊天历史
- `experience_memory_rule`：该 role 是否维护专属经验记忆，以及记忆提炼规则

角色指令文案不再直接写在代码里，当前放在：

- [src/prompt/role-leader-system.md](/Users/wangqizhi/Project/MyLittleBotty/src/prompt/role-leader-system.md)
- [src/prompt/role-paperwork-system.md](/Users/wangqizhi/Project/MyLittleBotty/src/prompt/role-paperwork-system.md)
- [src/prompt/role-all-in-one-system.md](/Users/wangqizhi/Project/MyLittleBotty/src/prompt/role-all-in-one-system.md)

## 技能组定义方法

技能组也在 [src/botty/botty-guy.rs](/Users/wangqizhi/Project/MyLittleBotty/src/botty/botty-guy.rs)。

当前内置：

```rust
const BASE_SKILL_GROUP: &[&str] = &["list", "watch", "write"];
```

组名到技能列表的映射由 `skill_group_members()` 处理。

如果要新增技能组：

1. 新增一个 `const XXX_SKILL_GROUP`
2. 在 `skill_group_members()` 里加分支
3. 在角色配置的 `skill_groups` 中引用这个组名

## 单角色定义方法

示例：

```rust
const PAPERWORK_ROLE_SPEC: BottyGuyRoleSpec = BottyGuyRoleSpec {
    role: "paperwork",
    description: "...",
    system_instruction: "...",
    skill_groups: &["base"],
    skills: &[],
    include_memory_context: false,
};
```

如果要新增角色：

1. 在 [src/botty/botty-guy.rs](/Users/wangqizhi/Project/MyLittleBotty/src/botty/botty-guy.rs) 新增一个 `BottyGuyRoleSpec`
2. 在 `resolve_role_spec()` 里注册角色名
3. 如果该角色允许被 leader 分派，在 `delegated_role_names()` 里加入它
4. 如果需要新技能，先在 [src/skill/mod.rs](/Users/wangqizhi/Project/MyLittleBotty/src/skill/mod.rs) 的 `build_skill()` 注册

## Leader 分派逻辑

`leader` skill 定义在 [src/skill/buildin-leader.rs](/Users/wangqizhi/Project/MyLittleBotty/src/skill/buildin-leader.rs)。

行为：

- 校验目标角色是否允许被分派
- 拉起一个新的 `--guy` 子进程
- 通过环境变量 `BOTTY_GUY_ROLE=<role>` 指定角色
- 给子进程发送精简后的任务描述
- 读取子进程返回结果并回传给 leader

这意味着 leader 本身不承担具体执行，它只负责：

- 识别任务类型
- 选择角色
- 传递必要信息

## Role 与上下文

角色是否带历史上下文由 `include_memory_context` 控制：

- `true`：会加载 `memory/summary/remember.md` 和最近 deep memory
- `false`：只基于当前任务执行

当前默认：

- `leader`：`true`
- `paperwork`：`false`
- `all-in-one`：`false`

## Role 专属 memory

部分 role 还可以维护自己的经验记忆文件：

- 路径格式：`~/.mylittlebotty/memory/summary/experience/<role>-exp.md`
- 当前启用：
  - `coder`
  - `info-searcher`

行为：

- 执行 `/remember` 时，除了更新全局 `remember.md`，还会同步更新所有已启用 role memory 的 `*-exp.md`
- role 启动时，如果该文件存在，会额外注入到该 role 的 system prompt

规则说明见 [doc/role-memory.md](/Users/wangqizhi/Project/MyLittleBotty/doc/role-memory.md)。

## 调试日志里的 role

brain debug 日志写在 `brain-debug*.log`，每一行现在都带 `role=` 字段，例如：

```text
[2026-03-09 10:00:00] role=leader request: ...
[2026-03-09 10:00:01] role=paperwork response: ...
```

`mylittlebotty log -f` 也会解析并显示这个字段，便于区分 leader 和被分派的子 agent。
