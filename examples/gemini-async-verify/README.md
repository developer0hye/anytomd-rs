# Experiment: Concurrent Gemini API Calls

## Motivation

The current `ImageDescriber` implementation calls the Gemini API **sequentially** for each image in a document (see `ooxml_utils::resolve_image_placeholders`). For documents with many embedded images, API latency accumulates linearly.

This experiment verifies whether concurrent (async) calls to the Gemini `generateContent` endpoint yield meaningful speedup, as a prerequisite for redesigning the `ImageDescriber` trait.

## Setup

- **Async HTTP**: `reqwest` + `tokio` (de facto standard in the Rust ecosystem)
- **Model**: `gemini-2.5-flash-lite` (CI/cost-savings tier per CLAUDE.md)
- **Test images**: `examples/demo/sample.png` and `examples/demo/sample.jpg`, cycled to produce N calls

## How to Run

```bash
GEMINI_API_KEY=<your-key> cargo run --example gemini-async-verify
```

## Results

### 2 images

| Mode | Wall Time | Speedup |
|------|-----------|---------|
| Sequential | 3.84s | - |
| Concurrent | 1.30s | **2.95x** |

### 5 images

| Mode | Wall Time | Speedup |
|------|-----------|---------|
| Sequential | 6.87s | - |
| Concurrent | 1.93s | **3.57x** |

### 10 images

| Mode | Wall Time | Speedup |
|------|-----------|---------|
| Sequential | 12.27s | - |
| Concurrent | 2.16s | **5.68x** |

### Per-request timing (10 images, concurrent)

All 10 requests start within ~0.1ms of each other, confirming true parallelism:

```
[0]  start=45.29us   end=1.20s  (1.20s)
[1]  start=85.25us   end=1.28s  (1.28s)
[2]  start=125.08us  end=1.47s  (1.47s)
[3]  start=756.13us  end=1.93s  (1.93s)
[4]  start=155.88us  end=1.14s  (1.14s)
[5]  start=215.71us  end=1.14s  (1.14s)
[6]  start=231.58us  end=1.42s  (1.42s)
[7]  start=252.38us  end=1.14s  (1.14s)
[8]  start=268.04us  end=1.24s  (1.24s)
[9]  start=766.04us  end=1.42s  (1.42s)
```

Total wall time is bounded by the **slowest single request** (~1.9s), not the sum.

## Key Findings

1. **Concurrent calls work** -- Gemini `generateContent` accepts multiple simultaneous requests without errors
2. **Speedup scales with image count** -- 2.95x (2 imgs) -> 3.57x (5 imgs) -> 5.68x (10 imgs)
3. **Theoretical max not reached** due to API response time variance (1.0s - 2.1s per request); wall time = max(individual latencies)
4. **No rate limit errors** observed at 10 concurrent requests on free tier (RPM limit: 10-15)
5. **`reqwest` + `tokio`** is a viable async stack for the `ImageDescriber` redesign

## Next Steps

- Redesign `ImageDescriber` trait to support async (`async fn describe`)
- Add concurrency limit (semaphore) to respect API rate limits
- Consider the library-as-dependency story: expose `async` API, let callers provide the runtime
