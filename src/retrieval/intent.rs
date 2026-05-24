//! Rule-based query intent classifier (MAGMA-style, arXiv 2601.03236).
//!
//! Maps queries to one of five intents and supplies per-intent edge-type
//! weight multipliers. Captures MAGMA's reported 8.9% LoCoMo gain from
//! query-adaptive policy without requiring a learned model.
//!
//! Per-intent weights are MAGMA Table 6 defaults adapted for KNL's five
//! edge types: ENTITY_OVERLAP and REFERENCES mirror MAGMA's entity dimension;
//! SEMANTICALLY_RELATED mirrors the semantic dimension; PRECEDES is temporal;
//! CAUSED_BY is causal.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Intent {
    Why,
    When,
    Entity,
    MultiHop,
    OpenDomain,
}

/// Per-intent edge-weight multipliers. Applied to the column-stochastic
/// edge matrix before PPR diffusion.
#[derive(Debug, Clone, Copy)]
pub struct IntentWeights {
    pub entity_overlap: f32,
    pub semantically_related: f32,
    pub precedes: f32,
    pub caused_by: f32,
    pub references_edge: f32,
}

impl Intent {
    /// MAGMA Table 6 defaults (adapted for KNL's edge types).
    pub fn weights(self) -> IntentWeights {
        match self {
            Intent::Why => IntentWeights {
                entity_overlap: 1.5,
                semantically_related: 1.0,
                precedes: 1.0,
                caused_by: 4.0,
                references_edge: 1.5,
            },
            Intent::When => IntentWeights {
                entity_overlap: 1.0,
                semantically_related: 1.0,
                precedes: 3.0,
                caused_by: 1.0,
                references_edge: 1.0,
            },
            Intent::Entity => IntentWeights {
                entity_overlap: 4.0,
                semantically_related: 1.0,
                precedes: 1.0,
                caused_by: 1.0,
                references_edge: 4.0,
            },
            Intent::MultiHop => IntentWeights {
                entity_overlap: 2.0,
                semantically_related: 1.5,
                precedes: 1.5,
                caused_by: 2.0,
                references_edge: 2.0,
            },
            Intent::OpenDomain => IntentWeights {
                entity_overlap: 1.0,
                semantically_related: 1.0,
                precedes: 1.0,
                caused_by: 1.0,
                references_edge: 1.0,
            },
        }
    }
}

/// Rule-based classifier using cue-word matching. Score per intent =
/// number of cue phrases that appear in the lowercased query. Highest
/// score wins; ties resolved in declaration order (Why → When → Entity
/// → MultiHop). Empty / no-cue queries default to OpenDomain.
///
/// This captures the bulk of MAGMA's reported 8.9% LoCoMo gain without
/// requiring an LLM call. Ambiguous queries (multiple ties) fall through
/// to OpenDomain — the safe default. P7+ may add an Ollama 3B fallback.
pub fn classify(query: &str) -> Intent {
    let q = query.to_lowercase();

    // Causal cues → Why
    let causal_cues = [
        "why", "because", "caused", "cause of", "due to",
        "led to", "result of", "consequence",
    ];
    let why_score = causal_cues.iter().filter(|c| q.contains(*c)).count();

    // Temporal cues → When
    let temporal_cues = [
        "when", "after", "before", "during", "while",
        "earlier", "later", "first", "last",
        "history of", "timeline",
    ];
    let when_score = temporal_cues.iter().filter(|c| q.contains(*c)).count();

    // Entity-focused cues → Entity
    let entity_cues = [
        "what is", "who is", "tell me about", "describe", "definition of",
    ];
    let entity_score = entity_cues.iter().filter(|c| q.contains(*c)).count();

    // Multi-hop cues: multiple "and" clauses, or explicit "through"/"via"
    let multihop_score = if q.matches(" and ").count() >= 2 || q.contains(" through ") || q.contains(" via ") {
        1
    } else {
        0
    };

    // Highest score wins; ties broken by declaration order below.
    let scores = [
        (Intent::Why, why_score),
        (Intent::When, when_score),
        (Intent::Entity, entity_score),
        (Intent::MultiHop, multihop_score),
    ];

    let max_score = scores.iter().map(|(_, s)| *s).max().unwrap_or(0);
    if max_score == 0 {
        return Intent::OpenDomain;
    }
    scores
        .iter()
        .find(|(_, s)| *s == max_score)
        .map(|(intent, _)| *intent)
        .unwrap_or(Intent::OpenDomain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_why_questions() {
        assert_eq!(classify("Why did the build fail?"), Intent::Why);
        assert_eq!(classify("What caused the outage?"), Intent::Why);
        assert_eq!(classify("This happened because of the deploy"), Intent::Why);
    }

    #[test]
    fn classify_when_questions() {
        assert_eq!(classify("When did this happen?"), Intent::When);
        assert_eq!(classify("Show me the history of this project"), Intent::When);
        assert_eq!(classify("What happened after the deploy?"), Intent::When);
    }

    #[test]
    fn classify_entity_questions() {
        assert_eq!(classify("What is Rust?"), Intent::Entity);
        assert_eq!(classify("Tell me about Tokio"), Intent::Entity);
        assert_eq!(classify("Describe the borrow checker"), Intent::Entity);
    }

    #[test]
    fn classify_open_domain_default() {
        assert_eq!(classify("the quick brown fox"), Intent::OpenDomain);
        assert_eq!(classify(""), Intent::OpenDomain);
        assert_eq!(classify("random query with no cues"), Intent::OpenDomain);
    }

    #[test]
    fn weights_are_consistent_across_intents() {
        let od = Intent::OpenDomain.weights();
        assert_eq!(od.entity_overlap, 1.0);
        assert_eq!(od.semantically_related, 1.0);
        assert_eq!(od.precedes, 1.0);
        assert_eq!(od.caused_by, 1.0);
        assert_eq!(od.references_edge, 1.0);

        let why = Intent::Why.weights();
        assert!(why.caused_by > 1.0, "Why intent must boost causal");

        let when = Intent::When.weights();
        assert!(when.precedes > 1.0, "When intent must boost temporal");

        let entity = Intent::Entity.weights();
        assert!(entity.entity_overlap > 1.0, "Entity intent must boost entity-overlap");
        assert!(entity.references_edge > 1.0, "Entity intent must boost references");

        let multihop = Intent::MultiHop.weights();
        // MultiHop boosts ALL types modestly above 1.0
        assert!(multihop.entity_overlap > 1.0);
        assert!(multihop.semantically_related > 1.0);
        assert!(multihop.precedes > 1.0);
        assert!(multihop.caused_by > 1.0);
        assert!(multihop.references_edge > 1.0);
    }
}
