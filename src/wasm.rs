//! WebAssembly bindings for anytomd via `wasm-bindgen`.
//!
//! Provides `convertBytes` and `convertBytesWithOptions` for use from JavaScript.
//! These functions accept raw document bytes and return a JS object with
//! `markdown`, `plainText`, `title`, and `warnings` fields.

use wasm_bindgen::prelude::*;

use crate::convert_bytes;
use crate::converter::ConversionOptions;

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
    convert_bytes_inner(data, extension, &options)
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
    convert_bytes_inner(data, extension, &options)
}

fn convert_bytes_inner(
    data: &[u8],
    extension: &str,
    options: &ConversionOptions,
) -> Result<JsValue, JsError> {
    let result =
        convert_bytes(data, extension, options).map_err(|e| JsError::new(&e.to_string()))?;

    let obj = js_sys::Object::new();

    js_sys::Reflect::set(&obj, &"markdown".into(), &result.markdown.into())
        .map_err(|_| JsError::new("failed to set markdown property"))?;
    js_sys::Reflect::set(&obj, &"plainText".into(), &result.plain_text.into())
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
