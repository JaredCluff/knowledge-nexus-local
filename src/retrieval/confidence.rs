//! Confidence Scorer - classify search results as High/Medium/Low
//! based on vector distance and keyword overlap.

use crate::k2k::models::K2KResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceLevel {
    High,
    Medium,
    Low,
}

pub struct ConfidenceScorer {
    /// Minimum confidence score to be considered High
    high_threshold: f32,
    /// Minimum confidence score to be considered Medium
    medium_threshold: f32,
    /// Minimum keyword overlap ratio for a boost
    keyword_boost_threshold: f32,
}

impl ConfidenceScorer {
    pub fn new() -> Self {
        Self {
            high_threshold: 0.7,
            medium_threshold: 0.4,
            keyword_boost_threshold: 0.3,
        }
    }

    /// Classify a result's confidence level
    pub fn classify(&self, result: &K2KResult, query: &str) -> ConfidenceLevel {
        let base_score = result.confidence;
        let keyword_overlap = self.keyword_overlap(query, &result.content);

        // Boost score based on keyword overlap
        let adjusted_score = if keyword_overlap >= self.keyword_boost_threshold {
            base_score + (keyword_overlap * 0.15)
        } else {
            base_score
        };

        if adjusted_score >= self.high_threshold {
            ConfidenceLevel::High
        } else if adjusted_score >= self.medium_threshold {
            ConfidenceLevel::Medium
        } else {
            ConfidenceLevel::Low
        }
    }

    /// Score all results and return (result, level) pairs
    #[allow(dead_code)]
    pub fn score_all<'a>(
        &self,
        results: &'a [K2KResult],
        query: &str,
    ) -> Vec<(&'a K2KResult, ConfidenceLevel)> {
        results
            .iter()
            .map(|r| (r, self.classify(r, query)))
            .collect()
    }

    /// Check if all results are low confidence
    pub fn all_low_confidence(&self, results: &[K2KResult], query: &str) -> bool {
        results
            .iter()
            .all(|r| self.classify(r, query) == ConfidenceLevel::Low)
    }

    /// Calculate keyword overlap ratio between query and content
    fn keyword_overlap(&self, query: &str, content: &str) -> f32 {
        let query_words: Vec<String> = query
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .filter(|w| w.len() > 2) // Skip short words
            .collect();

        if query_words.is_empty() {
            return 0.0;
        }

        let content_lower = content.to_lowercase();
        let matches = query_words
            .iter()
            .filter(|w| content_lower.contains(w.as_str()))
            .count();

        matches as f32 / query_words.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::k2k::models::K2KResult;

    fn make_result(confidence: f32, content: &str) -> K2KResult {
        K2KResult {
            article_id: "test".into(),
            store_id: "s1".into(),
            title: "Test".into(),
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
    fn test_high_confidence() {
        let scorer = ConfidenceScorer::new();
        let result = make_result(0.85, "rust programming language guide");
        assert_eq!(
            scorer.classify(&result, "rust programming"),
            ConfidenceLevel::High
        );
    }

    #[test]
    fn test_medium_confidence() {
        let scorer = ConfidenceScorer::new();
        let result = make_result(0.5, "some programming text");
        assert_eq!(
            scorer.classify(&result, "rust programming"),
            ConfidenceLevel::Medium
        );
    }

    #[test]
    fn test_low_confidence() {
        let scorer = ConfidenceScorer::new();
        let result = make_result(0.2, "unrelated content about cats");
        assert_eq!(
            scorer.classify(&result, "rust programming"),
            ConfidenceLevel::Low
        );
    }

    #[test]
    fn test_keyword_boost() {
        let scorer = ConfidenceScorer::new();
        // Score is below high threshold but keyword match should boost it
        let result = make_result(0.6, "rust programming language reference guide");
        let level = scorer.classify(&result, "rust programming language");
        // All 3 query words match, overlap = 1.0, boost = 0.15 → 0.75 → High
        assert_eq!(level, ConfidenceLevel::High);
    }

    #[test]
    fn test_all_low() {
        let scorer = ConfidenceScorer::new();
        let results = vec![
            make_result(0.1, "cats and dogs"),
            make_result(0.2, "weather forecast"),
        ];
        assert!(scorer.all_low_confidence(&results, "rust programming"));
    }
}
