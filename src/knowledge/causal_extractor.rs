//! LLM-based causal-edge extraction via local Ollama.
//!
//! Given two article excerpts (a "source" article and a candidate "effect"
//! article), prompts the LLM to decide whether the source caused or enabled
//! the effect. Returns confidence and rationale. Mirrors the structure of
//! `EntityExtractor`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::GraphConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalClaim {
    /// 0.0 if the LLM judges no causal link; otherwise its confidence
    /// in the source → effect direction.
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub rationale: Option<String>,
}

pub struct CausalExtractor {
    config: GraphConfig,
    ollama_url: String,
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

impl CausalExtractor {
    pub fn new(config: GraphConfig, ollama_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("build reqwest client");
        Self { config, ollama_url, client }
    }

    fn build_prompt(source_title: &str, source_excerpt: &str, effect_title: &str, effect_excerpt: &str) -> String {
        format!(
            r#"You judge causal relationships between two events or claims described in short excerpts.

Determine whether SOURCE causes or enables EFFECT. Output a JSON object with:
- "confidence": a float in [0.0, 1.0]. 0.0 means no causal relationship; 1.0 means clearly causal.
- "rationale": a short string (<=200 chars) explaining your judgment.

Only output the JSON object. No prose.

SOURCE: {st}
SOURCE EXCERPT: {se}

EFFECT: {et}
EFFECT EXCERPT: {ee}"#,
            st = source_title, se = source_excerpt,
            et = effect_title, ee = effect_excerpt,
        )
    }

    /// Returns `None` if `causal_enabled` is false or the LLM call fails.
    pub async fn extract(
        &self,
        source_title: &str,
        source_excerpt: &str,
        effect_title: &str,
        effect_excerpt: &str,
    ) -> Result<Option<CausalClaim>> {
        if !self.config.causal_enabled {
            return Ok(None);
        }

        let prompt = Self::build_prompt(source_title, source_excerpt, effect_title, effect_excerpt);
        let url = format!("{}/api/generate", self.ollama_url);

        let req = OllamaRequest {
            model: self.config.causal_model.clone(),
            prompt,
            format: "json".into(),
            stream: false,
        };

        let resp = self.client.post(&url).json(&req).send().await.context("ollama")?;
        if !resp.status().is_success() {
            anyhow::bail!("ollama HTTP {}", resp.status());
        }
        let body: OllamaResponse = resp.json().await.context("parse ollama body")?;
        Self::parse_response(&body.response).map(Some)
    }

    fn parse_response(json_str: &str) -> Result<CausalClaim> {
        let claim: CausalClaim = serde_json::from_str(json_str)
            .with_context(|| format!("parse causal claim from `{}`", json_str))?;
        // Clamp confidence to [0.0, 1.0]
        let clamped = claim.confidence.clamp(0.0, 1.0);
        Ok(CausalClaim { confidence: clamped, rationale: claim.rationale })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_response_extracts_confidence_and_rationale() {
        let json = r#"{"confidence": 0.78, "rationale": "explicit 'because' clause"}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert!((claim.confidence - 0.78).abs() < 1e-9);
        assert_eq!(claim.rationale.as_deref(), Some("explicit 'because' clause"));
    }

    #[test]
    fn parse_response_clamps_confidence() {
        let json = r#"{"confidence": 1.5, "rationale": "very confident"}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert_eq!(claim.confidence, 1.0);

        let json = r#"{"confidence": -0.2}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert_eq!(claim.confidence, 0.0);
    }

    #[test]
    fn parse_response_handles_missing_rationale() {
        let json = r#"{"confidence": 0.4}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert_eq!(claim.confidence, 0.4);
        assert!(claim.rationale.is_none());
    }

    #[test]
    fn parse_response_errors_on_bad_json() {
        let res = CausalExtractor::parse_response("not json");
        assert!(res.is_err());
    }

    #[test]
    fn build_prompt_contains_both_titles() {
        let p = CausalExtractor::build_prompt("S", "se", "E", "ee");
        assert!(p.contains("SOURCE: S"));
        assert!(p.contains("EFFECT: E"));
    }
}
