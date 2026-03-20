# Role Memory 说明

## 目标

除了全局长期记忆 `remember.md`，部分 role 还可以维护自己的经验记忆文件。

用途：

- `coder`：记住当前在开发的项目清单
- `info-searcher`：记住应用名、URL、leader 称呼等稳定映射

## 文件位置

- 全局长期记忆：
  [~/.mylittlebotty/memory/summary/remember.md](~/.mylittlebotty/memory/summary/remember.md)
- role 经验记忆目录：
  [~/.mylittlebotty/memory/summary/experience/](~/.mylittlebotty/memory/summary/experience)
- `coder` 文件：
  [~/.mylittlebotty/memory/summary/experience/coder-exp.md](~/.mylittlebotty/memory/summary/experience/coder-exp.md)
- `info-searcher` 文件：
  [~/.mylittlebotty/memory/summary/experience/info-searcher-exp.md](~/.mylittlebotty/memory/summary/experience/info-searcher-exp.md)

## 触发方式

- 在 TUI 中执行 `/remember`
- 仍然会更新全局 `remember.md`
- 还会统一刷新所有已配置经验记忆规则的 role 对应 `*-exp.md`
- 生成 `*-exp.md` 时，不只看这次新增对话，也会把当前 `remember.md` 一起作为参考

## 注入方式

role 启动时，如果存在对应的 `~/.mylittlebotty/memory/summary/experience/<role>-exp.md`，系统会把内容注入该 role 的 system prompt。

这部分记忆与全局 `remember.md` 独立：

- 全局 `remember.md`：记录近期关键事件、请求、状态，以及用户明确要求以后记住的内容
- role `*-exp.md`：记录某个角色未来复用价值高的稳定知识

## 生成依据

每次 `/remember` 生成 role 经验记忆时，模型会同时参考两部分输入：

- 本次新进入长期记忆范围的 deep memory transcript
- 当前已有的 `remember.md`

这样做的目的，是避免 role `*-exp.md` 只能看到“本轮新对话”，而丢掉已经沉淀在全局长期记忆里的关键信息。

例如：

- `remember.md` 里已经有某个项目、页面、系统入口、URL、负责人称呼
- 这次新对话没有重新提到这些信息
- `/remember` 依然可以把这些信息保留或整理进对应 role 的 `*-exp.md`

## 当前规则写法

### coder

目标：维护当前开发项目清单。

推荐写法：

```md
- 项目名:MyLittleBotty 项目路径:/Users/name/Project/MyLittleBotty 项目简介:Telegram bot + role agent + TUI
- 项目名:cmdb-worker 项目路径:/Users/name/work/cmdb-worker 项目简介:CMDB 数据同步与接口服务
```

约束：

- 优先记录“正在维护/近期持续维护”的项目
- 路径要尽量是绝对路径
- 项目移动目录、改名、目标变化时要覆盖旧信息
- 不要记录一次性临时目录或低价值细节

### info-searcher

目标：维护应用名、URL、leader 称呼等稳定映射。

推荐写法：

```md
- 应用名:cmdb平台 url地址:https://cmdb.example.com leader称呼:王工 说明:内网资产平台
- 应用名:报表后台 url地址:https://report.example.com leader称呼:李老师 说明:经营分析报表入口
```

约束：

- 优先记录之后还会反复访问的系统或站点
- 名称、URL、称呼尽量保持稳定字段格式
- 只有在映射发生变化时才更新旧项
- 不要把一次性搜索结果或页面噪音写进去

## 扩展新 role

如果以后要给新 role 增加经验记忆：

1. 在 [src/botty/botty-guy.rs](~/Project/MyLittleBotty/src/botty/botty-guy.rs) 的 `BottyGuyRoleSpec` 中配置 `experience_memory_rule`
2. 约定该 role 的经验文件名为 `~/.mylittlebotty/memory/summary/experience/<role>-exp.md`
3. 为该 role 设计稳定、可复用、低噪音的字段格式

## 新增一个 role exp.md 的修改点

如果你下次想让 `/remember` 自动多生成一个 role 的 `exp.md`，通常只需要改一个核心点：

1. 在 [src/botty/botty-guy.rs](~/Project/MyLittleBotty/src/botty/botty-guy.rs) 找到目标 role 的 `BottyGuyRoleSpec`
2. 给这个 role 增加或修改 `experience_memory_rule: Some(\"...\")`

为什么通常只改这里：

- `/remember` 现在会自动遍历所有内置 role
- 只要某个 role 配了 `experience_memory_rule`
- 就会自动生成 `~/.mylittlebotty/memory/summary/experience/<role>-exp.md`
- role 启动时也会自动读取并注入这个文件

也就是说，生成逻辑和注入逻辑已经是通用的，不需要再单独给每个 role 写一套代码。

### 例子

假设以后新增一个 `ops` role，希望它记住服务器和环境信息：

```rust
const OPS_ROLE_SPEC: BottyGuyRoleSpec = BottyGuyRoleSpec {
    role: "ops",
    description: "Handle server, deploy, and environment tasks.",
    system_instruction_prompt: prompt::ROLE_OPS_SYSTEM_PROMPT,
    skill_groups: &[],
    skills: &["terminal"],
    include_memory_context: false,
    experience_memory_rule: Some(
        "Keep stable environment and server records. Prefer records like `环境名:xxx 服务名:xxx 地址:xxx 登录方式:xxx 说明:xxx`.",
    ),
};
```

改完后，`/remember` 就会自动尝试生成：

- `~/.mylittlebotty/memory/summary/experience/ops-exp.md`

### 你下次可以直接这样提需求

你可以直接对我说：

```text
给 role `ops` 也加上 experience memory。
规则是记住环境名、服务名、地址、登录方式、说明。
文件放到 ~/.mylittlebotty/memory/summary/experience/ops-exp.md。
启动 ops role 时也要自动注入到 prompt。
顺便把 doc/role-memory.md 和 README 更新一下。
```

如果你把“字段格式”和“想记住什么信息”说清楚，我通常只需要补 role 配置和文档。
