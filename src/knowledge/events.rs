//! Event segmentation: LLM-based with heuristic fallback.
//!
//! Following CompassMem's Two-Step Alignment (mirrored from Nemori,
//! arXiv 2508.03341): an LLM identifies coherent event boundaries in a
//! stream of articles/messages and extracts structured event records.
//!
//! When the LLM is unavailable (no Ollama, opt-out, or repeated parse
//! failures), the segmenter falls back to a heuristic: split when
//! consecutive articles share fewer than N entities (topic shift) OR
//! when there's a long gap between created_at timestamps (silence).

use anyhow::Result;
use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::config::ExtractionConfig;
use crate::store::Article;

/// Structured event record produced by the segmenter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEvent {
    pub title: String,
    pub summary: String,
    pub started_at: String,
    pub ended_at: String,
    #[serde(default)]
    pub participants: Vec<String>,
    /// Article IDs that constitute the evidence for this event.
    #[serde(default)]
    pub evidence_article_ids: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_confidence() -> f64 {
    0.5
}

/// LLM-based event segmenter mirroring EntityExtractor's HTTP pattern.
pub struct EventSegmenter {
    config: ExtractionConfig,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    format: String,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

/// LLM response shape: `{"events": [ExtractedEvent, ...]}`.
#[derive(Deserialize)]
struct SegmentationResult {
    events: Vec<ExtractedEvent>,
}

impl EventSegmenter {
    pub fn new(config: ExtractionConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .expect("build reqwest client");
        Self { config, client }
    }

    /// Segment a batch of articles into events via the LLM. Falls back to
    /// the heuristic when the LLM is disabled or returns unparseable output.
    pub async fn segment(&self, articles: &[Article]) -> Result<Vec<ExtractedEvent>> {
        if !self.config.enabled || articles.is_empty() {
            return Ok(Self::heuristic_segment(articles));
        }

        let prompt = Self::build_prompt(articles);
        let url = format!("{}/api/generate", self.config.ollama_url);
        let req = OllamaRequest {
            model: self.config.model.clone(),
            prompt,
            format: "json".into(),
            stream: false,
        };

        match self.client.post(&url).json(&req).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<OllamaResponse>().await {
                Ok(body) => match Self::parse_response(&body.response) {
                    Ok(events) => Ok(events),
                    Err(e) => {
                        tracing::warn!(
                            "EventSegmenter LLM returned unparseable JSON; falling back to heuristic: {}",
                            e
                        );
                        Ok(Self::heuristic_segment(articles))
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "EventSegmenter LLM body parse failed; falling back to heuristic: {}",
                        e
                    );
                    Ok(Self::heuristic_segment(articles))
                }
            },
            Ok(resp) => {
                tracing::warn!(
                    "EventSegmenter LLM HTTP {}; falling back to heuristic",
                    resp.status()
                );
                Ok(Self::heuristic_segment(articles))
            }
            Err(e) => {
                tracing::warn!(
                    "EventSegmenter LLM unreachable ({}); falling back to heuristic",
                    e
                );
                Ok(Self::heuristic_segment(articles))
            }
        }
    }

    fn build_prompt(articles: &[Article]) -> String {
        let mut batch = String::new();
        for (i, a) in articles.iter().enumerate() {
            batch.push_str(&format!(
                "[{}] ts={} id={} title={:?}\nContent: {}\n\n",
                i, a.created_at, a.id, a.title, a.content
            ));
        }
        format!(
            r#"You segment a stream of articles into coherent EVENTS.

An EVENT is a time-bounded experience (a trip, an incident, a project milestone, a conversation thread) — NOT every individual article. Group consecutive articles that describe a single coherent experience together.

For each event you identify, output a JSON object with:
- "title": short label (<= 8 words)
- "summary": one-paragraph summary
- "started_at": ISO-8601 timestamp of the earliest article in the event
- "ended_at": ISO-8601 timestamp of the latest article in the event
- "participants": array of named people/agents in the event (may be empty)
- "evidence_article_ids": array of article IDs constituting this event
- "confidence": float in [0.0, 1.0]

Return a JSON object with a single key "events" containing an array. Only output the JSON, no prose.

Articles:
{batch}"#,
            batch = batch.trim_end()
        )
    }

    fn parse_response(json_str: &str) -> Result<Vec<ExtractedEvent>> {
        if let Ok(result) = serde_json::from_str::<SegmentationResult>(json_str) {
            return Ok(Self::filter_valid(result.events));
        }
        if let Ok(events) = serde_json::from_str::<Vec<ExtractedEvent>>(json_str) {
            return Ok(Self::filter_valid(events));
        }
        anyhow::bail!("Failed to parse Ollama response as event JSON: {}", json_str)
    }

    fn filter_valid(events: Vec<ExtractedEvent>) -> Vec<ExtractedEvent> {
        events
            .into_iter()
            .filter(|e| !e.title.trim().is_empty() && e.confidence >= 0.0 && e.confidence <= 1.0)
            .collect()
    }

    /// Heuristic segmentation: split where consecutive articles have a long
    /// silence gap (>4h) OR very different tag sets. Coarse but deterministic;
    /// schema-compatible with the LLM output so a later LLM pass can refine.
    pub(crate) fn heuristic_segment(articles: &[Article]) -> Vec<ExtractedEvent> {
        if articles.is_empty() {
            return vec![];
        }

        // Articles must be in chronological order; sort defensively
        let mut sorted: Vec<&Article> = articles.iter().collect();
        sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        let mut events = Vec::new();
        let mut current_group: Vec<&Article> = vec![sorted[0]];

        for i in 1..sorted.len() {
            let prev = sorted[i - 1];
            let curr = sorted[i];

            // Compute time gap (hours) between consecutive articles
            let gap_hours =
                Self::time_gap_hours(&prev.created_at, &curr.created_at).unwrap_or(0.0);

            // Heuristic split conditions:
            // - Silence gap > 4 hours, OR
            // - Tag sets are completely disjoint (proxy for topic shift)
            let topic_shift = Self::tags_disjoint(prev, curr);
            if gap_hours > 4.0 || topic_shift {
                events.push(Self::group_to_event(&current_group));
                current_group = vec![curr];
            } else {
                current_group.push(curr);
            }
        }

        if !current_group.is_empty() {
            events.push(Self::group_to_event(&current_group));
        }

        events
    }

    fn time_gap_hours(t1: &str, t2: &str) -> Option<f64> {
        let p1 = DateTime::parse_from_rfc3339(t1).ok()?;
        let p2 = DateTime::parse_from_rfc3339(t2).ok()?;
        Some((p2 - p1).num_minutes() as f64 / 60.0)
    }

    fn tags_disjoint(a: &Article, b: &Article) -> bool {
        let tags_a = Self::tags_set(a);
        let tags_b = Self::tags_set(b);
        if tags_a.is_empty() || tags_b.is_empty() {
            // No tags on one side → can't infer topic shift; assume no shift.
            return false;
        }
        tags_a.is_disjoint(&tags_b)
    }

    fn tags_set(a: &Article) -> std::collections::HashSet<String> {
        a.tags
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn group_to_event(group: &[&Article]) -> ExtractedEvent {
        let first = group[0];
        let last = group[group.len() - 1];
        let title = if first.title.len() <= 60 {
            first.title.clone()
        } else {
            format!("{}...", &first.title[..60])
        };
        let summary = format!(
            "{} article(s) between {} and {}",
            group.len(),
            first.created_at,
            last.created_at
        );
        ExtractedEvent {
            title,
            summary,
            started_at: first.created_at.clone(),
            ended_at: last.created_at.clone(),
            participants: vec![],
            evidence_article_ids: group.iter().map(|a| a.id.clone()).collect(),
            // Heuristic confidence is low — flags this as cheap output that
            // a later LLM pass should refine.
            confidence: 0.3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_article(
        id: &str,
        title: &str,
        created_at: &str,
        tags: serde_json::Value,
    ) -> Article {
        Article {
            id: id.into(),
            store_id: "s1".into(),
            title: title.into(),
            content: "content".into(),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: format!("{}-h", id),
            tags,
            embedded_at: None,
            created_at: created_at.into(),
            updated_at: created_at.into(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: crate::store::Tier::Hot,
            pinned: false,
            compacted_into: None,
        }
    }

    #[test]
    fn parse_response_wrapped_array() {
        let json = r#"{"events": [
            {"title":"Trip","summary":"AZ","started_at":"2026-03-15T00:00:00Z","ended_at":"2026-03-20T00:00:00Z","participants":["alice"],"evidence_article_ids":["a1","a2"],"confidence":0.85}
        ]}"#;
        let events = EventSegmenter::parse_response(json).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Trip");
        assert_eq!(events[0].participants, vec!["alice"]);
    }

    #[test]
    fn parse_response_bare_array() {
        let json = r#"[
            {"title":"Meeting","summary":"sprint planning","started_at":"2026-05-01T09:00:00Z","ended_at":"2026-05-01T10:00:00Z","participants":[],"evidence_article_ids":["a1"],"confidence":0.7}
        ]"#;
        let events = EventSegmenter::parse_response(json).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Meeting");
    }

    #[test]
    fn parse_response_filters_empty_titles() {
        let json = r#"{"events": [
            {"title":"","summary":"empty","started_at":"2026-05-01T00:00:00Z","ended_at":"2026-05-01T00:00:00Z","participants":[],"evidence_article_ids":[],"confidence":0.5},
            {"title":"Valid","summary":"valid","started_at":"2026-05-01T00:00:00Z","ended_at":"2026-05-01T00:00:00Z","participants":[],"evidence_article_ids":[],"confidence":0.6}
        ]}"#;
        let events = EventSegmenter::parse_response(json).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Valid");
    }

    #[test]
    fn parse_response_errors_on_bad_json() {
        let result = EventSegmenter::parse_response("totally not json");
        assert!(result.is_err());
    }

    #[test]
    fn heuristic_segment_splits_on_silence_gap() {
        // Three articles: a and b same hour; c 5 hours later
        let articles = vec![
            make_article(
                "a",
                "Morning",
                "2026-05-01T09:00:00Z",
                serde_json::json!(["work"]),
            ),
            make_article(
                "b",
                "Morning followup",
                "2026-05-01T10:00:00Z",
                serde_json::json!(["work"]),
            ),
            make_article(
                "c",
                "Afternoon",
                "2026-05-01T15:00:00Z",
                serde_json::json!(["work"]),
            ),
        ];

        let events = EventSegmenter::heuristic_segment(&articles);
        assert_eq!(events.len(), 2, "5-hour gap should split into 2 events");
        assert_eq!(events[0].evidence_article_ids, vec!["a", "b"]);
        assert_eq!(events[1].evidence_article_ids, vec!["c"]);
    }
}
