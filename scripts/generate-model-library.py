#!/usr/bin/env python3
"""生成内置模型库 `src/ai_gateway/model_library.json`。

数据来源：社区维护的 models.dev（https://models.dev/api.json）。该目录覆盖
200+ 厂商、数千个模型，但同一模型名会在几十家转售商里重复出现（例如
`deepseek-v4-pro` 出现在 32 家），且大部分字段对本项目无用。

因此本脚本把原始目录**按模型 id 归并**成一张扁平表，只保留填充目录条目所需的
能力字段：

    {
      "schemaVersion": 1,
      "generatedAt": "2026-09-16T...",
      "sourceUrl": "https://models.dev/api.json",
      "models": {
        "deepseek-v4-pro": {
          "displayName": "DeepSeek V4 Pro",
          "contextWindow": 1000000,
          "maxOutputTokens": 384000,
          "supportsImageInput": false,
          "supportsToolCall": true,
          "supportsReasoning": true,
          "reasoningLevels": ["high", "max"],
          "releaseDate": "2026-08-01"
        }
      }
    }

归并规则：
- 同一模型 id 出现在多家厂商时，取**多数表决**结果，而不是极值：转售商常写错
  上下文（例如 `deepseek-v4.1-flash` 有 1000000 / 1048576 / 1050000 三种值，
  众数才是可信的）。数值并列时偏向 2 的幂，再偏向较大值；
- 布尔能力（图片/工具/推理）取**并集**：上游声明得更全时不应被写窄；
- 输出按 id 排序，便于人工 diff 与审阅。

用法：
    python3 scripts/generate-model-library.py           # 联网拉取并生成
    python3 scripts/generate-model-library.py --input X  # 用本地 api.json 生成
"""

from __future__ import annotations

from collections import Counter

import argparse
import datetime
import json
import pathlib
import sys
import urllib.request

SOURCE_URL = "https://models.dev/api.json"
OUTPUT = pathlib.Path(__file__).resolve().parent.parent / "src/ai_gateway/model_library.json"
SCHEMA_VERSION = 1

# 只有这些字段参与运行时的能力填充。
CAPABILITY_KEYS = (
    "displayName",
    "contextWindow",
    "maxOutputTokens",
    "supportsImageInput",
    "supportsToolCall",
    "supportsReasoning",
    "reasoningLevels",
    "releaseDate",
)


def compact(entry: dict) -> dict:
    """把 models.dev 的一条记录压成内置库需要的字段。"""
    limit = entry.get("limit") or {}
    inputs = ((entry.get("modalities") or {}).get("input")) or []
    levels = []
    for option in entry.get("reasoning_options") or []:
        if isinstance(option, dict) and option.get("type") == "effort":
            for value in option.get("values") or []:
                if isinstance(value, str) and value not in levels:
                    levels.append(value)

    compacted = {
        "displayName": (entry.get("name") or "").strip() or entry["id"],
        "contextWindow": limit.get("context"),
        "maxOutputTokens": limit.get("output"),
        "supportsImageInput": any(
            isinstance(value, str) and value.lower() == "image" for value in inputs
        ),
        "supportsToolCall": bool(entry.get("tool_call")),
        "supportsReasoning": bool(entry.get("reasoning")),
        "reasoningLevels": levels,
        "releaseDate": entry.get("release_date"),
    }
    return {key: compacted[key] for key in CAPABILITY_KEYS if compacted.get(key) not in (None, [], False)}


def preferred_number(values: "Counter[int]") -> int | None:
    """多数表决取上下文值；并列时偏向 2 的幂，再偏向较大值。"""
    if not values:
        return None
    top = max(values.values())
    tied = [value for value, count in values.items() if count == top]

    def rank(value: int) -> tuple:
        is_power_of_two = value > 0 and (value & (value - 1)) == 0
        return (int(is_power_of_two), value)

    return max(tied, key=rank)


class Accumulator:
    """按模型 id 累积同名字段，最后用多数表决 + 并集生成一条记录。"""

    def __init__(self) -> None:
        self.contexts: "Counter[int]" = Counter()
        self.outputs: "Counter[int]" = Counter()
        self.names: "Counter[str]" = Counter()
        self.dates: "Counter[str]" = Counter()
        self.images = False
        self.tools = False
        self.reasoning = False
        self.levels: list[str] = []

    def add(self, entry: dict) -> None:
        if entry.get("contextWindow"):
            self.contexts[entry["contextWindow"]] += 1
        if entry.get("maxOutputTokens"):
            self.outputs[entry["maxOutputTokens"]] += 1
        if entry.get("displayName"):
            self.names[entry["displayName"]] += 1
        if entry.get("releaseDate"):
            self.dates[entry["releaseDate"]] += 1
        self.images = self.images or bool(entry.get("supportsImageInput"))
        self.tools = self.tools or bool(entry.get("supportsToolCall"))
        self.reasoning = self.reasoning or bool(entry.get("supportsReasoning"))
        for level in entry.get("reasoningLevels") or []:
            if level not in self.levels:
                self.levels.append(level)

    def result(self, model_id: str) -> dict:
        level_order = ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
        levels = sorted(
            self.levels,
            key=lambda level: level_order.index(level) if level in level_order else len(level_order),
        )
        built = {
            # 显示名取出现最多的一种写法（大小写与空格差异很常见）。
            "displayName": self.names.most_common(1)[0][0] if self.names else model_id,
            "contextWindow": preferred_number(self.contexts),
            "maxOutputTokens": preferred_number(self.outputs),
            "supportsImageInput": self.images,
            "supportsToolCall": self.tools,
            "supportsReasoning": self.reasoning,
            "reasoningLevels": levels,
            "releaseDate": max(self.dates) if self.dates else None,
        }
        return {
            key: built[key]
            for key in CAPABILITY_KEYS
            if built.get(key) not in (None, [], False)
        }


def build(raw: dict) -> dict:
    accumulators: dict[str, Accumulator] = {}
    for provider in raw.values():
        if not isinstance(provider, dict):
            continue
        for key, entry in (provider.get("models") or {}).items():
            if not isinstance(entry, dict):
                continue
            model_id = (entry.get("id") or key or "").strip()
            if not model_id:
                continue
            accumulators.setdefault(model_id, Accumulator()).add(
                compact({**entry, "id": model_id})
            )

    ordered = {
        model_id: accumulators[model_id].result(model_id)
        for model_id in sorted(accumulators, key=str.lower)
    }
    return {
        "schemaVersion": SCHEMA_VERSION,
        "generatedAt": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "sourceUrl": SOURCE_URL,
        "models": ordered,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=pathlib.Path, help="本地 api.json（默认联网拉取）")
    parser.add_argument("--output", type=pathlib.Path, default=OUTPUT)
    args = parser.parse_args()

    if args.input:
        raw = json.loads(args.input.read_text())
        print(f"已读取本地目录：{args.input}", file=sys.stderr)
    else:
        print(f"正在拉取 {SOURCE_URL} …", file=sys.stderr)
        with urllib.request.urlopen(SOURCE_URL, timeout=120) as response:
            raw = json.loads(response.read().decode("utf-8"))

    library = build(raw)
    args.output.write_text(json.dumps(library, ensure_ascii=False, indent=1) + "\n")
    size_kb = args.output.stat().st_size / 1024
    print(
        f"已生成 {args.output}：{len(library['models'])} 个模型，{size_kb:.0f} KB",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
