/// Verification: sequential vs concurrent Gemini API calls for image description.
///
/// Usage:
///   GEMINI_API_KEY=<key> cargo run --example gemini_async_verify
///
/// This example loads sample images, calls Gemini API both sequentially and
/// concurrently, and compares wall-clock times.
use std::time::Instant;

use base64::Engine;
use reqwest::Client;
use serde_json::json;

const MODEL: &str = "gemini-2.5-flash-lite";
const PROMPT: &str = "Describe this image concisely for use as alt text.";

async fn describe_image(
    client: &Client,
    api_key: &str,
    image_bytes: &[u8],
    mime_type: &str,
) -> Result<String, String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes);

    let body = json!({
        "contents": [{
            "parts": [
                {
                    "inline_data": {
                        "mime_type": mime_type,
                        "data": encoded
                    }
                },
                {
                    "text": PROMPT
                }
            ]
        }]
    });

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        MODEL
    );

    let resp = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if let Some(error) = json.get("error") {
        return Err(format!(
            "API error: {}",
            error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
        ));
    }

    json.get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.get(0))
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "unexpected response structure".to_string())
}

fn detect_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else {
        "application/octet-stream"
    }
}

#[tokio::main]
async fn main() {
    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let client = Client::new();

    // Load sample images and repeat to get 5 calls
    let image_paths = &["examples/demo/sample.png", "examples/demo/sample.jpg"];

    let raw: Vec<(String, Vec<u8>)> = image_paths
        .iter()
        .map(|p| {
            let data = std::fs::read(p).unwrap_or_else(|e| panic!("failed to read {p}: {e}"));
            (p.to_string(), data)
        })
        .collect();

    // Build 10 image entries by cycling through the loaded files
    let images: Vec<(String, Vec<u8>)> = (0..10)
        .map(|i| {
            let (path, data) = &raw[i % raw.len()];
            (format!("[{}] {}", i, path), data.clone())
        })
        .collect();

    let n = images.len();
    println!("Loaded {} image calls\n", n);

    // --- Sequential ---
    println!("=== Sequential ({n} calls) ===");
    let seq_start = Instant::now();
    for (path, data) in &images {
        let mime = detect_mime(data);
        let t0 = seq_start.elapsed();
        let result = describe_image(&client, &api_key, data, mime).await;
        let t1 = seq_start.elapsed();
        match result {
            Ok(desc) => println!(
                "  {path}  start={t0:.2?}  end={t1:.2?}  ({:.2?})  {}",
                t1 - t0,
                truncate(&desc, 60)
            ),
            Err(e) => println!("  {path}  start={t0:.2?}  end={t1:.2?}  ERROR: {e}"),
        }
    }
    let seq_elapsed = seq_start.elapsed();
    println!("Sequential total: {:.2?}\n", seq_elapsed);

    // --- Concurrent ---
    println!("=== Concurrent ({n} calls) ===");
    let conc_start = Instant::now();
    let mut handles = Vec::new();
    for (path, data) in &images {
        let client = client.clone();
        let api_key = api_key.clone();
        let mime = detect_mime(data);
        let data = data.clone();
        let path = path.clone();
        let t_ref = conc_start;
        handles.push(tokio::spawn(async move {
            let t0 = t_ref.elapsed();
            let result = describe_image(&client, &api_key, &data, mime).await;
            let t1 = t_ref.elapsed();
            (path, t0, t1, result)
        }));
    }

    for handle in handles {
        match handle.await {
            Ok((path, t0, t1, Ok(desc))) => println!(
                "  {path}  start={t0:.2?}  end={t1:.2?}  ({:.2?})  {}",
                t1 - t0,
                truncate(&desc, 60)
            ),
            Ok((path, t0, t1, Err(e))) => {
                println!("  {path}  start={t0:.2?}  end={t1:.2?}  ERROR: {e}")
            }
            Err(e) => println!("  task panicked: {e}"),
        }
    }
    let conc_elapsed = conc_start.elapsed();
    println!("Concurrent total: {:.2?}\n", conc_elapsed);

    // --- Summary ---
    println!("=== Summary ===");
    println!("Sequential: {:.2?}", seq_elapsed);
    println!("Concurrent: {:.2?}", conc_elapsed);
    if seq_elapsed > conc_elapsed {
        let speedup = seq_elapsed.as_secs_f64() / conc_elapsed.as_secs_f64();
        println!("Speedup:    {:.2}x", speedup);
    } else {
        println!("No speedup (possibly rate-limited or too few images)");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
