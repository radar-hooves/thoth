# Enhancement Latency Benchmark

Local Ollama enhancement round-trip latency at Thoth's real entry-size distribution. Enhancement (off by default) runs on the critical path: when enabled, text reaches the cursor only after this round-trip. For reference, the current transcription baseline (the "razor fast" feel) is p50 ~270ms, p90 ~720ms; entry word counts are p50=36, p90=123, p99=341.

Regenerate / append a run: `python3 scripts/bench_enhancement_latency.py`

## Run 2026-06-24 09:57 AEST

- Hardware: Apple M1 Max
- Ollama: 0.30.10 · warm runs/median: 5

| model        | input           | cold total | load   | warm total | tok/s | out tok |
| ------------ | --------------- | ---------- | ------ | ---------- | ----- | ------- |
| llama3.2     | p50_36w (36w)   | 7309ms     | 6696ms | 609ms      | 97.5  | 41      |
| llama3.2     | p90_123w (123w) | —          | —      | 1674ms     | 95.7  | 142     |
| llama3.2     | p99_341w (341w) | —          | —      | 4096ms     | 93.7  | 365     |
| llama3.2:1b  | p50_36w (36w)   | 3439ms     | 3055ms | 455ms      | 179.1 | 46      |
| llama3.2:1b  | p90_123w (123w) | —          | —      | 950ms      | 180.8 | 138     |
| llama3.2:1b  | p99_341w (341w) | —          | —      | 2300ms     | 178.7 | 379     |
| qwen2.5:1.5b | p50_36w (36w)   | 2542ms     | 1758ms | 467ms      | 151.4 | 48      |
| qwen2.5:1.5b | p90_123w (123w) | —          | —      | 1013ms     | 149.5 | 127     |
| qwen2.5:1.5b | p99_341w (341w) | —          | —      | 2714ms     | 148.6 | 380     |

### Verdict (24/06/2026)

Even the fastest warm model (llama3.2:1b) roughly triples time-to-cursor at the median (270ms → ~725ms) and is laggy by p90 — confirms enhancement stays OFF the default cursor path, as a deliberate opt-in "polish lane" rather than instant. If enabled, default to `llama3.2:1b` (3x faster warm than the configured 3B `llama3.2`) and pin it warm; A/B cleanup quality before switching the default, since only latency was measured here. Cold model-load penalty after Ollama's 5-minute idle-unload: 3–6.7s.
