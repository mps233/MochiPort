use std::{fs, path::PathBuf, time::SystemTime};

use axum::{
    Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app_state::SharedState;
use crate::codex_app_config;

const DEFAULT_FEATURED_PLUGINS: &[&str] = &[
    "github",
    "linear",
    "gmail",
    "slack",
    "figma",
    "google-calendar",
    "google-drive",
    "canva",
];
const OPENAI_BUNDLED_MARKETPLACE: &str = "openai-bundled";
const OPENAI_CURATED_REMOTE_MARKETPLACE: &str = "openai-curated-remote";
const CODEXHUB_CURATED_REMOTE_ID_PREFIX: &str = "plugins~codexhub-local-";
const CODEXHUB_BUNDLED_REMOTE_ID_PREFIX: &str = "plugins~codexhub-bundled-";
const LOCAL_BUNDLED_REMOTE_ID_PREFIX: &str = "local~openai-bundled~";

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/backend-api/ps/plugins/list", get(list_plugins))
        .route("/backend-api/ps/plugins/installed", get(installed_plugins))
        .route("/backend-api/ps/plugins/suggested", get(suggested_plugins))
        .route(
            "/backend-api/ps/plugins/{plugin_id}/install",
            post(install_plugin),
        )
        .route(
            "/backend-api/ps/plugins/{plugin_id}/skills/{skill_name}",
            get(plugin_skill_detail),
        )
        .route("/backend-api/ps/plugins/{plugin_id}", get(plugin_detail))
        .route("/backend-api/plugins/featured", get(featured_plugins))
}

#[derive(Debug, Deserialize)]
struct PluginListQuery {
    scope: Option<String>,
    #[serde(rename = "pageToken")]
    page_token: Option<String>,
}

async fn list_plugins(Query(query): Query<PluginListQuery>) -> Response {
    if !matches!(query.scope.as_deref(), None | Some("GLOBAL")) {
        return Json(empty_plugin_page()).into_response();
    }
    if query.page_token.is_some() {
        return Json(empty_plugin_page()).into_response();
    }

    match load_local_curated_remote_plugins() {
        Ok(plugins) => Json(json!({
            "plugins": plugins,
            "pagination": {
                "next_page_token": null
            }
        }))
        .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": err
            })),
        )
            .into_response(),
    }
}

async fn installed_plugins() -> Json<Value> {
    let plugins = load_installed_remote_plugins();
    Json(json!({
        "plugins": plugins,
        "pagination": {
            "next_page_token": null
        }
    }))
}

async fn featured_plugins() -> Json<Value> {
    Json(json!(
        DEFAULT_FEATURED_PLUGINS
            .iter()
            .map(|name| format!("{name}@openai-curated-remote"))
            .collect::<Vec<_>>()
    ))
}

async fn suggested_plugins() -> Response {
    match load_local_curated_remote_plugins() {
        Ok(plugins) => Json(json!({
            "enabled": true,
            "plugins": recommended_plugins(&plugins),
        }))
        .into_response(),
        Err(_) => Json(json!({
            "enabled": false,
            "plugins": [],
        }))
        .into_response(),
    }
}

async fn plugin_detail(Path(plugin_id): Path<String>) -> Response {
    match find_local_fallback_plugin_detail(&plugin_id) {
        Ok(Some(plugin)) => Json(plugin).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("plugin {plugin_id} not found")
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": err
            })),
        )
            .into_response(),
    }
}

async fn install_plugin(Path(plugin_id): Path<String>) -> Response {
    match find_local_fallback_plugin_detail(&plugin_id) {
        Ok(Some(_)) => Json(json!({
            "id": plugin_id,
            "enabled": true,
            "app_ids_needing_auth": [],
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("plugin {plugin_id} not found")
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": err
            })),
        )
            .into_response(),
    }
}

async fn plugin_skill_detail(Path((plugin_id, skill_name)): Path<(String, String)>) -> Response {
    match find_local_fallback_plugin_skill(&plugin_id, &skill_name) {
        Ok(Some(contents)) => Json(json!({
            "plugin_id": plugin_id,
            "name": skill_name,
            "skill_md_contents": contents,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("plugin skill {plugin_id}/{skill_name} not found")
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": err
            })),
        )
            .into_response(),
    }
}

fn empty_plugin_page() -> Value {
    json!({
        "plugins": [],
        "pagination": {
            "next_page_token": null
        }
    })
}

fn load_local_curated_remote_plugins() -> Result<Vec<Value>, String> {
    let path = curated_marketplace_path();
    let contents = std::fs::read_to_string(&path).map_err(|err| {
        format!(
            "failed to read local curated marketplace {}: {err}",
            path.display()
        )
    })?;
    let manifest: Value = serde_json::from_str(&contents).map_err(|err| {
        format!(
            "failed to parse local curated marketplace {}: {err}",
            path.display()
        )
    })?;
    let plugins = manifest
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "local curated marketplace {} does not contain plugins array",
                path.display()
            )
        })?;

    Ok(plugins
        .iter()
        .filter_map(|plugin| {
            local_marketplace_plugin_to_remote(
                plugin,
                OPENAI_CURATED_REMOTE_MARKETPLACE,
                CODEXHUB_CURATED_REMOTE_ID_PREFIX,
            )
        })
        .collect())
}

fn find_local_fallback_plugin_detail(plugin_id: &str) -> Result<Option<Value>, String> {
    if let Some(plugin) = find_local_bundled_compat_plugin(plugin_id)? {
        return Ok(Some(remote_directory_item_to_detail(plugin)));
    }
    Ok(find_local_curated_remote_plugin(plugin_id)?.map(remote_directory_item_to_detail))
}

fn find_local_fallback_plugin_skill(
    plugin_id: &str,
    skill_name: &str,
) -> Result<Option<String>, String> {
    let Some(plugin_name) = bundled_plugin_name_from_compat_id(plugin_id) else {
        return Ok(None);
    };

    read_local_bundled_skill(&plugin_name, skill_name)
}

fn load_installed_remote_plugins() -> Vec<Value> {
    installed_plugin_config_ids()
        .into_iter()
        .filter_map(|plugin_id| {
            let plugin = if plugin_id.ends_with("@openai-curated")
                || plugin_id.ends_with(&format!("@{OPENAI_CURATED_REMOTE_MARKETPLACE}"))
            {
                let plugin_name = plugin_id.split('@').next().unwrap_or_default();
                find_local_curated_remote_plugin(plugin_name).ok().flatten()
            } else {
                None
            }?;

            Some(installed_plugin_item(plugin))
        })
        .collect()
}

fn installed_plugin_item(mut plugin: Value) -> Value {
    if let Value::Object(map) = &mut plugin {
        map.insert("enabled".to_string(), Value::Bool(true));
        map.insert("disabled_skill_names".to_string(), Value::Array(Vec::new()));
    }
    plugin
}

fn installed_plugin_config_ids() -> Vec<String> {
    let config_path = codex_home().join("config.toml");
    let Ok(contents) = fs::read_to_string(config_path) else {
        return Vec::new();
    };
    let Ok(doc) = contents.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    let Some(plugins) = doc.get("plugins").and_then(|item| item.as_table()) else {
        return Vec::new();
    };

    plugins
        .iter()
        .filter_map(|(id, item)| {
            item.as_table()
                .and_then(|table| table.get("enabled"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                .then(|| id.to_string())
        })
        .collect()
}

fn find_local_curated_remote_plugin(plugin_id: &str) -> Result<Option<Value>, String> {
    Ok(load_local_curated_remote_plugins()?
        .into_iter()
        .find(|plugin| plugin_matches_id(plugin, plugin_id, OPENAI_CURATED_REMOTE_MARKETPLACE)))
}

fn find_local_bundled_compat_plugin(plugin_id: &str) -> Result<Option<Value>, String> {
    let Some(plugin_name) = bundled_plugin_name_from_compat_id(plugin_id) else {
        return Ok(None);
    };
    let path = bundled_marketplace_path();
    let contents = fs::read_to_string(&path).map_err(|err| {
        format!(
            "failed to read local bundled marketplace {}: {err}",
            path.display()
        )
    })?;
    let manifest: Value = serde_json::from_str(&contents).map_err(|err| {
        format!(
            "failed to parse local bundled marketplace {}: {err}",
            path.display()
        )
    })?;
    let plugins = manifest
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "local bundled marketplace {} does not contain plugins array",
                path.display()
            )
        })?;

    Ok(plugins
        .iter()
        .find(|plugin| plugin.get("name").and_then(Value::as_str) == Some(plugin_name.as_str()))
        .and_then(|plugin| {
            let remote_id = bundled_compat_remote_id(plugin_id, &plugin_name);
            let plugin = local_marketplace_plugin_to_remote_with_id(
                plugin,
                OPENAI_BUNDLED_MARKETPLACE,
                Some(remote_id.as_str()),
                CODEXHUB_BUNDLED_REMOTE_ID_PREFIX,
            )?;
            Some(with_local_bundled_skills(plugin, &plugin_name))
        }))
}

fn bundled_plugin_name_from_compat_id(plugin_id: &str) -> Option<String> {
    plugin_id
        .strip_prefix(CODEXHUB_BUNDLED_REMOTE_ID_PREFIX)
        .map(str::to_string)
        .or_else(|| {
            plugin_id
                .strip_prefix(LOCAL_BUNDLED_REMOTE_ID_PREFIX)
                .map(str::to_string)
        })
        .or_else(|| {
            plugin_id
                .strip_suffix(&format!("@{OPENAI_BUNDLED_MARKETPLACE}"))
                .map(str::to_string)
        })
        .filter(|name| !name.is_empty())
}

fn bundled_compat_remote_id(plugin_id: &str, plugin_name: &str) -> String {
    if plugin_id.starts_with(CODEXHUB_BUNDLED_REMOTE_ID_PREFIX)
        || plugin_id.starts_with(LOCAL_BUNDLED_REMOTE_ID_PREFIX)
    {
        return plugin_id.to_string();
    }
    format!("{CODEXHUB_BUNDLED_REMOTE_ID_PREFIX}{plugin_name}")
}

fn read_local_bundled_skill(plugin_name: &str, skill_name: &str) -> Result<Option<String>, String> {
    if !is_safe_path_segment(plugin_name) || !is_safe_path_segment(skill_name) {
        return Ok(None);
    }

    for root in local_bundled_skill_root_candidate_paths(plugin_name)? {
        let path = root.join(skill_name).join("SKILL.md");
        if path.is_file() {
            return fs::read_to_string(&path).map(Some).map_err(|err| {
                format!(
                    "failed to read bundled plugin skill {}: {err}",
                    path.display()
                )
            });
        }
    }

    Ok(None)
}

fn local_bundled_skill_root_candidate_paths(plugin_name: &str) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let cache_root = codex_home()
        .join("plugins")
        .join("cache")
        .join(OPENAI_BUNDLED_MARKETPLACE)
        .join(plugin_name);

    if cache_root.is_dir() {
        let mut cached_paths = Vec::new();
        for entry in fs::read_dir(&cache_root).map_err(|err| {
            format!(
                "failed to read bundled plugin cache {}: {err}",
                cache_root.display()
            )
        })? {
            let entry = entry.map_err(|err| {
                format!(
                    "failed to read bundled plugin cache entry {}: {err}",
                    cache_root.display()
                )
            })?;
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }

            let skills_root = entry.path().join("skills");
            if !skills_root.is_dir() {
                continue;
            }

            let modified = skills_root
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            cached_paths.push((modified, skills_root));
        }
        cached_paths.sort_by(|left, right| right.0.cmp(&left.0));
        paths.extend(cached_paths.into_iter().map(|(_, path)| path));
    }

    paths.push(
        codex_home()
            .join(".tmp")
            .join("bundled-marketplaces")
            .join(OPENAI_BUNDLED_MARKETPLACE)
            .join("plugins")
            .join(plugin_name)
            .join("skills"),
    );

    Ok(paths)
}

fn with_local_bundled_skills(mut plugin: Value, plugin_name: &str) -> Value {
    let skills = local_bundled_skill_summaries(plugin_name);
    if skills.is_empty() {
        return plugin;
    }

    if let Some(release) = plugin.get_mut("release").and_then(Value::as_object_mut) {
        release.insert("skills".to_string(), Value::Array(skills));
    }

    plugin
}

fn local_bundled_skill_summaries(plugin_name: &str) -> Vec<Value> {
    if !is_safe_path_segment(plugin_name) {
        return Vec::new();
    }

    let Ok(roots) = local_bundled_skill_root_candidate_paths(plugin_name) else {
        return Vec::new();
    };

    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };

        let mut skills = entries
            .flatten()
            .filter_map(|entry| {
                if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    return None;
                }

                let fallback_name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path().join("SKILL.md");
                let contents = fs::read_to_string(path).ok()?;
                let name = front_matter_value(&contents, "name").unwrap_or(fallback_name);
                let description =
                    front_matter_value(&contents, "description").unwrap_or_else(|| name.clone());
                let short_description = front_matter_value(&contents, "short_description")
                    .or_else(|| Some(description.clone()));

                Some(json!({
                    "name": name,
                    "description": description,
                    "interface": {
                        "display_name": name,
                        "short_description": short_description,
                        "brand_color": null,
                        "default_prompt": null,
                        "icon_small_url": null,
                        "icon_large_url": null
                    }
                }))
            })
            .collect::<Vec<_>>();

        if !skills.is_empty() {
            skills.sort_by(|left, right| {
                let left = left.get("name").and_then(Value::as_str).unwrap_or_default();
                let right = right
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                left.cmp(right)
            });
            return skills;
        }
    }

    Vec::new()
}

fn front_matter_value(contents: &str, key: &str) -> Option<String> {
    let mut lines = contents.lines();
    if lines.next()? != "---" {
        return None;
    }

    for line in lines {
        if line == "---" {
            break;
        }

        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        if left.trim() == key {
            return Some(
                right
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            )
            .filter(|value| !value.is_empty());
        }
    }

    None
}

fn is_safe_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn recommended_plugins(plugins: &[Value]) -> Vec<Value> {
    DEFAULT_FEATURED_PLUGINS
        .iter()
        .filter_map(|name| {
            plugins
                .iter()
                .find(|plugin| plugin.get("name").and_then(Value::as_str) == Some(*name))
        })
        .filter_map(recommended_plugin_item)
        .collect()
}

fn recommended_plugin_item(plugin: &Value) -> Option<Value> {
    let id = plugin.get("id")?.as_str()?;
    let name = plugin.get("name")?.as_str()?;
    let display_name = plugin
        .get("release")
        .and_then(|release| release.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or(name);

    Some(json!({
        "id": id,
        "name": name,
        "status": "ENABLED",
        "installation_policy": "AVAILABLE",
        "release": {
            "display_name": display_name,
            "app_ids": [],
        },
    }))
}

fn local_marketplace_plugin_to_remote(
    plugin: &Value,
    marketplace_name: &str,
    id_prefix: &str,
) -> Option<Value> {
    local_marketplace_plugin_to_remote_with_id(plugin, marketplace_name, None, id_prefix)
}

fn local_marketplace_plugin_to_remote_with_id(
    plugin: &Value,
    marketplace_name: &str,
    remote_id: Option<&str>,
    id_prefix: &str,
) -> Option<Value> {
    let name = plugin.get("name")?.as_str()?;
    let remote_id = remote_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("{id_prefix}{name}"));
    let interface = plugin.get("interface");
    let display_name = interface
        .and_then(|item| item.get("displayName").or_else(|| item.get("display_name")))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| display_name_from_slug(name));
    let short_description = interface
        .and_then(|item| {
            item.get("shortDescription")
                .or_else(|| item.get("short_description"))
        })
        .and_then(Value::as_str)
        .unwrap_or("Use this plugin with Codex");
    let category = plugin.get("category").and_then(Value::as_str).or_else(|| {
        interface
            .and_then(|item| item.get("category"))
            .and_then(Value::as_str)
    });
    let long_description = interface_string(interface, &["longDescription", "long_description"]);
    let developer_name = interface_string(interface, &["developerName", "developer_name"]);
    let website_url = interface_string(interface, &["websiteURL", "websiteUrl", "website_url"]);
    let privacy_policy_url = interface_string(
        interface,
        &["privacyPolicyURL", "privacyPolicyUrl", "privacy_policy_url"],
    );
    let terms_of_service_url = interface_string(
        interface,
        &[
            "termsOfServiceURL",
            "termsOfServiceUrl",
            "terms_of_service_url",
        ],
    );
    let brand_color = interface_string(interface, &["brandColor", "brand_color"]);
    let composer_icon_url = interface_string(
        interface,
        &["composerIconURL", "composerIconUrl", "composer_icon_url"],
    );
    let logo_url = interface_string(interface, &["logoURL", "logoUrl", "logo_url"]);
    let logo_url_dark =
        interface_string(interface, &["logoURLDark", "logoUrlDark", "logo_url_dark"]);

    Some(json!({
        "id": remote_id,
        "name": name,
        "scope": "GLOBAL",
        "installation_policy": "AVAILABLE",
        "authentication_policy": "ON_USE",
        "status": "ENABLED",
        "release": {
            "version": "local",
            "display_name": display_name,
            "description": short_description,
            "app_ids": [],
            "keywords": [],
            "interface": {
                "short_description": short_description,
                "long_description": long_description,
                "developer_name": developer_name,
                "category": category,
                "capabilities": string_array(interface, "capabilities"),
                "website_url": website_url,
                "privacy_policy_url": privacy_policy_url,
                "terms_of_service_url": terms_of_service_url,
                "brand_color": brand_color,
                "default_prompt": interface_string(interface, &["defaultPrompt", "default_prompt"]),
                "default_prompts": string_array_opt(interface, &["defaultPrompts", "default_prompts"]),
                "composer_icon_url": composer_icon_url,
                "logo_url": logo_url,
                "logo_url_dark": logo_url_dark,
                "screenshot_urls": string_array_any(interface, &["screenshotUrls", "screenshot_urls"])
            },
            "skills": []
        },
        "codexhub_marketplace_name": marketplace_name
    }))
}

fn interface_string(interface: Option<&Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        interface?
            .get(*key)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn string_array(interface: Option<&Value>, key: &str) -> Vec<String> {
    string_array_any(interface, &[key])
}

fn string_array_opt(interface: Option<&Value>, keys: &[&str]) -> Option<Vec<String>> {
    let values = string_array_any(interface, keys);
    (!values.is_empty()).then_some(values)
}

fn string_array_any(interface: Option<&Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| {
            interface?.get(*key).and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default()
}

fn remote_directory_item_to_detail(plugin: Value) -> Value {
    plugin
}

fn plugin_matches_id(plugin: &Value, plugin_id: &str, marketplace_name: &str) -> bool {
    let id = plugin.get("id").and_then(Value::as_str);
    let name = plugin.get("name").and_then(Value::as_str);
    id == Some(plugin_id)
        || name == Some(plugin_id)
        || name
            .map(|name| format!("{name}@{marketplace_name}"))
            .as_deref()
            == Some(plugin_id)
}

fn curated_marketplace_path() -> PathBuf {
    codex_home()
        .join(".tmp")
        .join("plugins")
        .join(".agents")
        .join("plugins")
        .join("marketplace.json")
}

fn bundled_marketplace_path() -> PathBuf {
    codex_home()
        .join(".tmp")
        .join("bundled-marketplaces")
        .join(OPENAI_BUNDLED_MARKETPLACE)
        .join(".agents")
        .join("plugins")
        .join("marketplace.json")
}

fn display_name_from_slug(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn codex_home() -> PathBuf {
    codex_app_config::default_codex_home()
}
