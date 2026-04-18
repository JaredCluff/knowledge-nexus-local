//! LLM-based entity extraction via local Ollama.
//!
//! Calls the Ollama `/api/generate` endpoint with a structured prompt that
//! constrains the model to return a JSON array of entities. Each entity has
//! a name, type, description, confidence, and excerpt.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ExtractionConfig;

/// A single entity extracted from article text by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    pub entity_type: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub excerpt: Option<String>,
}

fn default_confidence() -> f64 {
    0.5
}

/// Performs entity extraction against a local Ollama instance.
pub struct EntityExtractor {
    config: ExtractionConfig,
    client: reqwest::Client,
}

/// Generate a deterministic entity ID from type and name.
///
/// Format: `{entity_type}:{slug}` where slug is the name lowercased with
/// non-alphanumeric characters replaced by hyphens, with consecutive hyphens
/// collapsed and leading/trailing hyphens stripped.
pub fn entity_id(entity_type: &str, name: &str) -> String {
    let slug = slugify(name);
    format!("{}:{}", entity_type, slug)
}

/// Slugify a string: lowercase, replace non-alphanumeric with hyphens,
/// collapse consecutive hyphens, strip leading/trailing hyphens.
/// Common symbolic characters (+, #) are preserved as words to avoid
/// collisions (e.g., "C++" → "c-plus-plus", not "c").
pub fn slugify(s: &str) -> String {
    // Expand symbolic characters before slugifying
    let expanded = s
        .replace("++", "-plus-plus")
        .replace('+', "-plus")
        .replace('#', "-sharp")
        .replace('&', "-and");

    let mut slug = String::with_capacity(expanded.len());
    let mut last_was_hyphen = true; // prevents leading hyphen
    for ch in expanded.chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    // Strip trailing hyphen
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

const VALID_ENTITY_TYPES: &[&str] = &["topic", "person", "project", "tool", "concept", "reference"];

/// Ollama /api/generate request body
#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    format: String,
    stream: bool,
}

/// Ollama /api/generate response body
#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

/// Wrapper for the JSON array the LLM returns.
#[derive(Deserialize)]
struct ExtractionResult {
    entities: Vec<ExtractedEntity>,
}

impl EntityExtractor {
    pub fn new(config: ExtractionConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("Failed to build reqwest client");
        Self { config, client }
    }

    /// Build the extraction prompt for a given article.
    fn build_prompt(title: &str, content: &str) -> String {
        format!(
            r#"You are a knowledge graph entity extractor. Analyze the following article and extract all notable entities.

For each entity, provide:
- "name": The canonical name of the entity
- "entity_type": One of: "topic", "person", "project", "tool", "concept", "reference"
- "description": A brief 1-sentence description
- "confidence": A float between 0.0 and 1.0 indicating your confidence
- "excerpt": A short text snippet from the article where this entity is mentioned

Entity type guidelines:
- "topic": A subject area or field (e.g., "Machine Learning", "Distributed Systems")
- "person": A named individual (e.g., "Linus Torvalds")
- "project": A software project or initiative (e.g., "Linux Kernel", "Kubernetes")
- "tool": A specific tool, library, language, or framework (e.g., "Rust", "PostgreSQL")
- "concept": An abstract idea or methodology (e.g., "Borrow Checking", "Event Sourcing")
- "reference": A book, paper, URL, or external resource (e.g., "The Rust Programming Language book")

Return a JSON object with a single key "entities" containing an array of entity objects. Only include entities you are at least 50% confident about. Limit to the 20 most important entities.

Title: {title}

Content:
{content}"#,
            title = title,
            content = content,
        )
    }

    /// Extract entities from article text. Returns an empty vec on any error
    /// (Ollama down, parse failure, etc.) — caller should log and continue.
    pub async fn extract(&self, title: &str, content: &str) -> Result<Vec<ExtractedEntity>> {
        if !self.config.enabled {
            return Ok(vec![]);
        }

        let prompt = Self::build_prompt(title, content);
        let url = format!("{}/api/generate", self.config.ollama_url);

        let request = OllamaRequest {
            model: self.config.model.clone(),
            prompt,
            format: "json".into(),
            stream: false,
        };

        let resp = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to reach Ollama")?;

        if !resp.status().is_success() {
            anyhow::bail!("Ollama returned HTTP {}", resp.status());
        }

        let ollama_resp: OllamaResponse = resp
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        let entities = Self::parse_response(&ollama_resp.response)?;
        Ok(entities)
    }

    /// Parse the JSON string returned by Ollama into a list of entities.
    /// Handles both `{"entities": [...]}` and bare `[...]` formats.
    fn parse_response(json_str: &str) -> Result<Vec<ExtractedEntity>> {
        // Try {"entities": [...]} first
        if let Ok(result) = serde_json::from_str::<ExtractionResult>(json_str) {
            return Ok(Self::filter_valid_entities(result.entities));
        }

        // Try bare array
        if let Ok(entities) = serde_json::from_str::<Vec<ExtractedEntity>>(json_str) {
            return Ok(Self::filter_valid_entities(entities));
        }

        anyhow::bail!("Failed to parse Ollama response as entity JSON: {}", json_str)
    }

    /// Filter out entities with invalid types or empty names.
    fn filter_valid_entities(entities: Vec<ExtractedEntity>) -> Vec<ExtractedEntity> {
        entities
            .into_iter()
            .filter(|e| {
                !e.name.trim().is_empty()
                    && VALID_ENTITY_TYPES.contains(&e.entity_type.as_str())
                    && e.confidence >= 0.5
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Rust"), "rust");
        assert_eq!(slugify("Machine Learning"), "machine-learning");
        assert_eq!(slugify("C++"), "c-plus-plus");
        assert_eq!(slugify("C#"), "c-sharp");
        assert_eq!(slugify("C"), "c");
        assert_eq!(slugify("R&D"), "r-and-d");
        assert_eq!(slugify("  Hello   World  "), "hello-world");
        assert_eq!(slugify("knowledge-nexus"), "knowledge-nexus");
    }

    #[test]
    fn test_entity_id() {
        assert_eq!(entity_id("tool", "Rust"), "tool:rust");
        assert_eq!(entity_id("topic", "Machine Learning"), "topic:machine-learning");
        assert_eq!(entity_id("person", "Linus Torvalds"), "person:linus-torvalds");
    }

    #[test]
    fn test_parse_response_wrapped() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool", "description": "A language", "confidence": 0.95, "excerpt": "written in Rust"},
            {"name": "Tokio", "entity_type": "tool", "description": "Async runtime", "confidence": 0.9, "excerpt": "using Tokio"}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].name, "Rust");
        assert_eq!(entities[1].entity_type, "tool");
    }

    #[test]
    fn test_parse_response_bare_array() {
        let json = r#"[
            {"name": "SurrealDB", "entity_type": "tool", "description": "Multi-model DB", "confidence": 0.85}
        ]"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "SurrealDB");
    }

    #[test]
    fn test_parse_response_filters_invalid_type() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool", "confidence": 0.9},
            {"name": "Something", "entity_type": "invalid_type", "confidence": 0.8}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Rust");
    }

    #[test]
    fn test_parse_response_filters_low_confidence() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool", "confidence": 0.9},
            {"name": "Maybe", "entity_type": "concept", "confidence": 0.3}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
    }

    #[test]
    fn test_parse_response_filters_empty_name() {
        let json = r#"{"entities": [
            {"name": "", "entity_type": "tool", "confidence": 0.9},
            {"name": "Rust", "entity_type": "tool", "confidence": 0.9}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Rust");
    }

    #[test]
    fn test_parse_response_invalid_json() {
        let result = EntityExtractor::parse_response("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_build_prompt_contains_title_and_content() {
        let prompt = EntityExtractor::build_prompt("My Title", "My content here");
        assert!(prompt.contains("My Title"));
        assert!(prompt.contains("My content here"));
        assert!(prompt.contains("entity_type"));
    }

    #[test]
    fn test_default_confidence() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool"}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].confidence, 0.5);
    }
}
