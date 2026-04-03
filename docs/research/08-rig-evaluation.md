# Rig (rig-core) Evaluation

**Date:** 2026-04-03
**Verdict:** Not adopting. No meaningful value for Meridian.

---

## What Rig Is

Rig is a Rust library for building LLM-powered applications. It provides a unified interface across 20+ LLM providers, embedding model abstractions, vector store integrations, and agent/RAG pipeline primitives.

**Version evaluated:** 0.24.0
**Repo:** https://github.com/0xplaygrounds/rig

## Feature Inventory

| Feature | What it does |
|---|---|
| Provider clients | Unified `CompletionModel` trait across OpenAI, Anthropic, Cohere, etc. |
| Embedding models | `EmbeddingModel` trait, `EmbeddingsBuilder` for batch embedding |
| Vector stores | `VectorStoreIndex` trait with `top_n` / `top_n_ids`. 10+ backends: Qdrant, MongoDB, LanceDB, SQLite, Neo4j, Milvus, ScyllaDB, etc. |
| `rig-fastembed` | Wraps fastembed-rs to implement `EmbeddingModel` trait |
| `rig-qdrant` | Thin wrapper implementing `VectorStoreIndex` over qdrant-client |
| Agent type | High-level abstraction combining model + system prompt + tools + optional RAG |
| Pipeline module | Composable op chains (map/filter for LLM steps) |
| Loaders | File/PDF/CSV ingestion utilities |
| Streaming | Unified streaming completion trait across providers |
| `Embed` derive macro | Auto-derive which struct fields to embed |
| WASM | Core library compiles to wasm32-unknown-unknown |

## Why It Doesn't Fit Meridian

### Overlap is only on the easy parts

Meridian's `MessageConnector` trait is a single method: `send(messages, tools, config) -> Response`. Implementing that for Anthropic and OpenAI is ~100-200 lines each with reqwest. Rig would replace these stubs but brings a large dependency for trivial work.

### Abstraction conflict

Rig has its own `Agent` type, pipeline model, and vector store abstraction. These fight with Meridian's `Agent` state machine, `Store` traits, and FastEmbed ONNX embeddings. Adopting rig means either using a fraction of it (wasteful) or conforming to its opinions (counterproductive).

### TaskConnector has no rig equivalent

Meridian's `TaskConnector` trait manages agent process spawning and lifecycle (spawn, status, kill, wait). This is Meridian-specific. Rig doesn't do process management.

### Token counting control

Meridian needs precise token usage tracking for checkpoint thresholds (60%/75%/85%). Wrapping rig's responses to extract this adds a leaky abstraction.

### rig-fastembed duplicates existing work

Meridian's `embedding` crate already wraps fastembed-rs with its own `EmbeddingProvider` trait. `rig-fastembed` does the same thing but for rig's trait hierarchy.

### rig-qdrant is irrelevant

A thin glue layer between rig's `EmbeddingModel` and Qdrant's client. Meridian uses sqlite-vec for vector search and has no Qdrant dependency.

## One Possible Future Use

If Meridian ever needs to support a long tail of LLM providers (Ollama, Groq, Mistral, etc.) without writing individual connectors, rig's provider layer could be wrapped behind `MessageConnector` as one more `MessageConnectorKind` variant. This is a "nice to have later" — not an MVP concern.

## Conclusion

Rig solves the easy part (API call plumbing) and ignores the hard part (orchestration lifecycle). For Meridian's current stage, writing the two stub connectors directly is simpler and gives more control. Nothing worth pulling in as a dependency or porting.
