# System Crond

`Botty-crond` now has two scheduling layers:

- user-configured reminders in `~/.mylittlebotty/reminder.rec`
- hardcoded system tasks compiled into `src/botty/botty-crond.rs`

System tasks are not stored in `reminder.rec` and are not editable through the `crond` tool. They start running automatically after the service starts.

## Current Built-in Task

- `remember-hourly`: runs `/remember` once per hour through the normal Botty request path

Execution logs are written to:

- `~/.mylittlebotty/log/system-crond.log`
- development builds use `~/.mylittlebotty/log/system-crond-dev.log`

Runtime state is persisted in:

- `~/.mylittlebotty/run/system-crond-state.json`
- development builds use `~/.mylittlebotty/run/system-crond-state-dev.json`

The state file stores the last completed schedule slot for each task so the same hour is not executed twice after a restart.

## How To Add Another System Task

Add the task in [`src/botty/botty-crond.rs`](/Users/wangqizhi/Project/MyLittleBotty/src/botty/botty-crond.rs):

1. Extend `system_tasks()` with a new `SystemTask`.
2. Pick a stable `id`, a readable `description`, and the request text to send through `request_message`.
3. Choose the cadence in `SystemTaskCadence`.
4. If a new cadence is needed, extend `SystemTaskCadence` and `SystemTask::due_at()`.
5. Keep the task implementation idempotent for a single schedule slot, because state is recorded after each execution attempt.

Example:

```rust
fn system_tasks() -> [SystemTask; 2] {
    [
        SystemTask {
            id: "remember-hourly",
            description: "refresh long-term memory via /remember",
            request_message: "/remember",
            cadence: SystemTaskCadence::Hourly,
        },
        SystemTask {
            id: "daily-summary",
            description: "run the daily summary command",
            request_message: "/daily-summary",
            cadence: SystemTaskCadence::Daily,
        },
    ]
}
```

If you add a new cadence such as `Daily`, also update the due-slot calculation and keep the slot string stable so restart deduplication continues to work.
