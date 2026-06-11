//! Unit tests for model registry: loading, filtering, and scoped selection.
//!
//! Tests cover:
//! - Built-in model loading from `AuthStorage`
//! - Custom models.json parsing and application
//! - Model filtering by availability (API key presence)
//! - Model lookup by provider/id
//! - Error handling for malformed JSON
//! - Environment variable resolution
//! - Header merging and resolution

mod common;

use common::harness::TestHarness;
use pi::auth::AuthStorage;
use pi::models::{ModelRegistry, default_models_path};

// ============================================================================
// Built-in Models Tests
// ============================================================================

#[test]
fn test_built_in_models_without_api_keys() {
    let harness = TestHarness::new("test_built_in_models_without_api_keys");
    harness.section("Setup");

    // Create empty auth storage (no API keys)
    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, None);

    harness.section("Verify");
    harness
        .log()
        .info_ctx("verify", "Checking built-in models", |ctx| {
            ctx.push(("model_count".into(), registry.models().len().to_string()));
        });

    // Built-in catalog is intentionally limited to official DeepSeek API models.
    let models = registry.models();
    assert_eq!(models.len(), 2, "Should have two DeepSeek built-in models");
    assert!(
        models.iter().all(|m| m.model.provider == "deepseek"),
        "Only DeepSeek built-in models should be present"
    );

    // No models should be available (no API keys)
    let available = registry.get_available();
    assert!(
        available.is_empty(),
        "No models should be available without API keys"
    );

    // No errors should be reported
    assert!(registry.error().is_none(), "No error expected");
}

#[test]
fn test_built_in_models_with_deepseek_key() {
    let harness = TestHarness::new("test_built_in_models_with_deepseek_key");
    harness.section("Setup");

    // Create auth with DeepSeek API key
    let auth_content = r#"{"deepseek": {"type": "api_key", "key": "sk-ds-test-key"}}"#;
    let auth_path = harness.create_file("auth.json", auth_content);
    let auth = AuthStorage::load(auth_path).expect("load auth");

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, None);

    harness.section("Verify");
    let deepseek_models: Vec<_> = registry
        .models()
        .iter()
        .filter(|m| m.model.provider == "deepseek")
        .collect();
    assert!(
        deepseek_models.iter().all(|m| m.api_key.is_some()),
        "All DeepSeek models should have API key"
    );

    let available = registry.get_available();
    assert_eq!(
        available.len(),
        deepseek_models.len(),
        "Only DeepSeek models should be available"
    );
}

#[test]
fn test_model_fields_populated_correctly() {
    let harness = TestHarness::new("test_model_fields_populated_correctly");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");
    let registry = ModelRegistry::load(&auth, None);

    harness.section("Verify DeepSeek models");
    let model = registry
        .find("deepseek", "deepseek-v4-flash")
        .expect("DeepSeek V4 Flash should exist");

    harness
        .log()
        .info_ctx("verify", "DeepSeek V4 Flash fields", |ctx| {
            ctx.push(("id".into(), model.model.id.clone()));
            ctx.push(("name".into(), model.model.name.clone()));
            ctx.push(("reasoning".into(), model.model.reasoning.to_string()));
            ctx.push((
                "context_window".into(),
                model.model.context_window.to_string(),
            ));
        });

    assert_eq!(model.model.id, "deepseek-v4-flash");
    assert_eq!(model.model.name, "DeepSeek V4 Flash");
    assert!(model.model.reasoning, "Flash should support reasoning");
    assert_eq!(model.model.api, "openai-completions");
    assert_eq!(model.model.base_url, "https://api.deepseek.com");
    assert!(model.model.context_window > 0, "Should have context window");
    assert!(model.model.max_tokens > 0, "Should have max tokens");
    assert!(!model.model.input.is_empty(), "Should have input types");

    let pro = registry
        .find("deepseek", "deepseek-v4-pro")
        .expect("DeepSeek V4 Pro should exist");
    assert!(pro.model.reasoning, "Pro should support reasoning");
}

// ============================================================================
// Custom models.json Tests
// ============================================================================

#[test]
fn test_custom_models_json_adds_new_provider() {
    let harness = TestHarness::new("test_custom_models_json_adds_new_provider");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // Create models.json with a custom provider
    let models_json = r#"{
        "providers": {
            "lmstudio": {
                "baseUrl": "http://localhost:1234/v1",
                "api": "openai-completions",
                "models": [
                    {
                        "id": "llama3-8b",
                        "name": "Llama 3 8B",
                        "contextWindow": 8192,
                        "maxTokens": 4096
                    }
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    harness.section("Verify");
    assert!(registry.error().is_none(), "No error expected");

    let lmstudio_model = registry.find("lmstudio", "llama3-8b");
    assert!(lmstudio_model.is_some(), "LMStudio model should exist");

    let model = lmstudio_model.unwrap();
    harness
        .log()
        .info_ctx("verify", "Custom model fields", |ctx| {
            ctx.push(("id".into(), model.model.id.clone()));
            ctx.push(("base_url".into(), model.model.base_url.clone()));
            ctx.push((
                "context_window".into(),
                model.model.context_window.to_string(),
            ));
        });

    assert_eq!(model.model.id, "llama3-8b");
    assert_eq!(model.model.name, "Llama 3 8B");
    assert_eq!(model.model.base_url, "http://localhost:1234/v1");
    assert_eq!(model.model.context_window, 8192);
    assert_eq!(model.model.max_tokens, 4096);
}

#[test]
fn test_custom_models_json_overrides_provider_config() {
    let harness = TestHarness::new("test_custom_models_json_overrides_provider_config");
    harness.section("Setup");

    // Create auth with DeepSeek key
    let auth_content = r#"{"deepseek": {"type": "api_key", "key": "sk-ds-test-key"}}"#;
    let auth_path = harness.create_file("auth.json", auth_content);
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // Create models.json that overrides DeepSeek base URL (e.g., for proxy)
    let models_json = r#"{
        "providers": {
            "deepseek": {
                "baseUrl": "https://my-proxy.example.com"
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    harness.section("Verify");
    assert!(registry.error().is_none(), "No error expected");

    // All DeepSeek models should have the overridden base URL
    let deepseek_models: Vec<_> = registry
        .models()
        .iter()
        .filter(|m| m.model.provider == "deepseek")
        .collect();

    assert!(!deepseek_models.is_empty(), "Should have DeepSeek models");
    for model in deepseek_models {
        assert_eq!(
            model.model.base_url, "https://my-proxy.example.com",
            "Base URL should be overridden for {}",
            model.model.id
        );
    }
}

#[test]
fn test_custom_models_json_replaces_provider_models() {
    let harness = TestHarness::new("test_custom_models_json_replaces_provider_models");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // Create models.json that fully replaces DeepSeek models
    let models_json = r#"{
        "providers": {
            "deepseek": {
                "baseUrl": "https://api.deepseek.com",
                "models": [
                    {"id": "custom-deepseek", "name": "Custom DeepSeek"}
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    harness.section("Verify");
    // Built-in DeepSeek models should be replaced
    let deepseek_models: Vec<_> = registry
        .models()
        .iter()
        .filter(|m| m.model.provider == "deepseek")
        .collect();

    harness
        .log()
        .info_ctx("verify", "DeepSeek models after replace", |ctx| {
            ctx.push(("count".into(), deepseek_models.len().to_string()));
            for m in &deepseek_models {
                ctx.push(("model".into(), m.model.id.clone()));
            }
        });

    assert_eq!(deepseek_models.len(), 1, "Should only have the custom model");
    assert_eq!(deepseek_models[0].model.id, "custom-deepseek");

    // Original DeepSeek built-ins should not exist after provider replacement.
    assert!(
        registry.find("deepseek", "deepseek-v4-flash").is_none(),
        "Built-in deepseek-v4-flash should be replaced"
    );
}

#[test]
fn test_model_with_reasoning_flag() {
    let harness = TestHarness::new("test_model_with_reasoning_flag");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "custom": {
                "baseUrl": "http://localhost:8080",
                "models": [
                    {"id": "reasoning-model", "name": "Reasoning Model", "reasoning": true},
                    {"id": "basic-model", "name": "Basic Model", "reasoning": false}
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let reasoning = registry.find("custom", "reasoning-model").unwrap();
    let basic = registry.find("custom", "basic-model").unwrap();

    assert!(reasoning.model.reasoning, "Should support reasoning");
    assert!(!basic.model.reasoning, "Should not support reasoning");
}

#[test]
fn test_model_with_input_types() {
    let harness = TestHarness::new("test_model_with_input_types");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "custom": {
                "baseUrl": "http://localhost:8080",
                "models": [
                    {"id": "text-only", "input": ["text"]},
                    {"id": "multimodal", "input": ["text", "image"]}
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let text_only = registry.find("custom", "text-only").unwrap();
    let multimodal = registry.find("custom", "multimodal").unwrap();

    assert_eq!(
        text_only.model.input.len(),
        1,
        "Text-only should have 1 input type"
    );
    assert_eq!(
        multimodal.model.input.len(),
        2,
        "Multimodal should have 2 input types"
    );
}

#[test]
fn test_model_with_cost_config() {
    let harness = TestHarness::new("test_model_with_cost_config");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "custom": {
                "baseUrl": "http://localhost:8080",
                "models": [
                    {
                        "id": "priced-model",
                        "cost": {
                            "input": 0.003,
                            "output": 0.015,
                            "cacheRead": 0.001,
                            "cacheWrite": 0.002
                        }
                    }
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let model = registry.find("custom", "priced-model").unwrap();
    assert!((model.model.cost.input - 0.003).abs() < f64::EPSILON);
    assert!((model.model.cost.output - 0.015).abs() < f64::EPSILON);
    assert!((model.model.cost.cache_read - 0.001).abs() < f64::EPSILON);
    assert!((model.model.cost.cache_write - 0.002).abs() < f64::EPSILON);
}

#[test]
fn test_model_with_headers() {
    let harness = TestHarness::new("test_model_with_headers");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "custom": {
                "baseUrl": "http://localhost:8080",
                "headers": {
                    "X-Provider-Header": "provider-value"
                },
                "models": [
                    {
                        "id": "header-model",
                        "headers": {
                            "X-Model-Header": "model-value"
                        }
                    }
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let model = registry.find("custom", "header-model").unwrap();

    harness.log().info_ctx("verify", "Model headers", |ctx| {
        for (k, v) in &model.headers {
            ctx.push((k.clone(), v.clone()));
        }
    });

    // Headers should be merged (provider + model)
    assert_eq!(
        model.headers.get("X-Provider-Header"),
        Some(&"provider-value".to_string())
    );
    assert_eq!(
        model.headers.get("X-Model-Header"),
        Some(&"model-value".to_string())
    );
}

#[test]
fn test_model_with_compat_config() {
    let harness = TestHarness::new("test_model_with_compat_config");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "custom": {
                "baseUrl": "http://localhost:8080",
                "compat": {
                    "supportsStore": true,
                    "supportsDeveloperRole": false
                },
                "models": [
                    {"id": "compat-model"}
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let model = registry.find("custom", "compat-model").unwrap();
    assert!(model.compat.is_some(), "Should have compat config");

    let compat = model.compat.as_ref().unwrap();
    assert_eq!(compat.supports_store, Some(true));
    assert_eq!(compat.supports_developer_role, Some(false));
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
fn test_malformed_models_json_reports_error() {
    let harness = TestHarness::new("test_malformed_models_json_reports_error");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // Create invalid JSON
    let models_path = harness.create_file("models.json", "{ invalid json }");

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, Some(models_path.clone()));

    harness.section("Verify");
    assert!(
        registry.error().is_some(),
        "Should report error for invalid JSON"
    );

    let error = registry.error().unwrap();
    harness.log().info_ctx("verify", "Error message", |ctx| {
        ctx.push(("error".into(), error.to_string()));
    });

    assert!(
        error.contains(&models_path.display().to_string()),
        "Error should mention file path"
    );

    // Built-in models should still be available
    assert!(
        !registry.models().is_empty(),
        "Built-in models should still load on error"
    );
}

#[test]
fn test_missing_models_json_no_error() {
    let harness = TestHarness::new("test_missing_models_json_no_error");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // Point to non-existent file
    let models_path = harness.temp_path("nonexistent_models.json");

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    harness.section("Verify");
    assert!(
        registry.error().is_none(),
        "Missing file should not report error"
    );
    assert!(!registry.models().is_empty(), "Built-in models should load");
}

#[test]
fn test_empty_providers_models_json_no_error() {
    let harness = TestHarness::new("test_empty_providers_models_json_no_error");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // Valid JSON with empty providers
    let models_path = harness.create_file("models.json", r#"{"providers": {}}"#);

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    harness.section("Verify");
    assert!(
        registry.error().is_none(),
        "Empty providers should not error"
    );
    assert!(!registry.models().is_empty(), "Built-in models should load");
}

#[test]
fn test_invalid_models_json_structure_reports_error() {
    let harness = TestHarness::new("test_invalid_models_json_structure_reports_error");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // JSON missing required 'providers' field
    let models_path = harness.create_file("models.json", "{}");

    harness.section("Load registry");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    harness.section("Verify");
    // Missing required field should cause error
    assert!(
        registry.error().is_some(),
        "Missing 'providers' should report error"
    );
    // But built-in models should still load
    assert!(
        !registry.models().is_empty(),
        "Built-in models should still load"
    );
}

// ============================================================================
// Find and Filter Tests
// ============================================================================

#[test]
fn test_find_model_by_provider_and_id() {
    let harness = TestHarness::new("test_find_model_by_provider_and_id");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");
    let registry = ModelRegistry::load(&auth, None);

    harness.section("Find existing models");
    assert!(registry.find("deepseek", "deepseek-v4-flash").is_some());
    assert!(registry.find("deepseek", "deepseek-v4-pro").is_some());

    harness.section("Find non-existing models");
    assert!(registry.find("deepseek", "nonexistent").is_none());
    assert!(registry.find("nonexistent", "deepseek-v4-flash").is_none());
}

#[test]
fn test_get_available_filters_by_api_key() {
    let harness = TestHarness::new("test_get_available_filters_by_api_key");
    harness.section("Setup");

    // Create auth with only DeepSeek key
    let auth_content = r#"{"deepseek": {"type": "api_key", "key": "sk-test-key"}}"#;
    let auth_path = harness.create_file("auth.json", auth_content);
    let auth = AuthStorage::load(auth_path).expect("load auth");
    let registry = ModelRegistry::load(&auth, None);

    harness.section("Verify");
    let available = registry.get_available();

    harness.log().info_ctx("verify", "Available models", |ctx| {
        ctx.push(("count".into(), available.len().to_string()));
        for m in &available {
            ctx.push((
                "model".into(),
                format!("{}/{}", m.model.provider, m.model.id),
            ));
        }
    });

    // Only DeepSeek models should be available
    assert!(
        available.iter().all(|m| m.model.provider == "deepseek"),
        "Only DeepSeek models should be available"
    );
    assert!(!available.is_empty(), "Should have available models");
}

// ============================================================================
// API Key Resolution Tests
// ============================================================================

#[test]
fn test_auth_header_flag() {
    let harness = TestHarness::new("test_auth_header_flag");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "bearer-provider": {
                "baseUrl": "http://localhost:8080",
                "authHeader": true,
                "models": [{"id": "bearer-model"}]
            },
            "custom-auth-provider": {
                "baseUrl": "http://localhost:8081",
                "authHeader": false,
                "models": [{"id": "custom-auth-model"}]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let bearer = registry.find("bearer-provider", "bearer-model").unwrap();
    let custom = registry
        .find("custom-auth-provider", "custom-auth-model")
        .unwrap();

    assert!(bearer.auth_header, "Should use Authorization header");
    assert!(!custom.auth_header, "Should not use Authorization header");
}

// ============================================================================
// Default Path Tests
// ============================================================================

#[test]
fn test_default_models_path() {
    let harness = TestHarness::new("test_default_models_path");
    harness.section("Setup");

    let agent_dir = harness.temp_path("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();

    harness.section("Verify");
    let path = default_models_path(&agent_dir);

    harness.log().info_ctx("verify", "Default path", |ctx| {
        ctx.push(("path".into(), path.display().to_string()));
    });

    assert!(path.ends_with("models.json"), "Should end with models.json");
    assert!(
        path.starts_with(&agent_dir),
        "Should be inside agent directory"
    );
}

// ============================================================================
// Model Name Defaults
// ============================================================================

#[test]
fn test_model_name_defaults_to_id() {
    let harness = TestHarness::new("test_model_name_defaults_to_id");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "custom": {
                "baseUrl": "http://localhost:8080",
                "models": [
                    {"id": "no-name-model"}
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let model = registry.find("custom", "no-name-model").unwrap();
    assert_eq!(
        model.model.name, "no-name-model",
        "Name should default to ID"
    );
}

// ============================================================================
// Context Window and Max Tokens Defaults
// ============================================================================

#[test]
fn test_context_window_and_max_tokens_defaults() {
    let harness = TestHarness::new("test_context_window_and_max_tokens_defaults");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    let models_json = r#"{
        "providers": {
            "custom": {
                "baseUrl": "http://localhost:8080",
                "models": [
                    {"id": "defaults-model"}
                ]
            }
        }
    }"#;
    let models_path = harness.create_file("models.json", models_json);

    harness.section("Load and verify");
    let registry = ModelRegistry::load(&auth, Some(models_path));

    let model = registry.find("custom", "defaults-model").unwrap();

    harness.log().info_ctx("verify", "Default values", |ctx| {
        ctx.push((
            "context_window".into(),
            model.model.context_window.to_string(),
        ));
        ctx.push(("max_tokens".into(), model.model.max_tokens.to_string()));
    });

    // Should have reasonable defaults
    assert_eq!(
        model.model.context_window, 128_000,
        "Default context window"
    );
    assert_eq!(model.model.max_tokens, 16_384, "Default max tokens");
}

// ============================================================================
// Deterministic Model Ordering
// ============================================================================

#[test]
fn test_model_order_is_stable() {
    let harness = TestHarness::new("test_model_order_is_stable");
    harness.section("Setup");

    let auth_path = harness.create_file("auth.json", "{}");
    let auth = AuthStorage::load(auth_path).expect("load auth");

    // Load registry twice
    let registry1 = ModelRegistry::load(&auth, None);
    let registry2 = ModelRegistry::load(&auth, None);

    harness.section("Verify");
    let models1: Vec<_> = registry1.models().iter().map(|m| &m.model.id).collect();
    let models2: Vec<_> = registry2.models().iter().map(|m| &m.model.id).collect();

    assert_eq!(models1, models2, "Model ordering should be stable");
}

// ============================================================================
// Multiple Providers with API Keys
// ============================================================================

#[test]
fn test_multiple_providers_with_keys() {
    let harness = TestHarness::new("test_multiple_providers_with_keys");
    harness.section("Setup");

    let auth_content = r#"{"deepseek": {"type": "api_key", "key": "sk-ds-test"}}"#;
    let auth_path = harness.create_file("auth.json", auth_content);
    let auth = AuthStorage::load(auth_path).expect("load auth");
    let registry = ModelRegistry::load(&auth, None);

    harness.section("Verify");
    let available = registry.get_available();

    let providers: std::collections::HashSet<_> = available
        .iter()
        .map(|m| m.model.provider.as_str())
        .collect();

    harness
        .log()
        .info_ctx("verify", "Available providers", |ctx| {
            for p in &providers {
                ctx.push(("provider".to_string(), (*p).to_string()));
            }
        });

    assert_eq!(providers.len(), 1);
    assert!(providers.contains("deepseek"), "DeepSeek should be available");
}
