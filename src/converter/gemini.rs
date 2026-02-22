use crate::converter::ImageDescriber;
use crate::error::ConvertError;

#[cfg(feature = "async-gemini")]
use std::future::Future;
#[cfg(feature = "async-gemini")]
use std::pin::Pin;

/// Built-in `ImageDescriber` that uses the Google Gemini API.
///
/// Always available (no feature flag required). For the async variant,
/// see `AsyncGeminiDescriber` (requires the `async-gemini` feature).
///
/// # Example
///
/// ```no_run
/// use anytomd::gemini::GeminiDescriber;
///
/// let describer = GeminiDescriber::new("your-api-key".to_string());
/// // or from the GEMINI_API_KEY environment variable:
/// let describer = GeminiDescriber::from_env().unwrap();
/// ```
pub struct GeminiDescriber {
    api_key: String,
    model: String,
}

impl std::fmt::Debug for GeminiDescriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiDescriber")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

impl GeminiDescriber {
    /// Create a new `GeminiDescriber` with the given API key.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: "gemini-3-flash-preview".to_string(),
        }
    }

    /// Create a new `GeminiDescriber` by reading the `GEMINI_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ConvertError> {
        let api_key =
            std::env::var("GEMINI_API_KEY").map_err(|_| ConvertError::ImageDescriptionError {
                reason: "GEMINI_API_KEY environment variable not set".to_string(),
            })?;
        Ok(Self::new(api_key))
    }

    /// Set a custom model name (default: `gemini-3-flash-preview`).
    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }
}

/// Parse the text response from a Gemini `generateContent` JSON response body.
///
/// Extracts `candidates[0].content.parts[0].text`.
fn parse_response(body: &str) -> Result<String, ConvertError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| ConvertError::ImageDescriptionError {
            reason: format!("failed to parse Gemini response: {e}"),
        })?;

    // Check for API error
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(ConvertError::ImageDescriptionError {
            reason: format!("Gemini API error: {message}"),
        });
    }

    let text = value
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.get(0))
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| ConvertError::ImageDescriptionError {
            reason:
                "unexpected Gemini response structure: missing candidates[0].content.parts[0].text"
                    .to_string(),
        })?;

    Ok(text.trim().to_string())
}

impl ImageDescriber for GeminiDescriber {
    fn describe(
        &self,
        image_bytes: &[u8],
        mime_type: &str,
        prompt: &str,
    ) -> Result<String, ConvertError> {
        use base64::Engine;

        let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes);

        let request_body = serde_json::json!({
            "contents": [{
                "parts": [
                    {
                        "inline_data": {
                            "mime_type": mime_type,
                            "data": encoded
                        }
                    },
                    {
                        "text": prompt
                    }
                ]
            }]
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            self.model
        );

        let json_body = request_body.to_string();

        let response = ureq::post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &self.api_key)
            .send(json_body.as_bytes())
            .map_err(|e| ConvertError::ImageDescriptionError {
                reason: format!("Gemini API request failed: {e}"),
            })?;

        let body = response.into_body().read_to_string().map_err(|e| {
            ConvertError::ImageDescriptionError {
                reason: format!("failed to read Gemini response body: {e}"),
            }
        })?;

        parse_response(&body)
    }
}

/// Async built-in `AsyncImageDescriber` that uses the Google Gemini API via `reqwest`.
///
/// Requires the `async-gemini` feature flag.
///
/// # Example
///
/// ```no_run
/// use anytomd::gemini::AsyncGeminiDescriber;
///
/// let describer = AsyncGeminiDescriber::new("your-api-key".to_string());
/// // or from the GEMINI_API_KEY environment variable:
/// let describer = AsyncGeminiDescriber::from_env().unwrap();
/// ```
#[cfg(feature = "async-gemini")]
pub struct AsyncGeminiDescriber {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

#[cfg(feature = "async-gemini")]
impl std::fmt::Debug for AsyncGeminiDescriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncGeminiDescriber")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

#[cfg(feature = "async-gemini")]
impl AsyncGeminiDescriber {
    /// Create a new `AsyncGeminiDescriber` with the given API key.
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model: "gemini-3-flash-preview".to_string(),
        }
    }

    /// Create a new `AsyncGeminiDescriber` by reading the `GEMINI_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ConvertError> {
        let api_key =
            std::env::var("GEMINI_API_KEY").map_err(|_| ConvertError::ImageDescriptionError {
                reason: "GEMINI_API_KEY environment variable not set".to_string(),
            })?;
        Ok(Self::new(api_key))
    }

    /// Set a custom model name (default: `gemini-3-flash-preview`).
    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }
}

#[cfg(feature = "async-gemini")]
impl crate::converter::AsyncImageDescriber for AsyncGeminiDescriber {
    fn describe<'a>(
        &'a self,
        image_bytes: &'a [u8],
        mime_type: &'a str,
        prompt: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ConvertError>> + Send + 'a>> {
        Box::pin(async move {
            use base64::Engine;

            let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes);

            let request_body = serde_json::json!({
                "contents": [{
                    "parts": [
                        {
                            "inline_data": {
                                "mime_type": mime_type,
                                "data": encoded
                            }
                        },
                        {
                            "text": prompt
                        }
                    ]
                }]
            });

            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                self.model
            );

            let response = self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("x-goog-api-key", &self.api_key)
                .json(&request_body)
                .send()
                .await
                .map_err(|e| ConvertError::ImageDescriptionError {
                    reason: format!("Gemini API request failed: {e}"),
                })?;

            let body = response
                .text()
                .await
                .map_err(|e| ConvertError::ImageDescriptionError {
                    reason: format!("failed to read Gemini response body: {e}"),
                })?;

            parse_response(&body)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate the `GEMINI_API_KEY` environment variable
    /// to prevent race conditions when tests run in parallel.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn set_env_key(value: &str) {
        // SAFETY: All callers are inside `with_env_key`, which holds `ENV_MUTEX`,
        // serializing process environment mutation for this test module.
        unsafe { std::env::set_var("GEMINI_API_KEY", value) };
    }

    fn remove_env_key() {
        // SAFETY: All callers are inside `with_env_key`, which holds `ENV_MUTEX`,
        // serializing process environment mutation for this test module.
        unsafe { std::env::remove_var("GEMINI_API_KEY") };
    }

    /// Saves the current `GEMINI_API_KEY` value, runs the closure, then restores it.
    fn with_env_key<F: FnOnce()>(f: F) {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var("GEMINI_API_KEY").ok();
        f();
        match original {
            Some(v) => set_env_key(&v),
            None => remove_env_key(),
        }
    }

    #[test]
    fn test_gemini_describer_new() {
        let describer = GeminiDescriber::new("test-key".to_string());
        assert_eq!(describer.api_key, "test-key");
        assert_eq!(describer.model, "gemini-3-flash-preview");
    }

    #[test]
    fn test_gemini_describer_with_model() {
        let describer =
            GeminiDescriber::new("key".to_string()).with_model("gemini-2.0-flash".to_string());
        assert_eq!(describer.model, "gemini-2.0-flash");
    }

    #[test]
    fn test_gemini_describer_from_env_missing_key() {
        with_env_key(|| {
            remove_env_key();
            let result = GeminiDescriber::from_env();
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                format!("{err}").contains("GEMINI_API_KEY"),
                "error was: {err}"
            );
        });
    }

    #[test]
    fn test_gemini_describer_from_env_with_key() {
        with_env_key(|| {
            set_env_key("test-env-key");
            let result = GeminiDescriber::from_env();
            assert!(result.is_ok());
            let describer = result.unwrap();
            assert_eq!(describer.api_key, "test-env-key");
        });
    }

    #[test]
    fn test_parse_response_valid() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "A photo of a sunset over the ocean."
                    }]
                }
            }]
        }"#;
        let result = parse_response(json).unwrap();
        assert_eq!(result, "A photo of a sunset over the ocean.");
    }

    #[test]
    fn test_parse_response_with_whitespace_trimmed() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "  A cat sitting on a chair.  \n"
                    }]
                }
            }]
        }"#;
        let result = parse_response(json).unwrap();
        assert_eq!(result, "A cat sitting on a chair.");
    }

    #[test]
    fn test_parse_response_api_error() {
        let json = r#"{
            "error": {
                "code": 403,
                "message": "API key not valid",
                "status": "PERMISSION_DENIED"
            }
        }"#;
        let result = parse_response(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("API key not valid"),
            "error was: {err}"
        );
    }

    #[test]
    fn test_parse_response_missing_candidates() {
        let json = r#"{"result": "unexpected"}"#;
        let result = parse_response(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("unexpected Gemini response structure"),
            "error was: {err}"
        );
    }

    #[test]
    fn test_parse_response_invalid_json() {
        let result = parse_response("not json");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("failed to parse"),
            "error was: {err}"
        );
    }

    #[test]
    fn test_parse_response_empty_candidates_array() {
        let json = r#"{"candidates": []}"#;
        let result = parse_response(json);
        assert!(result.is_err());
    }

    #[cfg(feature = "async-gemini")]
    mod async_gemini_tests {
        use super::*;

        #[test]
        fn test_async_gemini_describer_new() {
            let describer = AsyncGeminiDescriber::new("test-key".to_string());
            assert_eq!(describer.api_key, "test-key");
            assert_eq!(describer.model, "gemini-3-flash-preview");
        }

        #[test]
        fn test_async_gemini_describer_with_model() {
            let describer = AsyncGeminiDescriber::new("key".to_string())
                .with_model("gemini-2.0-flash".to_string());
            assert_eq!(describer.model, "gemini-2.0-flash");
        }

        #[test]
        fn test_async_gemini_describer_from_env_missing_key() {
            super::with_env_key(|| {
                super::remove_env_key();
                let result = AsyncGeminiDescriber::from_env();
                assert!(result.is_err());
                let err = result.unwrap_err();
                assert!(
                    format!("{err}").contains("GEMINI_API_KEY"),
                    "error was: {err}"
                );
            });
        }

        #[test]
        fn test_async_gemini_describer_from_env_with_key() {
            super::with_env_key(|| {
                super::set_env_key("test-async-env-key");
                let result = AsyncGeminiDescriber::from_env();
                assert!(result.is_ok());
                let describer = result.unwrap();
                assert_eq!(describer.api_key, "test-async-env-key");
            });
        }

        #[test]
        fn test_async_gemini_describer_debug_redacts_key() {
            let describer = AsyncGeminiDescriber::new("secret-key".to_string());
            let debug = format!("{:?}", describer);
            assert!(debug.contains("[REDACTED]"));
            assert!(!debug.contains("secret-key"));
        }

        #[test]
        fn test_async_gemini_describer_implements_trait() {
            use crate::converter::AsyncImageDescriber;
            let describer = AsyncGeminiDescriber::new("key".to_string());
            // Verify it implements AsyncImageDescriber by using it as a trait object
            let _: &dyn AsyncImageDescriber = &describer;
        }
    }
}
