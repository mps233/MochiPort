use std::{
    collections::HashSet,
    path::Path,
    sync::{LazyLock, RwLock},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::config::{AiGatewayConfig, CustomModelConfig, ModelFamily};

/// 目录条目来源：内置目录、用户声明、按服务商自动合成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogModelSource {
    Builtin,
    Custom,
    Auto,
}

/// 协议家族模板：自动合成目录条目时继承的**协议字段**。
///
/// 字段来自 `family_templates.json`（由 `scripts/generate-family-templates.py`
/// 从内置目录的模板条目抽取）。改版前是"从内置目录挑一条真实模型整条复制"，
/// 那会让模板条目出现在用户的模型列表里，看着像"没人提供的模型"。
///
/// 能力字段（显示名/上下文/图片）不在这里，而由 `model_library.json` 提供。
struct ModelFamilyTemplate {
    /// 保守的默认上下文窗口（数据库与上游都没给时使用）。
    context_window: u64,
    /// 默认说明文案。
    description: &'static str,
    /// 该家族的协议字段。
    fields: &'static Value,
}

static FAMILY_TEMPLATES: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("family_templates.json"))
        .expect("embedded AI Gateway family templates")
});

fn family_template(family: ModelFamily) -> ModelFamilyTemplate {
    let key = family_key(family);
    let entry = FAMILY_TEMPLATES
        .get("families")
        .and_then(|families| families.get(key));
    ModelFamilyTemplate {
        context_window: entry
            .and_then(|entry| entry.get("contextWindow"))
            .and_then(Value::as_u64)
            .unwrap_or(128_000),
        description: entry
            .and_then(|entry| entry.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("Model served through MochiPort."),
        fields: entry
            .and_then(|entry| entry.get("fields"))
            .unwrap_or(&Value::Null),
    }
}

/// 家族在模板表里的键（与 `ModelFamily` 的 serde 名一致）。
fn family_key(family: ModelFamily) -> &'static str {
    match family {
        ModelFamily::OpenAiResponses => "open_ai_responses",
        ModelFamily::DeepSeekResponses => "deepseek_responses",
        ModelFamily::GrokResponses => "grok_responses",
        ModelFamily::ChatCompletions => "chat_completions",
        ModelFamily::AnthropicMessages => "anthropic_messages",
    }
}

static BASE_MODEL_CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("models.json")).expect("embedded AI Gateway model catalog")
});

/// 内置模型库：模型名 → 能力（来源 models.dev，由
/// `scripts/generate-model-library.py` 生成）。
///
/// 用来在服务商声明了某个模型、但本地既没有内置条目也没有自定义条目时，
/// 自动按库里记录的能力填充（显示名 / 上下文 / 图片 / 推理档位）。
/// 协议字段仍由协议家族模板提供——模型库不含 Codex 私有协议字段。
static MODEL_LIBRARY: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("model_library.json"))
        .expect("embedded AI Gateway model library")
});

/// 从内置模型库查一个模型的能力记录。
fn library_model(model_id: &str) -> Option<&'static Value> {
    let models = MODEL_LIBRARY.get("models")?.as_object()?;
    // 先精确匹配，再忽略大小写兜底（上游大小写写法不统一）。
    if let Some(entry) = models.get(model_id) {
        return Some(entry);
    }
    models
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(model_id.trim()))
        .map(|(_, entry)| entry)
}

/// 当前生效的官方覆盖层。
#[derive(Default)]
struct OfficialOverlay {
    /// 官方条目：按 slug 覆盖/追加内置目录。
    models: Vec<Value>,
    /// 官方**曾经提供过**的 slug：用来识别官方已下线的模型。
    known_slugs: Vec<String>,
}

/// 官方目录同步的覆盖层。
///
/// daemon 启动时与之后每 24 小时由 `model_sync` 刷新；这里只持有当前生效的
/// 快照，读目录时与内置目录合并，因此官方更新无需改源码或重建二进制。
static OFFICIAL_OVERLAY: LazyLock<RwLock<OfficialOverlay>> =
    LazyLock::new(|| RwLock::new(OfficialOverlay::default()));

/// 用覆盖文件里的官方条目替换当前覆盖层；传空表示清空、回落到内置目录。
pub(crate) fn set_official_overlay(models: Vec<Value>, known_slugs: Vec<String>) {
    if let Ok(mut guard) = OFFICIAL_OVERLAY.write() {
        guard.models = models;
        guard.known_slugs = known_slugs;
    }
}

/// 内置目录原文（同步时用来判断官方新增了哪些 slug）。
pub(crate) fn embedded_catalog() -> Value {
    BASE_MODEL_CATALOG.clone()
}

/// 从数据目录读取覆盖文件并生效，返回生效的官方条目数。
pub(crate) fn load_official_overlay(data_dir: &Path) -> usize {
    let overlay = super::model_sync::read_overlay(&super::model_sync::overlay_path(data_dir));
    let (models, mut known) = overlay
        .map(|overlay| (overlay.models, overlay.known_official_slugs))
        .unwrap_or_default();
    // 兜底：内置目录里的官方条目本来就从官方抄来，必须计入"官方提供过"的集合，
    // 否则旧覆盖文件（缺 knownOfficialSlugs）会让官方下线识别失效。
    for slug in super::model_sync::embedded_official_slugs(&BASE_MODEL_CATALOG) {
        if !known
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&slug))
        {
            known.push(slug);
        }
    }
    let count = models.len();
    set_official_overlay(models, known);
    count
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModelOption {
    pub slug: String,
    pub display_name: String,
    pub description: String,
    pub source: CatalogModelSource,
    /// 生效的上下文窗口（内置目录 / 自定义声明 / 上游声明 / 家族兜底）。
    pub context_window: Option<u64>,
    /// 是否接受图片输入。
    pub supports_image_input: Option<bool>,
    /// 声明了这个模型名的已启用服务商（按配置顺序）。
    pub providers: Vec<String>,
    /// 当前配置的归属服务商；为空表示按权重 + 会话粘性自动选路。
    pub owner: Option<String>,
    /// 归属是否仍然生效（provider 存在、启用且仍声明该模型）。
    pub owner_effective: bool,
}

#[cfg(test)]
pub fn visible_catalog_model_options() -> Vec<CatalogModelOption> {
    visible_catalog_model_options_for(None)
}

fn visible_catalog_model_options_for(config: Option<&AiGatewayConfig>) -> Vec<CatalogModelOption> {
    catalog_models()
        .iter()
        .filter(|model| is_catalog_model_visible(model))
        .filter_map(|model| catalog_model_option(model, CatalogModelSource::Builtin, config))
        .collect()
}

/// 与 [`visible_catalog_model_options`] 相同，但补上声明该模型的服务商与归属。
pub fn visible_catalog_model_options_with_config(
    config: &AiGatewayConfig,
) -> Vec<CatalogModelOption> {
    let mut options = visible_catalog_model_options_for(Some(config));
    for option in &mut options {
        option.providers = providers_declaring(config, &option.slug);
    }
    annotate_ownership(config, options)
}

/// GUI「Codex 可用模型」列表：内置目录 + 自定义条目 + 服务商自动项，按名字去重
/// （内置优先），并补上声明它的服务商与当前归属。
///
/// 内置目录里**没有任何启用服务商提供**的条目会被滤掉：它们只是官方目录里
/// 存在定义（例如官方 `gpt-5.6-luna`），本地并没有上游能提供，勾选后请求必然
/// 失败。用户显式声明的自定义条目不受此限制——那是用户自己的意图。
pub fn gui_catalog_model_options(config: &AiGatewayConfig) -> Vec<CatalogModelOption> {
    let mut seen = std::collections::HashSet::new();
    let options = visible_catalog_model_options_with_config(config)
        .into_iter()
        .chain(custom_catalog_model_options(config))
        .chain(auto_catalog_model_options(config))
        .filter(|option| seen.insert(option.slug.to_ascii_lowercase()))
        .map(|mut option| {
            // providers 以配置为准统一重算，避免同名被多家声明时只记到第一家。
            option.providers = providers_declaring(config, &option.slug);
            option
        })
        .filter(|option| {
            // 内置目录条目必须有服务商提供才展示；自定义/自动条目保持原样。
            option.source != CatalogModelSource::Builtin || !option.providers.is_empty()
        })
        .collect();
    annotate_ownership(config, options)
}

/// 给选项补归属信息（未配置归属时为 None / 未生效为 false）。
pub fn annotate_ownership(
    config: &AiGatewayConfig,
    options: Vec<CatalogModelOption>,
) -> Vec<CatalogModelOption> {
    options
        .into_iter()
        .map(|mut option| {
            option.owner = config.model_owner(&option.slug).map(str::to_string);
            option.owner_effective = config.effective_model_owner(&option.slug).is_some();
            option
        })
        .collect()
}

/// 用户显式声明的自定义模型条目。
pub fn custom_catalog_model_options(config: &AiGatewayConfig) -> Vec<CatalogModelOption> {
    config
        .custom_models
        .iter()
        .filter_map(|entry| custom_model_option(config, entry))
        .map(|mut option| {
            option.providers = providers_declaring(config, &option.slug);
            option
        })
        .collect()
}

/// 服务商已声明、但不在内置目录/自定义条目里的模型名。
///
/// 这些模型会被自动合成为保守的目录条目，这里提前列出来供 GUI 勾选。
pub fn auto_catalog_model_options(config: &AiGatewayConfig) -> Vec<CatalogModelOption> {
    let mut options = Vec::new();
    let mut seen = HashSet::new();
    for provider in config.providers.iter().filter(|provider| provider.enabled) {
        let names = provider
            .models
            .iter()
            .chain(provider.model_aliases.keys())
            .map(|name| name.trim())
            .filter(|name| !name.is_empty());
        for name in names {
            if !seen.insert(name.to_ascii_lowercase()) {
                continue;
            }
            if builtin_catalog_model(name).is_some() || config.custom_model(name).is_some() {
                continue;
            }
            let family = ModelFamily::from_provider_type(&provider.provider_type);
            let template = family_template(family);
            let discovered = provider.discovered_model(name);
            let base_display_name = discovered
                .and_then(|entry| entry.display_name.clone())
                .unwrap_or_else(|| name.to_string());
            options.push(CatalogModelOption {
                slug: name.to_string(),
                display_name: match config.model_display_prefix(name) {
                    Some(prefix) => format!("{prefix} · {base_display_name}"),
                    None => base_display_name,
                },
                description: template.description.to_string(),
                source: CatalogModelSource::Auto,
                context_window: Some(
                    discovered
                        .and_then(|entry| entry.context_window)
                        .unwrap_or(template.context_window),
                ),
                supports_image_input: Some(
                    discovered
                        .and_then(|entry| entry.supports_image_input)
                        .unwrap_or(false),
                ),
                providers: vec![provider.name.clone()],
                owner: None,
                owner_effective: false,
            });
        }
    }
    options.sort_by(|left, right| left.slug.cmp(&right.slug));
    options
}

/// 声明了该模型名的已启用服务商。
fn providers_declaring(config: &AiGatewayConfig, model: &str) -> Vec<String> {
    config
        .providers
        .iter()
        .filter(|provider| provider.enabled && provider.matches_model(model))
        .map(|provider| provider.name.clone())
        .collect()
}

fn catalog_model_option(
    model: &Value,
    source: CatalogModelSource,
    config: Option<&AiGatewayConfig>,
) -> Option<CatalogModelOption> {
    let slug = model_slug(model)?.to_string();
    let base_display_name = model
        .get("display_name")
        .and_then(Value::as_str)
        .unwrap_or(&slug)
        .to_string();
    let display_name = match config.and_then(|config| config.model_display_prefix(&slug)) {
        Some(prefix) => format!("{prefix} · {base_display_name}"),
        None => base_display_name,
    };
    let description = model
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some(CatalogModelOption {
        slug,
        display_name,
        description,
        source,
        context_window: model.get("context_window").and_then(Value::as_u64),
        supports_image_input: catalog_model_supports_image(model),
        providers: Vec::new(),
        owner: None,
        owner_effective: false,
    })
}

fn catalog_model_supports_image(model: &Value) -> Option<bool> {
    model
        .get("input_modalities")
        .and_then(Value::as_array)
        .map(|values| {
            values.iter().any(|value| {
                value
                    .as_str()
                    .is_some_and(|text| text.eq_ignore_ascii_case("image"))
            })
        })
}

fn custom_model_option(
    config: &AiGatewayConfig,
    entry: &CustomModelConfig,
) -> Option<CatalogModelOption> {
    let slug = entry.normalized_slug()?.to_string();
    let family = entry.resolve_family(config);
    let template = family_template(family);
    let base_display_name = entry
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&slug)
        .to_string();
    Some(CatalogModelOption {
        display_name: match config.model_display_prefix(&slug) {
            Some(prefix) => format!("{prefix} · {base_display_name}"),
            None => base_display_name,
        },
        description: entry
            .description
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or(template.description)
            .to_string(),
        slug,
        source: CatalogModelSource::Custom,
        context_window: Some(
            entry
                .context_window
                .filter(|value| *value > 0)
                .unwrap_or(template.context_window),
        ),
        supports_image_input: Some(entry.supports_image_input.unwrap_or(false)),
        providers: Vec::new(),
        owner: None,
        owner_effective: false,
    })
}

#[cfg(test)]
pub fn configured_models_response(config: &AiGatewayConfig) -> Value {
    build_configured_models_response(config)
}

pub fn configured_models_etag(config: &AiGatewayConfig) -> String {
    let response = build_configured_models_response(config);
    configured_models_etag_from_response(&response)
}

pub fn configured_models_response_with_etag(config: &AiGatewayConfig) -> (Value, String) {
    let response = build_configured_models_response(config);
    let etag = configured_models_etag_from_response(&response);
    (response, etag)
}

fn build_configured_models_response(config: &AiGatewayConfig) -> Value {
    let mut emitted = HashSet::new();
    let mut models = Vec::new();
    let mut priority = 0;

    for model_id in selected_codex_model_ids(config) {
        if !emitted.insert(model_id.clone()) {
            continue;
        }

        // 三层解析：内置目录 → 用户自定义条目 → 按服务商协议家族自动合成。
        let model = builtin_catalog_model(&model_id)
            .or_else(|| synthesize_catalog_model(config, &model_id));
        let Some(mut model) = model else {
            continue;
        };
        normalize_catalog_model(&mut model);
        apply_display_prefix(config, &model_id, &mut model);
        if let Some(object) = model.as_object_mut() {
            object.insert("priority".to_string(), json!(priority));
        }
        priority += 1;
        models.push(model);
    }

    json!({ "models": models })
}

/// 按配置给 `display_name` 加服务商前缀（`AutoClaw · GLM-5.3-Flash`）。
///
/// 只影响展示，不改 `slug`，因此 Codex 发回的名字与路由完全不变。
fn apply_display_prefix(config: &AiGatewayConfig, model_id: &str, model: &mut Value) {
    let Some(prefix) = config.model_display_prefix(model_id) else {
        return;
    };
    let Some(object) = model.as_object_mut() else {
        return;
    };
    let Some(display_name) = object.get("display_name").and_then(Value::as_str) else {
        return;
    };
    let display_name = display_name.trim();
    if display_name.is_empty() || display_name.starts_with(&format!("{prefix} · ")) {
        return;
    }
    object.insert(
        "display_name".to_string(),
        json!(format!("{prefix} · {display_name}")),
    );
}

fn builtin_catalog_model(model_id: &str) -> Option<Value> {
    catalog_models()
        .into_iter()
        .find(|model| model_slug(model) == Some(model_id) && is_catalog_model_visible(model))
}

/// 为内置目录之外的模型合成一个最小可用条目：继承协议家族的模板字段
/// （`comp_hash`、`use_responses_lite`、`shell_type`、推理等级等），
/// 能力默认保守（纯文本），只有显式声明才接受图片输入。
fn synthesize_catalog_model(config: &AiGatewayConfig, model_id: &str) -> Option<Value> {
    let custom = config.custom_model(model_id);
    let provider = config
        .provider_for_custom_model(model_id)
        .filter(|provider| provider.matches_model(model_id));
    // 既没有自定义条目、也没有任何 provider 声明该模型名时保持原有行为：跳过。
    if custom.is_none() && provider.is_none() {
        return None;
    }
    let family = custom
        .map(|entry| entry.resolve_family(config))
        .or_else(|| {
            provider.map(|provider| ModelFamily::from_provider_type(&provider.provider_type))
        })
        .unwrap_or(ModelFamily::ChatCompletions);
    let template = family_template(family);
    // 直接从协议模板表构造条目：不再依赖内置目录里存在某个具体的"模板模型"。
    let mut model = template
        .fields
        .as_object()
        .map(|fields| Value::Object(fields.clone()))
        .unwrap_or_else(|| json!({}));

    // 能力优先级：自定义条目 → 内置模型库 → 上游 /models 声明 → 家族保守兜底。
    //
    // 内置模型库放在上游声明之前：它来自社区维护的 models.dev，对"这个模型本来
    // 有什么能力"更权威；上游只声明自己提供的子集，且常把上下文写窄。
    let discovered = provider.and_then(|provider| provider.discovered_model(model_id));
    let library = library_model(model_id);
    let vision = custom
        .and_then(|entry| entry.supports_image_input)
        .or_else(|| library_bool(library, "supportsImageInput"))
        .or_else(|| discovered.and_then(|entry| entry.supports_image_input))
        .unwrap_or(false);
    let context_window = custom
        .and_then(|entry| entry.context_window)
        .filter(|value| *value > 0)
        .or_else(|| library_u64(library, "contextWindow"))
        .or_else(|| discovered.and_then(|entry| entry.context_window))
        .unwrap_or(template.context_window);
    let display_name = custom
        .and_then(|entry| entry.display_name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| library_display_name(library))
        .or_else(|| discovered.and_then(|entry| entry.display_name.clone()))
        .unwrap_or_else(|| model_id.to_string());
    let description = custom
        .and_then(|entry| entry.description.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(template.description)
        .to_string();

    let object = model.as_object_mut()?;
    object.insert("slug".to_string(), json!(model_id));
    object.insert("display_name".to_string(), json!(display_name));
    object.insert("description".to_string(), json!(description));
    object.insert("visibility".to_string(), json!("list"));
    object.insert("supported_in_api".to_string(), json!(true));
    object.insert("context_window".to_string(), json!(context_window));
    object.insert("max_context_window".to_string(), json!(context_window));
    object.insert("supports_image_detail_original".to_string(), json!(vision));
    object.insert(
        "input_modalities".to_string(),
        if vision {
            json!(["text", "image"])
        } else {
            json!(["text"])
        },
    );
    object.insert(
        "availability_nux".to_string(),
        json!({ "message": format!("{model_id} is now available in Codex.") }),
    );
    // 运营字段（available_in_plans / service_tiers / upgrade 等）在生成
    // family_templates.json 时就已剔除，这里无需再删。
    Some(model)
}

/// 内置目录 + 官方同步覆盖层（覆盖层按 slug 优先）。
fn catalog_models() -> Vec<Value> {
    let embedded = BASE_MODEL_CATALOG
        .get("models")
        .and_then(Value::as_array)
        .expect("embedded AI Gateway model catalog must contain models array");
    let (overlay, known_official_slugs) = OFFICIAL_OVERLAY
        .read()
        .map(|guard| (guard.models.clone(), guard.known_slugs.clone()))
        .unwrap_or_default();
    merge_catalog_with_official(embedded, &overlay, &known_official_slugs)
}

/// 合并逻辑的纯函数形态：方便测试，不依赖进程级全局状态。
///
/// - `overlay` 里出现的 slug：用官方条目替换内置条目（官方字段优先，
///   但保留内置的 `base_instructions` 等官方目录不提供的本地字段）；
/// - `overlay` 新增的 slug：追加到末尾；
/// - `known_official_slugs` 里出现过、且本次 `overlay` 已没有的 slug：
///   视为官方下线并移除（只影响官方模型，项目自建条目永不受影响）。
fn merge_catalog_with_official(
    embedded: &[Value],
    overlay: &[Value],
    known_official_slugs: &[String],
) -> Vec<Value> {
    if overlay.is_empty() {
        return embedded.to_vec();
    }

    // 覆盖层里出现的 slug 用官方版本替换内置版本，其余内置条目保持原样；
    // 覆盖层新增的 slug（官方新增模型）追加到末尾。
    let mut overridden: HashSet<String> = HashSet::new();
    let mut merged: Vec<Value> = Vec::with_capacity(embedded.len() + overlay.len());
    for official in overlay.iter() {
        let Some(slug) = model_slug(official).map(|slug| slug.to_ascii_lowercase()) else {
            continue;
        };
        overridden.insert(slug);
        merged.push(merge_official_entry(
            embedded.iter().find(|model| {
                model_slug(model).is_some_and(|existing| {
                    existing.eq_ignore_ascii_case(model_slug(official).unwrap_or_default())
                })
            }),
            official,
        ));
    }
    for model in embedded.iter() {
        let Some(slug) = model_slug(model) else {
            merged.push(model.clone());
            continue;
        };
        if overridden.contains(&slug.to_ascii_lowercase()) {
            continue;
        }
        // 官方曾提供过、但本次官方目录里已经没有了 → 跟随官方下线。
        // 只有"官方历史集合"里的 slug 才会被这样处理，项目自建的第三方条目
        // （grok / GLM / Claude / deepseek 等）永远不受影响。
        if known_official_slugs
            .iter()
            .any(|known| known.eq_ignore_ascii_case(slug))
        {
            continue;
        }
        merged.push(model.clone());
    }
    merged
}

fn library_bool(library: Option<&Value>, key: &str) -> Option<bool> {
    library?.get(key).and_then(Value::as_bool)
}

fn library_u64(library: Option<&Value>, key: &str) -> Option<u64> {
    library?
        .get(key)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
}

fn library_display_name(library: Option<&Value>) -> Option<String> {
    let name = library?.get("displayName").and_then(Value::as_str)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// 合并官方条目与内置条目：官方字段优先，但保留内置里官方不提供的本地字段
/// （主要是 `base_instructions`，Codex 会读它）。
fn merge_official_entry(embedded: Option<&Value>, official: &Value) -> Value {
    let mut merged = official.clone();
    let Some(embedded) = embedded else {
        return merged;
    };
    let (Some(merged_object), Some(embedded_object)) =
        (merged.as_object_mut(), embedded.as_object())
    else {
        return merged;
    };
    for key in ["base_instructions"] {
        if !merged_object.contains_key(key)
            && let Some(value) = embedded_object.get(key)
        {
            merged_object.insert(key.to_string(), value.clone());
        }
    }
    merged
}

fn configured_models_etag_from_response(response: &Value) -> String {
    let serialized = serde_json::to_vec(response)
        .expect("configured models response should always serialize for etag");
    let digest = Sha256::digest(serialized);
    format!("\"sha256:{}\"", hex::encode(digest))
}

fn selected_codex_model_ids(config: &AiGatewayConfig) -> Vec<String> {
    config
        .codex_visible_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn model_slug(model: &Value) -> Option<&str> {
    model.get("slug").and_then(Value::as_str)
}

fn is_catalog_model_visible(model: &Value) -> bool {
    model
        .get("supported_in_api")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && model.get("visibility").and_then(Value::as_str) == Some("list")
}

fn normalize_catalog_model(model: &mut Value) {
    normalize_deepseek_model(model);
}

/// DeepSeek 家族的兜底口径：**只补缺省值**，条目里显式声明的能力优先。
///
/// 这样新增 deepseek 变体（例如 v4.1/v4.2 的视觉版本）不必再改这段 Rust。
fn normalize_deepseek_model(model: &mut Value) {
    let Some(slug) = model_slug(model).map(str::to_string) else {
        return;
    };
    if !slug.starts_with("deepseek-") {
        return;
    }

    // 内置目录里声明支持视觉的 flash 系列是兜底值，不覆盖显式声明。
    let vision_default = matches!(slug.as_str(), "deepseek-v4-flash" | "deepseek-v4.1-flash");
    let Some(object) = model.as_object_mut() else {
        return;
    };
    object
        .entry("web_search_tool_type")
        .or_insert_with(|| json!("text"));
    let vision = object
        .get("supports_image_detail_original")
        .and_then(Value::as_bool)
        .unwrap_or(vision_default);
    object.insert(
        "supports_image_detail_original".to_string(),
        Value::Bool(vision),
    );
    object.entry("input_modalities").or_insert_with(|| {
        if vision {
            json!(["text", "image"])
        } else {
            json!(["text"])
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::config::{DiscoveredModel, ProviderConfig, ProviderType};

    fn config(models: &[&str]) -> AiGatewayConfig {
        AiGatewayConfig {
            codex_visible_models: models.iter().map(|model| model.to_string()).collect(),
            ..Default::default()
        }
    }

    /// 内置目录之外、且没有任何 provider 声明的模型名仍然被跳过。
    #[test]
    fn unknown_model_without_a_provider_is_skipped() {
        let response = configured_models_response(&config(&["custom-model"]));
        assert!(
            response["models"].as_array().unwrap().is_empty(),
            "undeclared models must not reach the Codex catalog"
        );
    }

    #[test]
    fn provider_models_are_synthesized_with_family_defaults() {
        let mut config = config(&["some-chat-model"]);
        config.providers = vec![ProviderConfig {
            name: "proxy".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["some-chat-model".to_string()],
            ..Default::default()
        }];

        let response = configured_models_response(&config);
        let model = &response["models"][0];
        assert_eq!(model["slug"], "some-chat-model");
        assert_eq!(model["display_name"], "some-chat-model");
        assert_eq!(model["visibility"], "list");
        assert_eq!(model["supported_in_api"], true);
        // Chat Completions 家族模板：comp_hash 3000、非 lite。
        assert_eq!(model["comp_hash"], "3000");
        assert_eq!(model["use_responses_lite"], false);
        // 保守默认：纯文本、128k 上下文。
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["supports_image_detail_original"], false);
        assert_eq!(model["context_window"], 128_000);
        assert_eq!(model["max_context_window"], 128_000);
    }

    #[test]
    fn anthropic_family_synthesis_uses_the_anthropic_comp_hash() {
        let mut config = config(&["my-claude"]);
        config.providers = vec![ProviderConfig {
            name: "anthropic-proxy".to_string(),
            enabled: true,
            provider_type: ProviderType::AnthropicMessages,
            base_url: "https://example.invalid/v1".to_string(),
            models: vec!["my-claude".to_string()],
            ..Default::default()
        }];

        let response = configured_models_response(&config);
        assert_eq!(
            response["models"][0]["comp_hash"],
            "codexhub-anthropic-summary-v1"
        );
        assert_eq!(response["models"][0]["context_window"], 372_000);
    }

    #[test]
    fn custom_model_entries_control_display_name_context_and_vision() {
        let mut config = config(&["deepseek-v4.1-flash"]);
        config.custom_models = vec![CustomModelConfig {
            slug: "deepseek-v4.1-flash".to_string(),
            display_name: Some("DeepSeek-V4.1-Flash".to_string()),
            description: None,
            context_window: Some(1_048_576),
            supports_image_input: Some(true),
            ..Default::default()
        }];
        config.providers = vec![ProviderConfig {
            name: "autoclaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["deepseek-v4.1-flash".to_string()],
            ..Default::default()
        }];

        // 内置目录条目优先于自定义条目。
        let builtin = configured_models_response(&config);
        assert_eq!(builtin["models"][0]["display_name"], "DeepSeek-V4.1-Flash");
        assert_eq!(builtin["models"][0]["context_window"], 1_048_576);

        // 同一个 slug 不在内置目录时，自定义能力生效。
        config.codex_visible_models = vec!["vendor-new-model".to_string()];
        config.custom_models = vec![CustomModelConfig {
            slug: "vendor-new-model".to_string(),
            provider_name: Some("autoclaw".to_string()),
            display_name: Some("Vendor New Model".to_string()),
            description: Some("自定义说明".to_string()),
            context_window: Some(200_000),
            supports_image_input: Some(true),
            ..Default::default()
        }];
        let custom = configured_models_response(&config);
        let model = &custom["models"][0];
        assert_eq!(model["slug"], "vendor-new-model");
        assert_eq!(model["display_name"], "Vendor New Model");
        assert_eq!(model["description"], "自定义说明");
        assert_eq!(model["context_window"], 200_000);
        assert_eq!(model["input_modalities"], json!(["text", "image"]));
        assert_eq!(model["supports_image_detail_original"], true);
        assert_eq!(model["comp_hash"], "3000");
    }

    #[test]
    fn explicit_deepseek_capabilities_survive_normalization() {
        // 条目显式声明纯文本时，归一化不得再按模型名硬编码改回视觉。
        let mut model = json!({
            "slug": "deepseek-v5-flash",
            "display_name": "DeepSeek-V5-Flash",
            "visibility": "list",
            "supported_in_api": true,
            "input_modalities": ["text"],
            "supports_image_detail_original": false,
            "comp_hash": "3000",
        });
        normalize_catalog_model(&mut model);
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["supports_image_detail_original"], false);
        assert_eq!(model["web_search_tool_type"], "text");

        // 缺省时仍按 deepseek flash 的兜底口径补齐。
        let mut bare = json!({ "slug": "deepseek-v4.1-flash" });
        normalize_catalog_model(&mut bare);
        assert_eq!(bare["input_modalities"], json!(["text", "image"]));
    }

    #[test]
    fn discovered_upstream_metadata_feeds_the_synthesized_entry() {
        let mut config = config(&["vendor-omni-model"]);
        config.providers = vec![ProviderConfig {
            name: "autoclaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["vendor-omni-model".to_string()],
            discovered_models: vec![DiscoveredModel {
                id: "vendor-omni-model".to_string(),
                display_name: Some("Vendor Omni".to_string()),
                context_window: Some(1_048_576),
                supports_image_input: Some(true),
            }],
            ..Default::default()
        }];

        let response = configured_models_response(&config);
        let model = &response["models"][0];
        // 上游明确声明的展示名/上下文/视觉能力优先于家族兜底值。
        assert_eq!(model["display_name"], "Vendor Omni");
        assert_eq!(model["context_window"], 1_048_576);
        assert_eq!(model["input_modalities"], json!(["text", "image"]));
        // 协议字段仍来自家族模板。
        assert_eq!(model["comp_hash"], "3000");
    }

    #[test]
    fn discovered_models_surface_in_the_auto_options_with_metadata() {
        let mut config = config(&["brand-new-model"]);
        config.providers = vec![ProviderConfig {
            name: "autoclaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["brand-new-model".to_string()],
            discovered_models: vec![DiscoveredModel {
                id: "brand-new-model".to_string(),
                display_name: Some("Brand New".to_string()),
                context_window: Some(1_048_576),
                supports_image_input: Some(true),
            }],
            ..Default::default()
        }];

        let options = auto_catalog_model_options(&config);
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].display_name, "Brand New");
        assert_eq!(options[0].context_window, Some(1_048_576));
        assert_eq!(options[0].supports_image_input, Some(true));
    }

    #[test]
    fn display_prefix_follows_the_provider_name() {
        let mut config = config(&["gpt-5.6-terra", "brand-new-model"]);
        config.providers = vec![
            ProviderConfig {
                name: "Mac_Local".to_string(),
                enabled: true,
                provider_type: ProviderType::OpenAiResponses,
                base_url: "http://127.0.0.1:8090".to_string(),
                models: vec!["gpt-5.6-terra".to_string(), "brand-new-model".to_string()],
                ..Default::default()
            },
            ProviderConfig {
                name: "autoclaw".to_string(),
                enabled: true,
                provider_type: ProviderType::ChatCompletions,
                base_url: "http://127.0.0.1:18800/v1".to_string(),
                models: vec!["brand-new-model".to_string()],
                ..Default::default()
            },
        ];
        config
            .model_owners
            .insert("brand-new-model".to_string(), "autoclaw".to_string());

        // 默认（全局开关关闭）不加前缀。
        let plain = configured_models_response(&config);
        assert_eq!(plain["models"][0]["display_name"], "GPT-5.6-Terra");
        assert_eq!(plain["models"][1]["display_name"], "brand-new-model");

        // 打开开关：前缀就是服务商的名字，逐条跟随。
        config.provider_display_prefix = true;
        let prefixed = configured_models_response(&config);
        assert_eq!(
            prefixed["models"][0]["display_name"],
            "Mac_Local · GPT-5.6-Terra"
        );
        // 归属为 autoclaw 的同名模型用 autoclaw 做前缀。
        assert_eq!(
            prefixed["models"][1]["display_name"],
            "autoclaw · brand-new-model"
        );
        // 只改显示，不改 slug（Codex 发回的名字与路由不变）。
        assert_eq!(prefixed["models"][1]["slug"], "brand-new-model");

        // 改服务商名，前缀自动跟着变（没有逐服务商覆盖字段）。
        config.providers[0].name = "公司网关".to_string();
        let renamed = configured_models_response(&config);
        assert_eq!(
            renamed["models"][0]["display_name"],
            "公司网关 · GPT-5.6-Terra"
        );
    }

    #[test]
    fn custom_entry_wins_over_discovered_metadata() {
        let mut config = config(&["brand-new-model"]);
        config.providers = vec![ProviderConfig {
            name: "autoclaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["brand-new-model".to_string()],
            discovered_models: vec![DiscoveredModel {
                id: "brand-new-model".to_string(),
                display_name: Some("Upstream Name".to_string()),
                context_window: Some(999),
                supports_image_input: Some(true),
            }],
            ..Default::default()
        }];
        config.custom_models = vec![CustomModelConfig {
            slug: "brand-new-model".to_string(),
            display_name: Some("我的名字".to_string()),
            context_window: Some(200_000),
            supports_image_input: Some(false),
            ..Default::default()
        }];

        let response = configured_models_response(&config);
        let model = &response["models"][0];
        assert_eq!(model["display_name"], "我的名字");
        assert_eq!(model["context_window"], 200_000);
        assert_eq!(model["input_modalities"], json!(["text"]));
    }

    #[test]
    fn gui_options_report_declaring_providers_and_ownership() {
        let mut config = config(&["gpt-5.6-terra", "vendor-shared"]);
        config.providers = vec![
            ProviderConfig {
                name: "openai".to_string(),
                enabled: true,
                provider_type: ProviderType::OpenAiResponses,
                base_url: "http://127.0.0.1:8090".to_string(),
                models: vec!["gpt-5.6-terra".to_string(), "vendor-shared".to_string()],
                ..Default::default()
            },
            ProviderConfig {
                name: "autoclaw".to_string(),
                enabled: true,
                provider_type: ProviderType::ChatCompletions,
                base_url: "http://127.0.0.1:18800/v1".to_string(),
                models: vec!["vendor-shared".to_string()],
                ..Default::default()
            },
        ];
        config
            .model_owners
            .insert("vendor-shared".to_string(), "autoclaw".to_string());

        let options = gui_catalog_model_options(&config);
        let terra = options
            .iter()
            .find(|option| option.slug == "gpt-5.6-terra")
            .expect("builtin entry");
        assert_eq!(terra.providers, vec!["openai".to_string()]);
        assert_eq!(terra.owner, None);
        assert!(!terra.owner_effective);

        let shared = options
            .iter()
            .find(|option| option.slug == "vendor-shared")
            .expect("auto entry");
        // 同名被两家声明，归属指向 autoclaw 且仍然生效。
        assert_eq!(
            shared.providers,
            vec!["openai".to_string(), "autoclaw".to_string()]
        );
        assert_eq!(shared.owner.as_deref(), Some("autoclaw"));
        assert!(shared.owner_effective);

        // 归属目标被禁用后标记为失效。
        config.providers[1].enabled = false;
        let options = gui_catalog_model_options(&config);
        let shared = options
            .iter()
            .find(|option| option.slug == "vendor-shared")
            .expect("auto entry");
        assert_eq!(shared.owner.as_deref(), Some("autoclaw"));
        assert!(!shared.owner_effective);
        assert_eq!(shared.providers, vec!["openai".to_string()]);
    }

    fn embedded_models() -> Vec<Value> {
        BASE_MODEL_CATALOG
            .get("models")
            .and_then(Value::as_array)
            .expect("embedded catalog")
            .clone()
    }

    #[test]
    fn built_in_library_fills_capabilities_for_provider_declared_models() {
        // kimi-k3 不在内置目录里，只在模型库里有记录；provider 仅声明模型名。
        let mut config = config(&["kimi-k3"]);
        config.providers = vec![ProviderConfig {
            name: "AutoClaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["kimi-k3".to_string()],
            ..Default::default()
        }];

        let model = configured_models_response(&config)
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| {
                models
                    .iter()
                    .find(|model| model_slug(model) == Some("kimi-k3"))
                    .cloned()
            })
            .expect("kimi-k3 should be synthesized");

        // 显示名/上下文/图片能力来自内置模型库，而不是家族模板的保守值。
        assert_eq!(model["display_name"], "Kimi K3");
        assert_eq!(model["context_window"], 1_048_576);
        assert_eq!(model["input_modalities"], json!(["text", "image"]));
    }

    #[test]
    fn built_in_library_is_case_insensitive_and_ignores_unknown_models() {
        assert!(library_model("glm-5.3-flash").is_some());
        assert!(library_model("GLM-5.3-FLASH").is_some());
        assert!(library_model("definitely-not-a-real-model-xyz").is_none());
        // 库里不应含 Codex 私有协议字段（那些只能来自本地家族模板）。
        let entry = library_model("glm-5.3-flash").expect("entry");
        for key in [
            "use_responses_lite",
            "tool_mode",
            "comp_hash",
            "base_instructions",
        ] {
            assert!(entry.get(key).is_none(), "library must not carry {key}");
        }
    }

    #[test]
    fn custom_entry_still_wins_over_the_built_in_library() {
        let mut config = config(&["kimi-k3"]);
        config.providers = vec![ProviderConfig {
            name: "AutoClaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["kimi-k3".to_string()],
            ..Default::default()
        }];
        config.custom_models = vec![CustomModelConfig {
            slug: "kimi-k3".to_string(),
            display_name: Some("我的 Kimi".to_string()),
            context_window: Some(128_000),
            supports_image_input: Some(false),
            ..Default::default()
        }];

        let model = configured_models_response(&config)
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| {
                models
                    .iter()
                    .find(|model| model_slug(model) == Some("kimi-k3"))
                    .cloned()
            })
            .expect("model");
        assert_eq!(model["display_name"], "我的 Kimi");
        assert_eq!(model["context_window"], 128_000);
        assert_eq!(model["input_modalities"], json!(["text"]));
    }

    #[test]
    fn official_overlay_retires_models_removed_upstream() {
        // 内置目录里有 gpt-5.6-sol（官方）和 deepseek-v4.1-flash（项目自建第三方）。
        let merged = merge_catalog_with_official(
            &embedded_models(),
            &[json!({ "slug": "gpt-5.5", "use_responses_lite": false })],
            &["gpt-5.5".to_string(), "gpt-5.6-sol".to_string()],
        );
        let slugs: Vec<&str> = merged
            .iter()
            .filter_map(|model| model_slug(model))
            .collect();

        // 官方已下线的 gpt-5.6-sol 被移除。
        assert!(
            !slugs.contains(&"gpt-5.6-sol"),
            "retired official model should be dropped: {slugs:?}"
        );
        // 官方仍提供的条目保留。
        assert!(slugs.contains(&"gpt-5.5"));
        // 项目自建的第三方条目不受影响。
        assert!(slugs.contains(&"deepseek-v4.1-flash"));
        assert!(slugs.contains(&"glm-5.3-flash"));

        // 空覆盖层 = 内置目录原样。
        let restored = merge_catalog_with_official(&embedded_models(), &[], &[]);
        assert!(
            restored
                .iter()
                .any(|model| model_slug(model) == Some("gpt-5.6-sol"))
        );
    }

    #[test]
    fn official_overlay_keeps_local_base_instructions() {
        let merged = merge_catalog_with_official(
            &embedded_models(),
            &[json!({ "slug": "gpt-5.6-sol", "use_responses_lite": true })],
            &["gpt-5.6-sol".to_string()],
        );
        let model = merged
            .iter()
            .find(|model| model_slug(model) == Some("gpt-5.6-sol"))
            .expect("gpt-5.6-sol");
        // 官方字段生效。
        assert_eq!(model["use_responses_lite"], true);
        // 内置的 base_instructions 被保留（官方目录不含它，Codex 会读）。
        assert!(
            model
                .get("base_instructions")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
        );
    }

    #[test]
    fn catalog_options_report_their_source() {
        let mut config = config(&[]);
        config.providers = vec![ProviderConfig {
            name: "autoclaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec!["vendor-proxy-model".to_string()],
            ..Default::default()
        }];
        config.custom_models = vec![CustomModelConfig {
            slug: "vendor-new-model".to_string(),
            ..Default::default()
        }];

        let custom = custom_catalog_model_options(&config);
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0].source, CatalogModelSource::Custom);
        assert_eq!(custom[0].slug, "vendor-new-model");

        let auto = auto_catalog_model_options(&config);
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].source, CatalogModelSource::Auto);
        assert_eq!(auto[0].slug, "vendor-proxy-model");

        assert!(
            visible_catalog_model_options()
                .iter()
                .all(|option| option.source == CatalogModelSource::Builtin)
        );
    }

    #[test]
    fn configured_models_response_uses_codex_visible_models() {
        // 白名单里混入两类不该出现的名字：
        // - 已从内置目录删除、且没有服务商声明的模型（grok-4.6 等）；
        // - 完全未知的名字（custom-model）。
        let config = config(&[
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-6-astra",
            "deepseek-v4.1-flash",
            "glm-5.3-flash",
            "grok-4.6",
            "custom-model",
        ]);

        let response = configured_models_response(&config);
        let slugs: Vec<&str> = response["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect();

        // 顺序与白名单一致，跳过了没有目录条目也没有服务商声明的名字。
        assert_eq!(
            slugs,
            vec![
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-6-astra",
                "deepseek-v4.1-flash",
                "glm-5.3-flash",
            ]
        );
        // 已删除的条目在无服务商声明时不会凭空出现。
        assert!(!slugs.contains(&"grok-4.6"));
        assert!(!slugs.contains(&"custom-model"));

        // 目录条目的能力与协议字段照常提供。
        let deepseek = response["models"][5].clone();
        assert_eq!(deepseek["display_name"], "DeepSeek-V4.1-Flash");
        assert_eq!(deepseek["comp_hash"], "3000");
        assert_eq!(deepseek["apply_patch_tool_type"], "freeform");
        assert_eq!(deepseek["supports_search_tool"], true);
        assert_eq!(deepseek["supports_image_detail_original"], true);
        assert_eq!(deepseek["input_modalities"], json!(["text", "image"]));
    }

    #[test]
    fn configured_models_etag_is_stable_for_same_response() {
        let config = config(&["deepseek-v4-pro", "deepseek-v4-flash"]);

        let (response, etag) = configured_models_response_with_etag(&config);

        assert_eq!(response, configured_models_response(&config));
        assert_eq!(etag, configured_models_etag(&config));
        assert!(etag.starts_with("\"sha256:"));
        assert!(etag.ends_with('"'));
    }

    #[test]
    fn configured_models_etag_changes_when_visible_models_change() {
        let base_config = config(&["gpt-5.5"]);
        let changed_config = config(&["gpt-5.5", "gpt-5.6-sol"]);

        assert_ne!(
            configured_models_etag(&base_config),
            configured_models_etag(&changed_config)
        );
    }

    #[test]
    fn configured_models_response_skips_unknown_configured_model() {
        let config = config(&["custom-model"]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn configured_models_response_skips_hidden_catalog_model() {
        let config = config(&["codex-auto-review"]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn deepseek_models_preserve_apply_patch_tool_from_catalog() {
        // deepseek-v4.1-flash 是仍在内置目录里的条目，协议字段应原样保留。
        let response = configured_models_response(&config(&["deepseek-v4.1-flash"]));
        let model = &response["models"][0];
        assert_eq!(model["apply_patch_tool_type"], "freeform");
        assert_eq!(model["supports_search_tool"], true);
        // 反代实测可识别双色图，视觉能力保留。
        assert_eq!(model["supports_image_detail_original"], true);
        assert_eq!(model["input_modalities"], json!(["text", "image"]));
    }

    #[test]
    fn deleted_catalog_entries_are_synthesized_from_templates_instead() {
        // deepseek-v4-pro / grok-4.6 / GLM-5.2 等条目已从内置目录删除：
        // 它们的协议字段现在来自 family_templates.json，因此只要服务商声明了
        // 模型名，仍然能合成出带正确协议字段的目录条目。
        let mut config = config(&["deepseek-v4-pro", "grok-4.6", "GLM-5.2"]);
        config.providers = vec![ProviderConfig {
            name: "AutoClaw".to_string(),
            enabled: true,
            provider_type: ProviderType::ChatCompletions,
            base_url: "http://127.0.0.1:18800/v1".to_string(),
            models: vec![
                "deepseek-v4-pro".to_string(),
                "grok-4.6".to_string(),
                "GLM-5.2".to_string(),
            ],
            ..Default::default()
        }];

        let response = configured_models_response(&config);
        let models = response["models"].as_array().unwrap();
        assert_eq!(models.len(), 3, "all three should still be synthesized");

        let by_slug = |slug: &str| {
            models
                .iter()
                .find(|model| model_slug(model) == Some(slug))
                .unwrap_or_else(|| panic!("{slug} should exist"))
        };
        // 协议字段来自**该服务商协议**对应的家族模板：AutoClaw 是
        // chat_completions，所以三者都拿到 chat 家族的 comp_hash。
        assert_eq!(by_slug("deepseek-v4-pro")["comp_hash"], "3000");
        assert_eq!(by_slug("grok-4.6")["comp_hash"], "3000");
        assert_eq!(by_slug("GLM-5.2")["comp_hash"], "3000");
        // 能力字段来自内置模型库。
        assert_eq!(
            by_slug("deepseek-v4-pro")["display_name"],
            "DeepSeek V4 Pro"
        );
        assert_eq!(by_slug("deepseek-v4-pro")["context_window"], 1_000_000);
    }

    #[test]
    fn configured_models_response_returns_empty_when_no_models_configured() {
        let config = config(&[]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn catalog_model_visibility_requires_api_support_and_list_visibility() {
        assert!(is_catalog_model_visible(&json!({
            "supported_in_api": true,
            "visibility": "list"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "supported_in_api": false,
            "visibility": "list"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "supported_in_api": true,
            "visibility": "hide"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "visibility": "list"
        })));
    }

    #[test]
    fn all_visible_catalog_models_declare_comp_hash() {
        let missing = catalog_models()
            .into_iter()
            .filter(is_catalog_model_visible)
            .filter_map(|model| {
                let comp_hash = model
                    .get("comp_hash")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                comp_hash
                    .is_empty()
                    .then(|| model_slug(&model).unwrap_or("<missing-slug>").to_string())
            })
            .collect::<Vec<_>>();

        assert!(missing.is_empty(), "models missing comp_hash: {missing:?}");
    }

    #[test]
    fn comp_hashes_follow_protocol_families() {
        // comp_hash 现在来自协议模板表，不再依赖内置目录里存在某个具体模型。
        let field =
            |family: ModelFamily, key: &str| family_template(family).fields.get(key).cloned();
        assert_eq!(
            field(ModelFamily::GrokResponses, "comp_hash"),
            Some(json!("codexhub-grok-summary-v1"))
        );
        assert_eq!(
            field(ModelFamily::DeepSeekResponses, "comp_hash"),
            Some(json!("3000"))
        );
        // AutoClaw 反代走 chat_completions，压缩摘要家族与 OpenAI Chat 一致。
        assert_eq!(
            field(ModelFamily::ChatCompletions, "comp_hash"),
            Some(json!("3000"))
        );
        assert_eq!(
            field(ModelFamily::AnthropicMessages, "comp_hash"),
            Some(json!("codexhub-anthropic-summary-v1"))
        );
        assert_eq!(
            field(ModelFamily::OpenAiResponses, "comp_hash"),
            Some(json!("2911"))
        );

        assert_ne!(
            field(ModelFamily::GrokResponses, "comp_hash"),
            field(ModelFamily::OpenAiResponses, "comp_hash")
        );
        assert_ne!(
            field(ModelFamily::AnthropicMessages, "comp_hash"),
            field(ModelFamily::DeepSeekResponses, "comp_hash")
        );
    }

    #[test]
    fn every_family_template_carries_the_required_protocol_fields() {
        for family in [
            ModelFamily::OpenAiResponses,
            ModelFamily::DeepSeekResponses,
            ModelFamily::GrokResponses,
            ModelFamily::ChatCompletions,
            ModelFamily::AnthropicMessages,
        ] {
            let template = family_template(family);
            for key in [
                "comp_hash",
                "shell_type",
                "apply_patch_tool_type",
                "base_instructions",
            ] {
                assert!(
                    template
                        .fields
                        .get(key)
                        .is_some_and(|value| !value.is_null()),
                    "{family:?} 模板缺少 {key}"
                );
            }
        }
    }

    #[test]
    fn family_templates_survive_removal_of_their_catalog_entries() {
        // 模板条目已从内置目录删除，但模板本身仍必须完整可用。
        let slugs: Vec<String> = catalog_models()
            .iter()
            .filter_map(|model| model_slug(model).map(str::to_string))
            .collect();
        for slug in ["grok-4.6", "deepseek-v4-pro", "GLM-5.2"] {
            assert!(
                !slugs.iter().any(|existing| existing == slug),
                "{slug} 不应再出现在内置目录里"
            );
        }
        assert_eq!(
            family_template(ModelFamily::GrokResponses)
                .fields
                .get("comp_hash"),
            Some(&json!("codexhub-grok-summary-v1"))
        );
    }

    #[test]
    fn family_templates_keep_their_conservative_context_fallbacks() {
        // 库和上游都没给上下文时，各家族回落到各自的保守值。
        for (family, expected) in [
            (ModelFamily::OpenAiResponses, 272_000),
            (ModelFamily::DeepSeekResponses, 372_000),
            (ModelFamily::GrokResponses, 372_000),
            (ModelFamily::ChatCompletions, 128_000),
            (ModelFamily::AnthropicMessages, 372_000),
        ] {
            assert_eq!(
                family_template(family).context_window,
                expected,
                "{family:?}"
            );
        }
    }

    #[test]
    fn deepseek_provider_declared_models_use_library_capabilities() {
        // deepseek-v4-pro / deepseek-v4-flash 已从内置目录删除；服务商声明后
        // 应由协议模板（DeepSeek Responses）+ 内置模型库合成。
        let mut config = config(&["deepseek-v4-pro", "deepseek-v4-flash"]);
        config.providers = vec![ProviderConfig {
            name: "DeepSeek".to_string(),
            enabled: true,
            provider_type: ProviderType::DeepSeekResponses,
            base_url: "https://api.deepseek.com".to_string(),
            models: vec![
                "deepseek-v4-pro".to_string(),
                "deepseek-v4-flash".to_string(),
            ],
            ..Default::default()
        }];

        let response = configured_models_response(&config);
        let models = response["models"].as_array().unwrap();
        let by_slug = |slug: &str| {
            models
                .iter()
                .find(|model| model_slug(model) == Some(slug))
                .unwrap_or_else(|| panic!("{slug} should be synthesized"))
        };

        // 协议字段来自 DeepSeek Responses 家族模板。
        for slug in ["deepseek-v4-pro", "deepseek-v4-flash"] {
            assert_eq!(by_slug(slug)["comp_hash"], "3000", "model {slug}");
            assert_eq!(
                by_slug(slug)["apply_patch_tool_type"],
                "freeform",
                "model {slug}"
            );
            assert_eq!(by_slug(slug)["supports_search_tool"], true, "model {slug}");
        }
        // 能力字段来自内置模型库。
        assert_eq!(
            by_slug("deepseek-v4-pro")["display_name"],
            "DeepSeek V4 Pro"
        );
        assert_eq!(by_slug("deepseek-v4-pro")["context_window"], 1_000_000);
        assert_eq!(
            by_slug("deepseek-v4-flash")["display_name"],
            "DeepSeek V4 Flash"
        );
        assert_eq!(by_slug("deepseek-v4-flash")["context_window"], 1_000_000);
    }

    #[test]
    fn autoclaw_deepseek_v41_flash_keeps_its_proxy_route_capabilities() {
        let model = catalog_models()
            .into_iter()
            .find(|model| model_slug(model) == Some("deepseek-v4.1-flash"))
            .expect("AutoClaw DeepSeek catalog model should exist");

        assert_eq!(model["display_name"], "DeepSeek-V4.1-Flash");
        assert_eq!(
            model["description"],
            "DeepSeek V4.1 Flash served through the local AutoClaw reverse proxy."
        );
        assert_eq!(model["visibility"], "list");
        assert_eq!(model["supported_in_api"], true);
        // AutoClaw 客户端对该路由声明 1M 上下文。
        assert_eq!(model["context_window"], 1_048_576);
        assert_eq!(model["max_context_window"], 1_048_576);
        assert_eq!(model["effective_context_window_percent"], 95);
        // 反代双色图实测可识别，保留视觉能力。
        assert_eq!(model["supports_image_detail_original"], true);
        assert_eq!(model["input_modalities"], json!(["text", "image"]));
        assert_eq!(model["use_responses_lite"], false);
        assert_eq!(model["default_reasoning_level"], "high");

        // 归一化不得把 v4.1-flash 降级成纯文本。
        let response = configured_models_response(&config(&["deepseek-v4.1-flash"]));
        assert_eq!(response["models"][0]["slug"], "deepseek-v4.1-flash");
        assert_eq!(
            response["models"][0]["input_modalities"],
            json!(["text", "image"])
        );
        assert_eq!(response["models"][0]["web_search_tool_type"], "text");
    }

    #[test]
    fn autoclaw_glm_53_flash_keeps_its_proxy_route_capabilities() {
        let model = catalog_models()
            .into_iter()
            .find(|model| model_slug(model) == Some("glm-5.3-flash"))
            .expect("AutoClaw GLM catalog model should exist");

        assert_eq!(model["display_name"], "GLM-5.3-Flash");
        assert_eq!(
            model["description"],
            "Fast frontier model served through the local AutoClaw reverse proxy."
        );
        assert_eq!(model["visibility"], "list");
        assert_eq!(model["supported_in_api"], true);
        assert_eq!(model["prefer_websockets"], false);
        assert_eq!(model["context_window"], 1_048_576);
        assert_eq!(model["max_context_window"], 1_048_576);
        // 反代双色图实测可识别。
        assert_eq!(model["input_modalities"], json!(["text", "image"]));
        assert_eq!(model["supports_image_detail_original"], true);
        assert_eq!(model["comp_hash"], "3000");

        // GLM 不在 deepseek 归一化范围内，能力应原样透出。
        let response = configured_models_response(&config(&["glm-5.3-flash"]));
        assert_eq!(response["models"][0]["slug"], "glm-5.3-flash");
        assert_eq!(
            response["models"][0]["input_modalities"],
            json!(["text", "image"])
        );
        assert_eq!(response["models"][0]["comp_hash"], "3000");
    }

    #[test]
    fn gpt_lite_models_use_current_official_capabilities() {
        for (slug, priority) in [
            ("gpt-6-astra", 1),
            ("gpt-5.6-sol", 6),
            ("gpt-5.6-terra", 7),
            ("gpt-5.6-luna", 8),
        ] {
            let model = catalog_models()
                .into_iter()
                .find(|model| model_slug(model) == Some(slug))
                .expect("catalog model should exist");

            assert_eq!(model["context_window"], 272_000, "model {slug}");
            assert_eq!(model["max_context_window"], 872_000, "model {slug}");
            assert_eq!(model["use_responses_lite"], true, "model {slug}");
            assert_eq!(
                model["supports_reasoning_summary_parameter"], true,
                "model {slug}"
            );
            assert_eq!(model["visibility"], "list", "model {slug}");
            assert_eq!(model["priority"], priority, "model {slug}");
        }
    }

    #[test]
    fn gpt_5_5_remains_visible_and_older_models_are_removed() {
        let gpt_5_5 = catalog_models()
            .into_iter()
            .find(|model| model_slug(model) == Some("gpt-5.5"))
            .expect("gpt-5.5 should exist");
        assert_eq!(gpt_5_5["visibility"], "list");
        assert_eq!(gpt_5_5["supports_reasoning_summary_parameter"], true);
        assert_eq!(gpt_5_5.get("availability_nux"), Some(&Value::Null));

        for slug in ["gpt-5.4", "gpt-5.4-mini"] {
            assert!(
                !catalog_models()
                    .into_iter()
                    .any(|model| model_slug(&model) == Some(slug))
            );
        }
    }

    #[test]
    fn visible_catalog_model_options_returns_listable_api_models() {
        let options = visible_catalog_model_options();
        let slugs = options
            .iter()
            .map(|model| model.slug.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "deepseek-v4.1-flash",
            "glm-5.3-flash",
        ] {
            assert!(
                slugs.contains(&expected),
                "missing visible model {expected}"
            );
        }
        // 已从内置目录删除的条目不再出现在可见列表里。
        assert!(!slugs.contains(&"grok-4.6"));
        assert!(!slugs.contains(&"deepseek-v4-pro"));
        assert!(!slugs.contains(&"GLM-5.2"));
        assert!(!slugs.contains(&"gpt-5.2"));
        assert!(
            options
                .iter()
                .all(|model| !model.slug.trim().is_empty() && !model.display_name.trim().is_empty())
        );
    }
}
