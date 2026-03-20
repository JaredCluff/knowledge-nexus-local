//! Reranker - boosts results based on exact phrase matches, title matches, and recency.

use crate::k2k::models::K2KResult;

pub struct Reranker;

impl Reranker {
    pub fn new() -> Self {
        Self
    }

    /// Rerank results by applying boosting signals
    pub fn rerank(&self, results: Vec<K2KResult>, query: &str) -> Vec<K2KResult> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<String> = query_lower
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .map(|w| w.to_string())
            .collect();

        let mut scored: Vec<(f32, K2KResult)> = results
            .into_iter()
            .map(|result| {
                let mut boost = 0.0f32;

                // Exact phrase match in content
                if result.content.to_lowercase().contains(&query_lower) {
                    boost += 0.15;
                }

                // Title match
                let title_lower = result.title.to_lowercase();
                if title_lower.contains(&query_lower) {
                    boost += 0.2;
                } else {
                    // Partial title keyword match
                    let title_matches = query_words
                        .iter()
                        .filter(|w| title_lower.contains(w.as_str()))
                        .count();
                    if !query_words.is_empty() {
                        boost += 0.1 * (title_matches as f32 / query_words.len() as f32);
                    }
                }

                // Recency boost - parse modified_at from metadata
                if let Some(modified_at) =
                    result.metadata.get("modified_at").and_then(|v| v.as_str())
                {
                    if let Ok(modified) = chrono::DateTime::parse_from_rfc3339(modified_at) {
                        let age_days =
                            (chrono::Utc::now() - modified.with_timezone(&chrono::Utc)).num_days();
                        // Recent documents (< 7 days) get a small boost
                        if age_days < 7 {
                            boost += 0.05;
                        } else if age_days < 30 {
                            boost += 0.02;
                        }
                    }
                }

                let final_score = result.confidence + boost;
                (final_score, result)
            })
            .collect();

        // Sort by boosted score
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        scored
            .into_iter()
            .map(|(score, mut result)| {
                result.confidence = score;
                result
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(id: &str, title: &str, content: &str, confidence: f32) -> K2KResult {
        K2KResult {
            article_id: id.into(),
            store_id: "s1".into(),
            title: title.into(),
            summary: String::new(),
            content: content.into(),
            confidence,
            source_type: "local".into(),
            tags: vec![],
            metadata: serde_json::json!({}),
            provenance: None,
        }
    }

    #[test]
    fn test_title_match_boost() {
        let reranker = Reranker::new();
        let results = vec![
            make_result("a1", "unrelated title", "rust programming guide", 0.5),
            make_result("a2", "rust programming", "some content about rust", 0.5),
        ];
        let reranked = reranker.rerank(results, "rust programming");
        // a2 has title match, should be first
        assert_eq!(reranked[0].article_id, "a2");
        assert!(reranked[0].confidence > reranked[1].confidence);
    }

    #[test]
    fn test_exact_phrase_boost() {
        let reranker = Reranker::new();
        let results = vec![
            make_result("a1", "guide", "learn to code with rust", 0.5),
            make_result(
                "a2",
                "guide",
                "rust programming is a guide to rust programming",
                0.5,
            ),
        ];
        let reranked = reranker.rerank(results, "rust programming");
        // a2 has exact phrase "rust programming" in content, should rank higher
        assert_eq!(reranked[0].article_id, "a2");
    }

    #[test]
    fn test_preserves_relative_order() {
        let reranker = Reranker::new();
        let results = vec![
            make_result("a1", "doc", "content", 0.9),
            make_result("a2", "doc", "content", 0.3),
        ];
        let reranked = reranker.rerank(results, "unrelated query");
        // No boost for either, original order preserved
        assert_eq!(reranked[0].article_id, "a1");
    }
}
