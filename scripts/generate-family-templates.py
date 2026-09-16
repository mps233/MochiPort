#!/usr/bin/env python3
"""生成协议家族模板表 `src/ai_gateway/family_templates.json`。

自动合成目录条目时需要两类信息：

1. **协议字段**（`comp_hash`、`use_responses_lite`、`tool_mode`、`shell_type`、
   `apply_patch_tool_type`、`truncation_policy`、提示词等 Codex 私有概念）——
   本表提供；
2. **能力字段**（显示名、上下文窗口、图片输入）——`model_library.json` 提供。

改版前这些协议字段是"从内置目录里挑一条真实模型整条复制过来"（例如
Chat Completions 家族借用 `deepseek-v4.1-flash` 条目）。那会让这些条目出现在
用户的模型列表里，看起来像"没人提供的模型"，而它们其实只是模板。

本脚本把每个家族对应的模板条目里的协议字段抽出来，固化成本表；此后
`models.json` 里是否存在那条模型都不再影响合成。

用法：
    python3 scripts/generate-family-templates.py
"""

from __future__ import annotations

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
MODELS = ROOT / "src/ai_gateway/models.json"
OUTPUT = ROOT / "src/ai_gateway/family_templates.json"
SCHEMA_VERSION = 1

# 家族名 → 内置目录里的模板条目。与 Rust 侧 `ModelFamily` 的 serde 名一致。
FAMILY_SOURCE = {
    "open_ai_responses": "gpt-5.5",
    "deepseek_responses": "deepseek-v4-pro",
    "grok_responses": "grok-4.6",
    "chat_completions": "deepseek-v4.1-flash",
    "anthropic_messages": "GLM-5.2",
}

# 每个模型自己决定、不从模板继承的字段。
PER_MODEL = {
    "slug",
    "display_name",
    "description",
    "visibility",
    "supported_in_api",
    "context_window",
    "max_context_window",
    "supports_image_detail_original",
    "input_modalities",
    "availability_nux",
    "priority",
}

# 模板里属于具体官方模型的运营字段，合成时不带过去。
DROPPED = {
    "available_in_plans",
    "service_tiers",
    "additional_speed_tiers",
    "upgrade",
}

# 家族的保守兜底上下文（库和上游都没给时使用）。
FALLBACK_CONTEXT = {
    "open_ai_responses": 272_000,
    "deepseek_responses": 372_000,
    "grok_responses": 372_000,
    "chat_completions": 128_000,
    "anthropic_messages": 372_000,
}

FALLBACK_DESCRIPTION = {
    "open_ai_responses": "OpenAI Responses-compatible model served through MochiPort.",
    "deepseek_responses": "DeepSeek Responses-compatible model served through MochiPort.",
    "grok_responses": "Grok Responses-compatible model served through MochiPort.",
    "chat_completions": "Chat Completions-compatible model served through MochiPort.",
    "anthropic_messages": "Anthropic Messages-compatible model served through MochiPort.",
}


def main() -> int:
    catalog = {entry["slug"]: entry for entry in json.loads(MODELS.read_text())["models"]}
    # 源条目可能已从内置目录删除（协议字段既然已固化到本表，那些条目就不再需要
    # 留在 models.json 里）。此时复用本表已有字段，保证脚本可重复运行。
    existing = {}
    if OUTPUT.exists():
        existing = json.loads(OUTPUT.read_text()).get("families", {})

    families = {}
    for family, slug in FAMILY_SOURCE.items():
        source = catalog.get(slug)
        if source is None:
            previous = existing.get(family)
            if not previous or not previous.get("fields"):
                raise SystemExit(
                    f"内置目录里找不到条目 {slug}（{family}），且 {OUTPUT.name} 里也没有可复用的字段"
                )
            print(f"提示：{slug}（{family}）已不在内置目录，沿用 {OUTPUT.name} 里的现有字段")
            fields = previous["fields"]
        else:
            fields = {
                key: value
                for key, value in source.items()
                if key not in PER_MODEL and key not in DROPPED
            }
        families[family] = {
            "sourceSlug": slug,
            "contextWindow": FALLBACK_CONTEXT[family],
            "description": FALLBACK_DESCRIPTION[family],
            "fields": fields,
        }

    payload = {
        "schemaVersion": SCHEMA_VERSION,
        "description": (
            "协议家族模板：合成目录条目时继承的 Codex 私有协议字段与提示词。"
            "能力字段（显示名/上下文/图片）来自 model_library.json。"
        ),
        "families": families,
    }
    OUTPUT.write_text(json.dumps(payload, ensure_ascii=False, indent=1) + "\n")
    summary = ", ".join(f"{name}={len(data['fields'])}" for name, data in families.items())
    print(f"已生成 {OUTPUT}（{OUTPUT.stat().st_size} 字节）：{summary}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
