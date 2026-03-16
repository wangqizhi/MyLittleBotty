# Browser Procedure Memory

## Current implementation

- Structured browser action trace is appended to `~/.mylittlebotty/app/browser/sessions/<session_id>/action-trace.jsonl`.
- A successful browser task can now be closed explicitly with `browser.complete_task`.
- `complete_task` reads trace entries after the most recent prior `complete_task`, summarizes them into a reusable SOP with the configured LLM, and writes outputs under `~/.mylittlebotty/memory/browser-procedures/<domain>/`.
- Each saved procedure currently produces:
  - `<timestamp>-<task-slug>.md`: Markdown SOP for human inspection.
  - `<timestamp>-<task-slug>.json`: metadata plus the same SOP content.

## Why retrieval is separate

- Trace capture and SOP generation are useful even before retrieval exists.
- Retrieval policy needs tuning around ranking, freshness, and failure fallback; that work should not block capture.
- Keeping browser procedure memory separate from `memory/deep` avoids noisy keyword search across chat transcripts.

## Retrieval proposal

### Retrieval trigger

- Run SOP lookup only for browser-driven tasks, not for ordinary `web-search`.
- Trigger when the agent is about to use `browser` for a multi-step website task such as login-gated navigation, dashboard workflows, repeated searches on the same site, or form submission.

### Candidate selection

- Primary filters:
  - Current URL or requested domain.
  - User intent or agent task description.
- Secondary ranking signals:
  - Newer SOPs first.
  - Exact domain match over suffix/domain-family match.
  - SOPs whose `task` wording overlaps with the current request.
  - SOPs with more recent successful reuse history once that metric exists.

### Suggested storage additions

- Extend procedure JSON with optional fields:
  - `url_patterns`
  - `stable_cues`
  - `last_used_at_ms`
  - `reuse_success_count`
  - `reuse_failure_count`
- Keep the Markdown file human-friendly and use JSON for ranking metadata.

### Retrieval flow

1. Build a short task query from the current browser intent.
2. Enumerate procedure JSON files under the matching domain folder first, then optionally sibling domains.
3. Score candidates with lightweight local heuristics before asking an LLM to read anything.
4. Inject only the top 1 to 3 SOPs into the browser agent prompt.
5. Tell the agent to try SOP-guided execution first, but fall back to normal snapshot-driven reasoning immediately if a step fails.

### Failure handling

- A reused SOP should never be treated as authoritative.
- On step failure:
  - Capture a fresh snapshot.
  - Let the agent adapt the locator or branch.
  - Record that deviation for the next `complete_task`.
- After repeated failures for the same SOP, reduce its rank or mark it stale.

### Possible future browser API support

- `browser.list_procedures(domain=...)`
- `browser.get_procedure(path=...)`
- `browser.find_procedures(task=..., url=...)`
- `browser.mark_procedure_result(path=..., success=true|false)`

## Suggested next implementation

- Add a lightweight retrieval helper in the browser/info-searcher path, not in generic `remember`.
- Start with domain-folder scan plus filename/task matching before adding embeddings or semantic indexing.
- Keep prompt injection small: one SOP summary plus maybe one fallback SOP is usually enough.
