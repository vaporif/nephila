# Deep Dive: xMemory -- Beyond RAG for Agent Memory: Retrieval by Decoupling and Aggregation

**Date:** 2026-03-29
**Paper:** arXiv:2602.02007 (February 2, 2026)
**Authors:** Zhanghao Hu, Qinglin Zhu, Hanqi Yan, Yulan He, Lin Gui
**Venue:** ICML 2026
**Project page:** https://zhanghao-xmemory.github.io/Academic-project-page-template/

---

## Executive Summary

xMemory argues that standard RAG (embed-retrieve-top-k) is fundamentally mismatched for agent memory. RAG assumes a large, heterogeneous corpus where retrieved passages are diverse; agent memory is a bounded, coherent dialogue stream where candidate spans are highly correlated and often near-duplicates. Fixed top-k similarity retrieval collapses into redundant clusters, and post-hoc pruning breaks temporally linked evidence chains.

xMemory's solution: **decouple** the memory stream into semantic components organized in a four-level hierarchy (messages -> episodes -> semantics -> themes), then **aggregate** by retrieving top-down over this structure rather than by similarity ranking over raw chunks. A sparsity-semantics guidance objective governs theme split/merge during construction. At retrieval, it selects diverse theme/semantic representatives first, then expands to episodes/messages only when they reduce reader uncertainty.

Results: consistent gains in answer quality (BLEU, F1) and token efficiency across LoCoMo and PerLTQA benchmarks with three LLM backbones (Qwen3-8B, Llama-3.1-8B, GPT-5 nano). Covers evidence with ~half the tokens of naive RAG.

This paper is directly relevant to Meridian's L2 memory retrieval layer -- it provides a concrete, empirically validated alternative to flat vector similarity search for agent memories.

---

## Paper Summary

### The Problem: RAG Doesn't Fit Agent Memory

Standard RAG was designed for large heterogeneous corpora (Wikipedia, web pages) where:
- Retrieved passages are diverse
- The main failure mode is irrelevance
- Redundancy is not a primary concern

Agent memory is fundamentally different:
- **Bounded and coherent**: All memories come from the same agent's history
- **Highly correlated**: Many spans are near-duplicates or variants of the same information
- **Temporally entangled**: Key evidence relies on neighboring turns via co-reference, ellipsis, and timeline dependencies

Under these conditions:
1. **Fixed top-k collapses**: Similarity retrieval returns redundant spans from a single dense region, missing diverse evidence needed for multi-fact queries
2. **Pruning breaks evidence chains**: Post-hoc compression (e.g., LLMLingua-2) uses local relevance cues developed for RAG, which can delete temporal prerequisites within an agent's evidence chain

### The xMemory Architecture

#### Four-Level Hierarchy (Section 3.2)

```
Theme          (abstract groupings of related semantics)
  └── Semantic   (reusable long-term facts distilled from episodes)
        └── Episode    (summary of a contiguous message block)
              └── Original Message  (raw dialogue turns)
```

**Mapping rules:**
- Each message block maps to one episode
- Each episode can yield multiple semantic nodes
- Each semantic node belongs to exactly one theme
- Each theme contains multiple semantic nodes

The key insight: **explicitly separate episodic traces from reusable semantic components**. Two episodes about "going to the gym" might share the semantic fact "user exercises regularly" -- this fact is extracted once as a semantic node rather than stored redundantly in both episodes.

#### Guidance Objective for Theme Organization (Section 3.2)

Themes are maintained via a combined sparsity + semantics score:

```
f(P) = SparsityScore(P) + SemScore(P)
```

**Sparsity Score**: Measures how balanced theme sizes are. Penalizes oversized themes (which cause retrieval to concentrate in dense neighborhoods) and undersized themes (which fragment evidence coverage).

```
SparsityScore(P) = N² / (K * Σ n_k²)
```

**Semantic Score**: Combines intra-theme cohesion (cosine similarity to centroid) with a bell-shaped regularizer on inter-theme nearest-neighbor similarity. Penalizes both:
- Overly similar themes (redundancy)
- Overly dissimilar themes ("semantic islands" that hinder cross-topic navigation)

**Operations**: When a new semantic node arrives:
1. **Attach** to most similar theme by centroid similarity (or create new theme)
2. **Split** overcrowded themes by clustering semantic nodes, choosing the split that maximizes f(P)
3. **Merge** tiny themes into nearest-neighbor themes, choosing the merge that maximizes f(P)

**kNN graph**: A k-nearest-neighbor graph among theme and semantic nodes provides efficient navigation without quadratic maintenance.

#### Two-Stage Adaptive Retrieval (Section 3.3)

**Stage I: Query-Aware Representative Selection on kNN Graphs**

At the theme and semantic levels, answers often require *set-level* evidence (multiple complementary facts, multi-hop reasoning chains). Instead of returning the top-k most similar nodes, xMemory selects representatives via a greedy procedure that trades off:
- **Coverage gain**: How many new graph neighbors does this node cover?
- **Query relevance**: How similar is this node to the query?

```
i* = argmax α * (coverage_gain / Z) + (1-α) * similarity(q, i)
```

Applied first on theme candidates, then on induced semantic candidates. Stops when coverage ratio hits a threshold or budget is exhausted.

**Stage II: Uncertainty-Adaptive Evidence Inclusion**

From selected semantics, gather linked episodes as intact evidence units. Build a coarse context from selected themes + semantic summaries. Then:
1. Include episodes only when they **sufficiently reduce the reader's predictive uncertainty** (measured via output logits/entropy)
2. Optionally expand to original messages when further uncertainty reduction justifies the added tokens
3. **Early stopping** when additional episodes fail to improve certainty

This controls redundancy while preserving temporally linked evidence within intact units.

### Key Results

#### LoCoMo Benchmark (Table 1)

| Model | Method | Avg BLEU | Avg F1 | Tokens/query |
|---|---|---|---|---|
| Qwen3-8B | Naive RAG | 27.92 | 36.42 | 6613 |
| Qwen3-8B | A-Mem | 19.49 | 21.78 | 9103 |
| Qwen3-8B | MemoryOS | 29.20 | 33.76 | 7235 |
| Qwen3-8B | **xMemory** | **34.48** | **43.98** | **4711** |
| GPT-5 nano | Nemori | 36.65 | 48.17 | 9155 |
| GPT-5 nano | **xMemory** | **38.71** | **50.00** | **6581** |

Best accuracy AND lowest token usage simultaneously.

#### Evidence Coverage Efficiency (Table 3)

| Method | Avg blocks for coverage | Avg tokens for coverage |
|---|---|---|
| Naive RAG | 10.81 | 1979 |
| RAG + Pruning | 13.31 | 1588 |
| **xMemory** | **5.66** | **975** |

xMemory covers evidence with ~half the blocks and ~half the tokens.

#### Ablation (Figure 3)

- Hierarchical memory alone (without adaptive retrieval) already yields large gains over naive RAG (BLEU 27.92 -> 31.81)
- Stage I (representative selection) adds diversity and reduces tokens
- Stage II (uncertainty gating) adds precision and further reduces tokens
- Both stages are complementary

#### Retroactive Restructuring (Figure 5)

- 44.91% of semantic nodes get reassigned to different themes during construction
- Disabling split/merge freezes structure and reduces F1 from 43.98 to 38.59
- Split is the primary source of structural revision; merge consolidates redundant themes into compact organization (1122 themes without merge -> 642 with merge)

---

## Relation to Meridian

### Where xMemory's Insights Apply

Meridian's L2 memory layer currently uses flat vector similarity search (sqlite-vec) to retrieve detailed findings stored across agent lifecycles. This is exactly the scenario xMemory identifies as problematic:

1. **Bounded, coherent source**: All L2 memories come from agents working on related objectives in the same codebase. They are inherently correlated.
2. **Near-duplicate accumulation**: Across multiple context reset cycles, agents may store overlapping observations about the same code, the same errors, the same architectural patterns.
3. **Multi-fact queries**: When a fresh agent searches for context ("what do we know about the authentication module?"), the answer typically spans multiple related but distinct findings -- exactly where top-k similarity collapses.

### What Meridian Already Has That Aligns

| xMemory Concept | Meridian Equivalent | Gap |
|---|---|---|
| Four-level hierarchy | L0 (objectives) / L1 (summary) / L2 (detailed) | Meridian's layers are lifecycle-based, not semantic. L2 entries are flat. |
| Semantic deduplication | `novelty_threshold = 0.95` | Meridian filters duplicates but doesn't organize non-duplicate entries into semantic components |
| Memory linking | `memory_links` table (similarity-based auto-linking) | Exists but not used for retrieval navigation -- only stored |
| Theme organization | None | No higher-level grouping of L2 memories |
| Guidance objective for structure | None | No optimization of memory organization during construction |
| Uncertainty-gated retrieval | None | `search_graph()` returns top-k by similarity |
| Retroactive restructuring | None | Memory links are computed on insert, not revised |

### Concrete Adoption Opportunities

**1. Hierarchical L2 Organization**

Instead of flat L2 entries with vector search, organize L2 memories into a hierarchy:
- **L2-raw**: Original detailed findings (current L2 entries)
- **L2-semantic**: Distilled reusable facts extracted from raw findings (e.g., "the auth module uses JWT with 24h expiry" extracted from a verbose debugging session)
- **L2-theme**: Grouped semantic nodes (e.g., "authentication", "database schema", "API design")

This maps directly to xMemory's message -> episode -> semantic -> theme hierarchy but adapted for code agent findings rather than dialogue turns.

**2. Sparsity-Semantics Guided Memory Organization**

When `store_memory` is called, instead of just computing embeddings and checking novelty:
1. Extract semantic facts from the raw finding
2. Assign to existing themes by centroid similarity
3. Periodically split/merge themes using the guidance objective
4. Maintain a kNN graph among themes and semantic nodes

This could run as a background maintenance task after each memory insertion, or periodically between agent resets.

**3. Two-Stage Retrieval for `search_graph()`**

Replace flat top-k vector search with:
- **Stage I**: Select diverse, query-relevant themes and semantic nodes using the greedy representative selection on the kNN graph
- **Stage II**: Expand to raw L2 entries only when they add information beyond what the semantic summaries provide

This would produce shorter, more evidence-dense context for fresh agents after a reset -- critical when the L0+L1 checkpoint already consumes a significant portion of the fresh context window.

**4. Retroactive Restructuring on Reset**

After each context reset cycle, the newly stored memories may change the overall memory landscape. Run restructuring (theme split/merge, semantic reassignment) as a post-reset maintenance step. The 44.91% reassignment ratio from the paper suggests this is not a marginal effect -- nearly half the memory organization changes as new information arrives.

**5. Uncertainty-Gated Memory Expansion**

For the `search_graph()` MCP tool, instead of returning a fixed number of results:
- Return theme + semantic summaries first (compact)
- Let the agent request expansion to raw entries only if the summaries are insufficient
- Or: implement this server-side by measuring whether adding more raw entries decreases embedding-space uncertainty relative to the query

### What NOT to Adopt

**1. LLM-based semantic extraction during memory construction.**
xMemory uses LLM calls to generate episode summaries, extract semantic nodes, and create themes. In Meridian's context, this would mean additional LLM API calls during `store_memory`, which conflicts with the goal of keeping the MCP server independent of the LLM. Consider using the local embedding model (fastembed) + clustering for theme management, and defer semantic extraction to the agent itself (let the agent store pre-distilled findings rather than raw dumps).

**2. Output logit-based uncertainty gating.**
Stage II uses reader LLM logits to decide when to expand evidence. This requires access to output probabilities, which Claude's API may not expose. Use embedding-space similarity as a proxy for uncertainty, or let the agent self-report whether retrieved context is sufficient.

**3. Full mathematical optimization framework.**
The Fano-inequality-based theoretical analysis is interesting but the practical guidance is simple: keep themes balanced (not too large, not too small), keep them semantically coherent (not too similar, not too dissimilar). This can be implemented with straightforward heuristics rather than the full optimization framework.

---

## Architectural Implications for Meridian

### High-Value, Low-Effort Changes

1. **Hierarchical tags on L2 entries**: Add a `theme` field to L2 memories. When storing, assign to nearest existing theme (by centroid similarity). Use themes as a pre-filter before vector search. This alone avoids the "collapsed retrieval" problem by ensuring results span multiple themes.

2. **Use the existing `memory_links` table for navigation**: The kNN graph in xMemory is functionally identical to Meridian's `memory_links` table with `similarity_score`. Currently these links are stored but not used in retrieval. Use them: when retrieving L2 memories, traverse linked memories to broaden coverage beyond the query's embedding neighborhood.

3. **Budget-aware `search_graph()`**: Instead of returning top-k results, return results until a token budget is met. Prioritize diversity (one result per theme) before depth (multiple results from the same theme).

### Medium-Effort, High-Value Changes

4. **Semantic node extraction**: When an agent calls `store_memory`, extract reusable factual claims as separate semantic nodes (shorter, more precise). Store both the raw finding and the extracted semantics. Retrieve at the semantic level first, expand to raw only if needed.

5. **Theme split/merge maintenance**: After each agent reset, run a background restructuring pass: recompute theme centroids, split oversized themes, merge near-duplicate themes. Use the sparsity-semantics score as a simple quality metric.

### Lower Priority

6. **Retroactive reassignment**: Periodically reassign semantic nodes to themes when the guidance score improves. The paper shows this has significant impact (44.91% reassignment, +5 F1 points) but requires more complex bookkeeping.

7. **Uncertainty-adaptive expansion**: Implement server-side expansion gating in `search_graph()` using embedding uncertainty. Valuable but requires careful tuning and may not be needed for MVP.

---

## Key Takeaways

1. **Flat vector similarity search is provably suboptimal for agent memory** -- the bounded, correlated nature of agent memories means top-k will return redundant spans. Meridian's current `search_graph()` has this problem.

2. **Structure during construction is more important than filtering during retrieval** -- the hierarchical memory alone (without adaptive retrieval) provides most of the gain over naive RAG. Getting the memory organization right matters more than sophisticated query-time logic.

3. **Theme-level organization is the highest-leverage change** -- it prevents retrieval collapse and enables diversity without complex retrieval algorithms. Adding a simple `theme` grouping to L2 memories would capture much of the benefit.

4. **Memory organization must be dynamic** -- 44.91% of nodes get reassigned as new memories arrive. Static organization degrades quickly. Meridian should plan for periodic restructuring, not just insert-time organization.

5. **Token efficiency and accuracy are not in tension** -- xMemory gets better answers with fewer tokens by being more precise about what to retrieve. This directly benefits Meridian, where every token in the fresh agent's context is precious.

**Relevance to Meridian: HIGH** -- directly addresses the quality of L2 memory retrieval, which is a core mechanism for cross-reset knowledge transfer. The hierarchical organization and diversity-aware retrieval are concrete improvements over flat vector search that Meridian should incorporate.
