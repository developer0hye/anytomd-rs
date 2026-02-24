//! WebAssembly bindings for anytomd via `wasm-bindgen`.
//!
//! Provides `convertBytes` and `convertBytesWithOptions` for use from JavaScript.
//! When the `async-gemini` feature is also enabled, `convertBytesWithGemini` is
//! available for async conversion with Gemini-powered image descriptions.
//!
//! All functions return a JS object with `markdown`, `plainText`, `title`, and
//! `warnings` fields.

use wasm_bindgen::prelude::*;

use crate::convert_bytes;
use crate::converter::{ConversionOptions, ConversionResult};

/// Convert raw document bytes to Markdown from JavaScript.
///
/// # Arguments
///
/// * `data` - Raw file bytes as a `Uint8Array`
/// * `extension` - File format extension (e.g., `"docx"`, `"csv"`, `"html"`)
///
/// # Returns
///
/// A JS object with fields: `markdown` (string), `plainText` (string),
/// `title` (string or null), `warnings` (array of strings).
#[wasm_bindgen(js_name = "convertBytes")]
pub fn convert_bytes_wasm(data: &[u8], extension: &str) -> Result<JsValue, JsError> {
    let options = ConversionOptions::default();
    let result =
        convert_bytes(data, extension, &options).map_err(|e| JsError::new(&e.to_string()))?;
    convert_result_to_js(&result)
}

/// Convert raw document bytes to Markdown with options from JavaScript.
///
/// # Arguments
///
/// * `data` - Raw file bytes as a `Uint8Array`
/// * `extension` - File format extension (e.g., `"docx"`, `"csv"`, `"html"`)
/// * `strict` - If `true`, treat recoverable parse errors as hard errors
///
/// # Returns
///
/// A JS object with fields: `markdown` (string), `plainText` (string),
/// `title` (string or null), `warnings` (array of strings).
#[wasm_bindgen(js_name = "convertBytesWithOptions")]
pub fn convert_bytes_with_options_wasm(
    data: &[u8],
    extension: &str,
    strict: bool,
) -> Result<JsValue, JsError> {
    let options = ConversionOptions {
        strict,
        ..Default::default()
    };
    let result =
        convert_bytes(data, extension, &options).map_err(|e| JsError::new(&e.to_string()))?;
    convert_result_to_js(&result)
}

/// Convert raw document bytes to Markdown with Gemini-powered image descriptions.
///
/// This is an async function that creates an `AsyncGeminiDescriber` with the
/// given API key and uses `convert_bytes_async` to resolve image descriptions
/// concurrently via the Gemini API.
///
/// # Arguments
///
/// * `data` - Raw file bytes as a `Uint8Array`
/// * `extension` - File format extension (e.g., `"docx"`, `"pptx"`, `"xlsx"`)
/// * `api_key` - Google Gemini API key
/// * `model` - Optional Gemini model name (defaults to `gemini-3-flash-preview`)
///
/// # Returns
///
/// A Promise that resolves to a JS object with fields: `markdown` (string),
/// `plainText` (string), `title` (string or null), `warnings` (array of strings).
#[cfg(feature = "async-gemini")]
#[wasm_bindgen(js_name = "convertBytesWithGemini")]
pub async fn convert_bytes_with_gemini_wasm(
    data: &[u8],
    extension: &str,
    api_key: &str,
    model: Option<String>,
) -> Result<JsValue, JsError> {
    use std::sync::Arc;

    use crate::convert_bytes_async;
    use crate::converter::AsyncConversionOptions;
    use crate::gemini::AsyncGeminiDescriber;

    let describer = match model {
        Some(m) => AsyncGeminiDescriber::new(api_key.to_string()).with_model(m),
        None => AsyncGeminiDescriber::new(api_key.to_string()),
    };

    let options = AsyncConversionOptions {
        async_image_describer: Some(Arc::new(describer)),
        ..Default::default()
    };

    let result = convert_bytes_async(data, extension, &options)
        .await
        .map_err(|e| JsError::new(&e.to_string()))?;
    convert_result_to_js(&result)
}

/// Convert a `ConversionResult` into a JS object.
fn convert_result_to_js(result: &ConversionResult) -> Result<JsValue, JsError> {
    let obj = js_sys::Object::new();

    js_sys::Reflect::set(&obj, &"markdown".into(), &result.markdown.as_str().into())
        .map_err(|_| JsError::new("failed to set markdown property"))?;
    js_sys::Reflect::set(
        &obj,
        &"plainText".into(),
        &result.plain_text.as_str().into(),
    )
    .map_err(|_| JsError::new("failed to set plainText property"))?;
    js_sys::Reflect::set(
        &obj,
        &"title".into(),
        &match result.title {
            Some(ref t) => JsValue::from_str(t),
            None => JsValue::NULL,
        },
    )
    .map_err(|_| JsError::new("failed to set title property"))?;

    let warnings_arr = js_sys::Array::new();
    for w in &result.warnings {
        let msg = match &w.location {
            Some(loc) => format!("[{:?}] {} ({})", w.code, w.message, loc),
            None => format!("[{:?}] {}", w.code, w.message),
        };
        warnings_arr.push(&msg.into());
    }
    js_sys::Reflect::set(&obj, &"warnings".into(), &warnings_arr.into())
        .map_err(|_| JsError::new("failed to set warnings property"))?;

    Ok(obj.into())
}
