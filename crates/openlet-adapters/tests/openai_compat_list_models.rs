//! `OpenAiCompatProvider::list_models` — catalog fetch + decode.
//!
//! Drives the real provider against the in-process mock service, which
//! serves a small canned catalog on `GET /models`. Covers:
//! - Happy path: 200 + JSON `{data:[…]}` decodes into `ModelInfo`.
//! - Auth failure (401) → typed `ProviderError::Auth`.
//! - Server error (500) → typed `ProviderError::Network`.

use openlet_adapters::openai_compat::OpenAiCompatProvider;
use openlet_core::adapters::model_provider::ModelProvider;
use openlet_core::error::ProviderError;
use secrecy::SecretString;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn list_models_decodes_catalog_from_mock_service() {
    let mock = openlet_test_mock_provider::MockOpenAiService::spawn()
        .await
        .expect("spawn mock");
    let provider = OpenAiCompatProvider::new(mock.base_url(), Some(SecretString::from("test-key")));

    let models = provider.list_models().await.expect("list_models ok");

    // Mock serves two canned entries.
    assert_eq!(models.len(), 2, "expected 2 models, got {models:?}");
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"mock/model-small"), "ids={ids:?}");
    assert!(ids.contains(&"mock/model-large"), "ids={ids:?}");

    let large = models
        .iter()
        .find(|m| m.id == "mock/model-large")
        .expect("large present");
    assert_eq!(large.display_name.as_deref(), Some("Mock Large"));
    assert_eq!(large.context_length, Some(128_000));
}

#[tokio::test]
async fn list_models_maps_401_to_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"nope"}"#))
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), Some(SecretString::from("bad")));
    let err = provider.list_models().await.expect_err("must fail");
    assert!(matches!(err, ProviderError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn list_models_maps_500_to_network_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), Some(SecretString::from("test-key")));
    let err = provider.list_models().await.expect_err("must fail");
    assert!(matches!(err, ProviderError::Network(_)), "got {err:?}");
}
