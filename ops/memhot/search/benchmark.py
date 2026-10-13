#!/usr/bin/env python3
"""Public synthetic queries only; no user, account, prompt, or provider secrets."""
import argparse
import datetime
import json
import time
import urllib.error
import urllib.parse
import urllib.request

QUERIES = [
    ("en", "SQLite WAL concurrency documentation"),
    ("zh-CN", "国家统计局 居民消费价格 指数"),
    ("en", "small business cash flow management"),
    ("zh-CN", "间隔重复 学习 研究"),
]


def query(endpoint, engine, language, text, recent):
    fields = {
        "q": text, "format": "json", "engines": engine,
        "language": language, "safesearch": "1", "pageno": "1",
    }
    if recent:
        fields["time_range"] = "month"
    data = urllib.parse.urlencode(fields).encode()
    started = time.monotonic()
    result = {"engine": engine or "all", "language": language, "query": text,
              "timeRange": "month" if recent else "any"}
    try:
        request = urllib.request.Request(endpoint, data=data)
        with urllib.request.urlopen(request, timeout=15) as response:
            result["httpStatus"] = response.status
            body = json.load(response)
        results = body.get("results", [])
        result.update({
            "resultCount": len(results),
            "unresponsiveEngines": body.get("unresponsive_engines", []),
            "topResults": [
                {"title": r.get("title", ""), "url": r.get("url", ""),
                 "engines": r.get("engines", []), "snippetChars": len(r.get("content", ""))}
                for r in results[:5]
            ],
        })
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
        result.update({"errorType": type(error).__name__, "resultCount": 0})
    result["elapsedSeconds"] = round(time.monotonic() - started, 3)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--endpoint", default="http://127.0.0.1:8791/search")
    parser.add_argument("--engines", default="duckduckgo,360search")
    parser.add_argument("--combined", action="store_true")
    parser.add_argument("--auto-language", action="store_true")
    parser.add_argument("--recent", action="store_true")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    report = {"generatedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "purpose": "public synthetic general-search smoke test; no personal queries",
              "queries": []}
    for engine in ([""] if args.combined else args.engines.split(",")):
        for language, text in QUERIES:
            outcome = query(args.endpoint, engine, "auto" if args.auto_language else language,
                            text, args.recent)
            report["queries"].append(outcome)
            print(json.dumps({k: outcome[k] for k in
                              ("engine", "language", "resultCount", "elapsedSeconds")}), flush=True)
    with open(args.output, "w", encoding="utf-8") as handle:
        json.dump(report, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


if __name__ == "__main__":
    main()
