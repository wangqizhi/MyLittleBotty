use crate::acp::browser::singleton_browser_session;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_MAX_RESULTS: usize = 5;
const DEFAULT_TIMEOUT_SECONDS: u64 = 15;

pub fn handle_web_search_skill_request(input_json: &str) -> io::Result<String> {
    let request: WebSearchSkillRequest = serde_json::from_str(input_json).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("parse web-search skill input failed: {err}"),
        )
    })?;

    let query = required_text(request.query.as_deref(), "query")?;
    let max_results = request
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, 10);
    let timeout = Duration::from_secs(
        request
            .timeout_seconds
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
            .clamp(3, 60),
    );
    let session = singleton_browser_session(request.headless)?;

    let engine_names = resolve_engines(request.engine.as_deref(), query)?;
    let use_default_fallback_chain =
        should_use_default_fallback_chain(request.engine.as_deref(), query);
    let mut engine_results = Vec::new();
    for engine_name in engine_names {
        let engine = SearchEngine::from_name(engine_name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported search engine: {engine_name}"),
            )
        })?;

        if matches!(engine, SearchEngine::XGrok) {
            let result = run_x_grok_search(&session.browser, query, max_results, timeout)?;
            engine_results.push(json!({
                "engine": engine.name(),
                "homeUrl": engine.home_url(),
                "usedFallbackUrl": false,
                "submit": {
                    "ok": true,
                    "method": "x-grok-compose",
                },
                "pageUrl": result.get("pageUrl").cloned().unwrap_or_else(|| json!("")),
                "pageTitle": result.get("pageTitle").cloned().unwrap_or_else(|| json!("")),
                "results": result.get("results").cloned().unwrap_or_else(|| json!([])),
            }));
            continue;
        }

        let result_url = engine.results_url(query);
        let search_payload = json!({
            "ok": true,
            "method": "direct-results-url",
            "url": result_url,
        });

        let results = match session.browser.navigate(&result_url) {
            Ok(_) => wait_for_results(&session.browser, engine, max_results, timeout),
            Err(err) => Err(err),
        };
        let results = match results {
            Ok(results) => results,
            Err(err) if use_default_fallback_chain => {
                engine_results.push(json!({
                    "engine": engine.name(),
                    "homeUrl": engine.home_url(),
                    "usedFallbackUrl": true,
                    "submit": search_payload,
                    "pageUrl": "",
                    "pageTitle": "",
                    "results": [],
                    "error": err.to_string(),
                }));
                continue;
            }
            Err(err) => return Err(err),
        };
        let result_count = results
            .get("results")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        engine_results.push(json!({
            "engine": engine.name(),
            "homeUrl": engine.home_url(),
            "usedFallbackUrl": true,
            "submit": search_payload,
            "pageUrl": results.get("pageUrl").cloned().unwrap_or_else(|| json!("")),
            "pageTitle": results.get("pageTitle").cloned().unwrap_or_else(|| json!("")),
            "results": results.get("results").cloned().unwrap_or_else(|| json!([])),
            "debug": results.get("debug").cloned().unwrap_or_else(|| json!({})),
            "blocked": results.get("blocked").cloned().unwrap_or_else(|| json!(false)),
            "blockReason": results
                .get("blockReason")
                .cloned()
                .unwrap_or_else(|| json!("")),
        }));
        if use_default_fallback_chain && result_count > 0 {
            break;
        }
    }

    let payload = json!({
        "query": query,
        "engine": request
            .engine
            .unwrap_or_else(|| "google->bing->baidu".to_string()),
        "maxResults": max_results,
        "engines": engine_results,
    });
    let body = serde_json::to_string_pretty(&payload)
        .map_err(|err| io::Error::other(format!("serialize web-search result failed: {err}")))?;
    Ok(format!(
        "web_search_action=search\nsession_id={}\nresult={body}",
        session.id
    ))
}

#[derive(Deserialize)]
struct WebSearchSkillRequest {
    query: Option<String>,
    engine: Option<String>,
    max_results: Option<usize>,
    timeout_seconds: Option<u64>,
    headless: Option<bool>,
}

#[derive(Clone, Copy)]
enum SearchEngine {
    Google,
    Baidu,
    Bing,
    XGrok,
}

impl SearchEngine {
    fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "google" => Some(Self::Google),
            "baidu" => Some(Self::Baidu),
            "bing" => Some(Self::Bing),
            "x" | "grok" | "x-grok" => Some(Self::XGrok),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Baidu => "baidu",
            Self::Bing => "bing",
            Self::XGrok => "x",
        }
    }

    fn home_url(self) -> &'static str {
        match self {
            Self::Google => "https://www.google.com/",
            Self::Baidu => "https://www.baidu.com/",
            Self::Bing => "https://www.bing.com/",
            Self::XGrok => "https://x.com/i/grok",
        }
    }

    fn results_url(self, query: &str) -> String {
        let encoded = utf8_percent_encode(query, NON_ALPHANUMERIC).to_string();
        match self {
            Self::Google => format!("https://www.google.com/search?q={encoded}"),
            Self::Baidu => format!("https://www.baidu.com/s?wd={encoded}"),
            Self::Bing => format!("https://www.bing.com/search?q={encoded}"),
            Self::XGrok => format!("https://x.com/i/grok?q={encoded}"),
        }
    }

    fn result_wait_selector(self) -> &'static str {
        match self {
            Self::Google => "#search .MjjYud, #search .g, #rso h3",
            Self::Baidu => {
                "#content_left .result, #content_left .result-op, #content_left .c-container"
            }
            Self::Bing => "#b_results > li.b_algo, #b_results .b_algo, #b_results .b_result",
            Self::XGrok => "",
        }
    }
}

fn required_text<'a>(value: Option<&'a str>, name: &str) -> io::Result<&'a str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("web-search requires `{name}`"),
            )
        })
}

fn resolve_engines(engine: Option<&str>, query: &str) -> io::Result<Vec<&'static str>> {
    let Some(raw) = engine.map(str::trim).filter(|value| !value.is_empty()) else {
        if infer_x_search(query) {
            return Ok(vec!["x"]);
        }
        return Ok(vec!["google", "bing", "baidu"]);
    };
    if raw.eq_ignore_ascii_case("all") {
        return Ok(vec!["google", "bing", "baidu"]);
    }
    let mut engines = Vec::new();
    for part in raw.split(',') {
        let name = part.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        match name.as_str() {
            "google" | "baidu" | "bing" | "x" | "grok" | "x-grok" => {
                if !engines.contains(&name.as_str()) {
                    engines.push(match name.as_str() {
                        "google" => "google",
                        "baidu" => "baidu",
                        "bing" => "bing",
                        _ => "x",
                    });
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unsupported search engine: {name}"),
                ));
            }
        }
    }
    if engines.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "web-search requires at least one engine",
        ));
    }
    Ok(engines)
}

fn should_use_default_fallback_chain(engine: Option<&str>, query: &str) -> bool {
    engine
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
        && !infer_x_search(query)
}

fn infer_x_search(query: &str) -> bool {
    let compact = query.to_ascii_lowercase();
    compact.contains("在x上搜")
        || compact.contains("在 x 上搜")
        || compact.contains("从x搜")
        || compact.contains("从 x 搜")
        || compact.contains("推特")
        || compact.contains("x(twitter)")
        || compact.contains("twitter")
        || compact.contains("grok")
        || compact.contains("x.com")
}

fn wait_for_results(
    browser: &crate::infra::app_browser::AppBrowser,
    engine: SearchEngine,
    max_results: usize,
    timeout: Duration,
) -> io::Result<Value> {
    if matches!(engine, SearchEngine::XGrok) {
        return wait_for_x_results(browser, max_results, timeout);
    }
    let _ = browser.wait_for(engine.result_wait_selector(), timeout);
    let started = Instant::now();
    let mut last_value = json!({
        "pageUrl": "",
        "pageTitle": "",
        "results": [],
        "debug": {},
        "blocked": false,
        "blockReason": "",
    });
    let script = build_extract_script(engine, max_results);

    loop {
        let value = browser.eval(&script)?;
        if value
            .get("blocked")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(value);
        }
        let count = value
            .get("results")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        if count > 0 {
            return Ok(value);
        }
        last_value = value;
        if started.elapsed() >= timeout {
            return Ok(last_value);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn build_extract_script(engine: SearchEngine, max_results: usize) -> String {
    let max_results = max_results.max(1);
    let engine_name = engine.name();
    let google_container_selector =
        serde_json::to_string("#search .MjjYud, #search .g, #rso > div, [data-snc]")
            .unwrap_or_else(|_| "\"\"".to_string());
    let google_title_selector =
        serde_json::to_string("h3, [role='heading']").unwrap_or_else(|_| "\"\"".to_string());
    let google_snippet_selector =
        serde_json::to_string(".VwiC3b, .yXK7lf, .MUxGbd, .lyLwlc, .FrIlee, .st")
            .unwrap_or_else(|_| "\"\"".to_string());
    let baidu_container_selector = serde_json::to_string(
        "#content_left .result, #content_left .result-op, #content_left .c-container, #content_left [data-click]",
    )
    .unwrap_or_else(|_| "\"\"".to_string());
    let baidu_title_selector =
        serde_json::to_string("h3, .c-title a").unwrap_or_else(|_| "\"\"".to_string());
    let baidu_snippet_selector = serde_json::to_string(
        ".c-abstract, .c-color-text, .content-right_8Zs40, .c-span-last, .c-gap-top-small span, div[class*='content']",
    )
    .unwrap_or_else(|_| "\"\"".to_string());
    let bing_container_selector = serde_json::to_string(
        "#b_results > li.b_algo, #b_results .b_algo, #b_results li.b_ans, #b_results .b_result",
    )
    .unwrap_or_else(|_| "\"\"".to_string());
    let bing_title_selector =
        serde_json::to_string("h2, .b_title").unwrap_or_else(|_| "\"\"".to_string());
    let bing_snippet_selector =
        serde_json::to_string(".b_caption p, .b_snippet, .b_algoSlug, .b_paractl, p")
            .unwrap_or_else(|_| "\"\"".to_string());
    let extractor = match engine {
        SearchEngine::Google => {
            r##"
            const containers = Array.from(document.querySelectorAll(
                "#search .MjjYud, #search .g, #rso > div, [data-snc]"
            ));
            for (const node of containers) {
                const anchor = node.querySelector("a[href][data-ved], a[href]") || null;
                const titleNode = node.querySelector("h3, [role='heading']") || anchor;
                if (!anchor || !titleNode) continue;
                const title = clean(titleNode.innerText || titleNode.textContent || "");
                const url = anchor.href || anchor.getAttribute("href") || "";
                if (!title || !url || seen.has(url)) continue;
                const snippetNode = node.querySelector(
                    ".VwiC3b, .yXK7lf, .MUxGbd, .lyLwlc, .FrIlee, .st"
                );
                results.push({
                    title,
                    url,
                    snippet: clean(snippetNode ? (snippetNode.innerText || snippetNode.textContent || "") : "")
                });
                seen.add(url);
                if (results.length >= limit) break;
            }
            "##
        }
        SearchEngine::Baidu => {
            r##"
            const containers = Array.from(document.querySelectorAll(
                "#content_left .result, #content_left .result-op, #content_left .c-container, #content_left [data-click]"
            ));
            for (const node of containers) {
                const anchor = node.querySelector("h3 a[href], a[href]") || null;
                const titleNode = node.querySelector("h3, .c-title a") || anchor;
                if (!anchor || !titleNode) continue;
                const title = clean(titleNode.innerText || titleNode.textContent || "");
                const url = anchor.href || anchor.getAttribute("href") || "";
                if (!title || !url || seen.has(url)) continue;
                const snippetNode = node.querySelector(
                    ".c-abstract, .c-color-text, .content-right_8Zs40, .c-span-last, .c-gap-top-small span, div[class*='content']"
                );
                results.push({
                    title,
                    url,
                    snippet: clean(snippetNode ? (snippetNode.innerText || snippetNode.textContent || "") : "")
                });
                seen.add(url);
                if (results.length >= limit) break;
            }
            "##
        }
        SearchEngine::Bing => {
            r##"
            const containers = Array.from(document.querySelectorAll(
                "#b_results > li.b_algo, #b_results .b_algo, #b_results li.b_ans, #b_results .b_result"
            ));
            for (const node of containers) {
                const anchor = node.querySelector("h2 a[href], .b_title a[href], a[href]") || null;
                const titleNode = node.querySelector("h2, .b_title") || anchor;
                if (!anchor || !titleNode) continue;
                const title = clean(titleNode.innerText || titleNode.textContent || "");
                const url = anchor.href || anchor.getAttribute("href") || "";
                if (!title || !url || seen.has(url)) continue;
                const snippetNode = node.querySelector(".b_caption p, .b_snippet, .b_algoSlug, .b_paractl, p");
                results.push({
                    title,
                    url,
                    snippet: clean(snippetNode ? (snippetNode.innerText || snippetNode.textContent || "") : "")
                });
                seen.add(url);
                if (results.length >= limit) break;
            }
            "##
        }
        SearchEngine::XGrok => {
            r##"
            const containers = [];
            "##
        }
    };

    format!(
        r#"(function () {{
            const limit = {max_results};
            const engine = {engine_name:?};
            const googleContainerSelector = {google_container_selector};
            const googleTitleSelector = {google_title_selector};
            const googleSnippetSelector = {google_snippet_selector};
            const baiduContainerSelector = {baidu_container_selector};
            const baiduTitleSelector = {baidu_title_selector};
            const baiduSnippetSelector = {baidu_snippet_selector};
            const bingContainerSelector = {bing_container_selector};
            const bingTitleSelector = {bing_title_selector};
            const bingSnippetSelector = {bing_snippet_selector};
            const seen = new Set();
            const results = [];
            function clean(text) {{
                return String(text || "").replace(/\s+/g, " ").trim();
            }}
            function sampleNodes(selectors, titleSelectors, snippetSelectors) {{
                const samples = [];
                const nodes = Array.from(document.querySelectorAll(selectors)).slice(0, 5);
                for (const node of nodes) {{
                    const titleNode = node.querySelector(titleSelectors);
                    const snippetNode = node.querySelector(snippetSelectors);
                    const anchor = node.querySelector("a[href]");
                    samples.push({{
                        containerClass: clean(node.className || node.getAttribute("class") || ""),
                        containerTag: (node.tagName || "").toLowerCase(),
                        title: clean(titleNode ? (titleNode.innerText || titleNode.textContent || "") : ""),
                        snippet: clean(snippetNode ? (snippetNode.innerText || snippetNode.textContent || "") : ""),
                        url: anchor ? (anchor.href || anchor.getAttribute("href") || "") : ""
                    }});
                }}
                return samples;
            }}
            function detectBlockedPage() {{
                const title = clean(document.title || "");
                const text = clean(document.body ? (document.body.innerText || document.body.textContent || "") : "");
                const lowered = `${{title}} ${{text}}`.toLowerCase();
                if (engine === "google" && (
                    title === "Google Search" ||
                    lowered.includes("enablejs") ||
                    lowered.includes("unusual traffic") ||
                    lowered.includes("having trouble accessing google search")
                )) {{
                    return "google-challenge";
                }}
                if (engine === "bing" && (
                    lowered.includes("one last step") ||
                    lowered.includes("please solve the challenge below to continue") ||
                    lowered.includes("turnstile")
                )) {{
                    return "bing-challenge";
                }}
                if (engine === "baidu" && (
                    lowered.includes("百度安全验证") ||
                    lowered.includes("安全验证") ||
                    lowered.includes("网络不给力，请稍后重试")
                )) {{
                    return "baidu-challenge";
                }}
                return "";
            }}
            const blockReason = detectBlockedPage();
            {extractor}
            return {{
                pageTitle: document.title || "",
                pageUrl: location.href || "",
                blocked: blockReason.length > 0,
                blockReason,
                debug: {{
                    bodyTextSample: clean(document.body ? (document.body.innerText || document.body.textContent || "") : "").slice(0, 1200),
                    googleSamples: engine === "google" ? sampleNodes(
                        googleContainerSelector,
                        googleTitleSelector,
                        googleSnippetSelector
                    ) : [],
                    baiduSamples: engine === "baidu" ? sampleNodes(
                        baiduContainerSelector,
                        baiduTitleSelector,
                        baiduSnippetSelector
                    ) : [],
                    bingSamples: engine === "bing" ? sampleNodes(
                        bingContainerSelector,
                        bingTitleSelector,
                        bingSnippetSelector
                    ) : []
                }},
                results
            }};
        }})()"#
    )
}

fn run_x_grok_search(
    browser: &crate::infra::app_browser::AppBrowser,
    query: &str,
    max_results: usize,
    timeout: Duration,
) -> io::Result<Value> {
    browser.navigate("https://x.com/i/grok")?;
    let submit = browser.eval(&build_x_grok_submit_script(query))?;
    if submit
        .get("needsLogin")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(io::Error::other(
            "x/grok requires login before search can continue",
        ));
    }
    wait_for_x_results(browser, max_results, timeout)
}

fn wait_for_x_results(
    browser: &crate::infra::app_browser::AppBrowser,
    max_results: usize,
    timeout: Duration,
) -> io::Result<Value> {
    let started = Instant::now();
    let mut last_value = json!({
        "pageTitle": "",
        "pageUrl": "",
        "results": [],
    });
    let script = build_x_grok_extract_script(max_results);
    loop {
        let value = browser.eval(&script)?;
        if value
            .get("needsLogin")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(io::Error::other(
                "x/grok requires login before search can continue",
            ));
        }
        let count = value
            .get("results")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        if count > 0 {
            return Ok(value);
        }
        last_value = value;
        if started.elapsed() >= timeout {
            return Ok(last_value);
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn build_x_grok_submit_script(query: &str) -> String {
    let query = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".to_string());
    r##"(function () {
            const query = __QUERY__;
            const inputXpaths = [
                "//*[@id='react-root']/div/div/div[2]/main/div/div/div/div/div/div[3]/div/div/div/div/div/div[1]/div/div/div[1]/div/div/div[1]/div[1]/div/textarea",
                "//main//textarea",
                "//textarea"
            ];
            const buttonXpaths = [
                "//*[@id='react-root']/div/div/div[2]/main/div/div/div/div/div/div[3]/div/div/div/div/div/div[1]/div/div/div[1]/div/div/div[2]/div[2]/button",
                "//main//button[@type='submit']",
                "//main//button",
                "//button[@type='submit']"
            ];

            function byXPath(path) {
                try {
                    return document.evaluate(path, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;
                } catch (_) {
                    return null;
                }
            }

            function visible(el) {
                if (!el) return false;
                const style = window.getComputedStyle(el);
                if (!style || style.visibility === "hidden" || style.display === "none") return false;
                const rect = el.getBoundingClientRect();
                return rect.width > 0 && rect.height > 0;
            }

            function firstVisibleByXPath(paths) {
                for (const path of paths) {
                    const el = byXPath(path);
                    if (visible(el)) return el;
                }
                for (const path of paths) {
                    const el = byXPath(path);
                    if (el) return el;
                }
                return null;
            }

            function looksLikeLogin() {
                const text = (document.body?.innerText || "").toLowerCase();
                return text.includes("sign in") || text.includes("login") || text.includes("log in");
            }

            const input = firstVisibleByXPath(inputXpaths);
            if (!input) {
                return { ok: false, needsLogin: looksLikeLogin(), reason: "input-not-found" };
            }

            input.scrollIntoView({ block: "center", inline: "center" });
            input.focus();
            if ("value" in input) {
                input.value = query;
            } else {
                input.textContent = query;
            }
            input.dispatchEvent(new InputEvent("input", { bubbles: true, data: query, inputType: "insertText" }));
            input.dispatchEvent(new Event("change", { bubbles: true }));

            const button = firstVisibleByXPath(buttonXpaths);
            if (button) {
                button.click();
                return { ok: true, method: "click" };
            }

            const enter = { key: "Enter", code: "Enter", which: 13, keyCode: 13, bubbles: true };
            input.dispatchEvent(new KeyboardEvent("keydown", enter));
            input.dispatchEvent(new KeyboardEvent("keypress", enter));
            input.dispatchEvent(new KeyboardEvent("keyup", enter));
            return { ok: true, method: "enter" };
        })()"##
        .replace("__QUERY__", &query)
}

fn build_x_grok_extract_script(max_results: usize) -> String {
    format!(
        r##"(function () {{
            const limit = {max_results};
            function clean(text) {{
                return String(text || "").replace(/\s+/g, " ").trim();
            }}
            function looksLikeLogin() {{
                const text = (document.body?.innerText || "").toLowerCase();
                return text.includes("sign in") || text.includes("login") || text.includes("log in");
            }}

            const articleNodes = Array.from(document.querySelectorAll("article, [data-testid='cellInnerDiv'] article"));
            const results = [];
            const seen = new Set();
            for (const node of articleNodes) {{
                const text = clean(node.innerText || node.textContent || "");
                if (!text || text.length < 20) continue;
                const link = node.querySelector("a[href*='/status/'], a[href*='/i/grok'], a[href^='https://x.com/']");
                const url = link ? (link.href || link.getAttribute("href") || "") : "";
                if (seen.has(text)) continue;
                seen.add(text);
                results.push({{
                    title: text.slice(0, 80),
                    url,
                    snippet: text
                }});
                if (results.length >= limit) break;
            }}

            if (results.length === 0) {{
                const blocks = Array.from(document.querySelectorAll("main div[dir='auto'], main span, main p"));
                for (const node of blocks) {{
                    const text = clean(node.innerText || node.textContent || "");
                    if (!text || text.length < 40) continue;
                    if (text === "Grok" || text === "Search Grok" || text === "Post") continue;
                    if (seen.has(text)) continue;
                    seen.add(text);
                    results.push({{
                        title: text.slice(0, 80),
                        url: "",
                        snippet: text
                    }});
                    if (results.length >= limit) break;
                }}
            }}

            return {{
                pageTitle: document.title || "",
                pageUrl: location.href || "",
                needsLogin: looksLikeLogin(),
                results
            }};
        }})()"##
    )
}
