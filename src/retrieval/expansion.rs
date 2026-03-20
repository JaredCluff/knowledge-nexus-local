//! Query Expander - generates query variants for low-confidence results.
//! Uses synonym expansion, term splitting, and common transformations.

pub struct QueryExpander;

impl QueryExpander {
    pub fn new() -> Self {
        Self
    }

    /// Generate expanded query variants from the original query
    pub fn expand(&self, query: &str) -> Vec<String> {
        let mut variants = Vec::new();

        // Original query is always first
        variants.push(query.to_string());

        // Try removing stop words
        let without_stop = self.remove_stop_words(query);
        if without_stop != query && !without_stop.is_empty() {
            variants.push(without_stop);
        }

        // Try splitting compound terms
        let split = self.split_compound_terms(query);
        for s in split {
            if s != query && !variants.contains(&s) {
                variants.push(s);
            }
        }

        // Try synonym expansion
        let synonyms = self.expand_synonyms(query);
        for s in synonyms {
            if !variants.contains(&s) {
                variants.push(s);
            }
        }

        variants
    }

    /// Remove common English stop words
    fn remove_stop_words(&self, query: &str) -> String {
        const STOP_WORDS: &[&str] = &[
            "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has",
            "had", "do", "does", "did", "will", "would", "could", "should", "may", "might", "can",
            "shall", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into",
            "about", "like", "through", "after", "over", "between", "out", "against", "during",
            "without", "before", "under", "around", "among", "it", "its", "this", "that", "these",
            "those", "my", "your", "his", "her", "how", "what", "when", "where", "who", "which",
            "why",
        ];

        query
            .split_whitespace()
            .filter(|w| !STOP_WORDS.contains(&w.to_lowercase().as_str()))
            .collect::<Vec<&str>>()
            .join(" ")
    }

    /// Split camelCase and snake_case terms
    fn split_compound_terms(&self, query: &str) -> Vec<String> {
        let mut results = Vec::new();

        let words: Vec<&str> = query.split_whitespace().collect();
        let mut expanded_words = Vec::new();
        let mut changed = false;

        for word in &words {
            // Split camelCase: "fileSystem" → "file System"
            let camel_split = self.split_camel_case(word);
            if camel_split.len() > 1 {
                expanded_words.extend(camel_split);
                changed = true;
            }
            // Split snake_case: "file_system" → "file system"
            else if word.contains('_') {
                let parts: Vec<&str> = word.split('_').filter(|s| !s.is_empty()).collect();
                if parts.len() > 1 {
                    expanded_words.extend(parts.iter().map(|s| s.to_string()));
                    changed = true;
                } else {
                    expanded_words.push(word.to_string());
                }
            } else {
                expanded_words.push(word.to_string());
            }
        }

        if changed {
            results.push(expanded_words.join(" "));
        }

        results
    }

    /// Split camelCase into separate words
    fn split_camel_case(&self, word: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();

        for ch in word.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                parts.push(current.clone());
                current.clear();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            parts.push(current);
        }

        if parts.len() > 1 {
            parts.iter().map(|p| p.to_lowercase()).collect()
        } else {
            vec![word.to_string()]
        }
    }

    /// Expand known synonyms in technical domains
    fn expand_synonyms(&self, query: &str) -> Vec<String> {
        let synonyms: &[(&[&str], &[&str])] = &[
            (
                &["error", "bug", "issue"],
                &["error", "bug", "issue", "problem", "failure"],
            ),
            (
                &["fix", "repair", "resolve"],
                &["fix", "repair", "resolve", "patch"],
            ),
            (
                &["config", "configuration", "settings"],
                &["config", "configuration", "settings", "preferences"],
            ),
            (
                &["docs", "documentation", "guide"],
                &["docs", "documentation", "guide", "manual", "reference"],
            ),
            (
                &["function", "method", "func"],
                &["function", "method", "func", "procedure"],
            ),
            (&["database", "db"], &["database", "db", "data store"]),
            (
                &["api", "endpoint"],
                &["api", "endpoint", "route", "interface"],
            ),
            (
                &["auth", "authentication"],
                &["auth", "authentication", "login", "authorization"],
            ),
            (
                &["deploy", "deployment"],
                &["deploy", "deployment", "release", "ship"],
            ),
            (
                &["test", "testing"],
                &["test", "testing", "spec", "assertion"],
            ),
        ];

        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for (triggers, expansions) in synonyms {
            for trigger in *triggers {
                if query_lower.contains(trigger) {
                    for expansion in *expansions {
                        if !query_lower.contains(expansion) {
                            let expanded = query_lower.replace(trigger, expansion);
                            if !results.contains(&expanded) {
                                results.push(expanded);
                            }
                        }
                    }
                }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_basic() {
        let expander = QueryExpander::new();
        let variants = expander.expand("how to fix the error");
        assert!(variants.len() > 1);
        assert_eq!(variants[0], "how to fix the error"); // Original first
    }

    #[test]
    fn test_stop_word_removal() {
        let expander = QueryExpander::new();
        let variants = expander.expand("how to configure the database");
        // Should contain a variant without stop words
        assert!(variants.iter().any(|v| v == "configure database"));
    }

    #[test]
    fn test_camel_case_split() {
        let expander = QueryExpander::new();
        let variants = expander.expand("fileSystem error");
        assert!(variants.iter().any(|v| v.contains("file system")));
    }

    #[test]
    fn test_snake_case_split() {
        let expander = QueryExpander::new();
        let variants = expander.expand("file_system error");
        assert!(variants.iter().any(|v| v.contains("file system")));
    }

    #[test]
    fn test_synonym_expansion() {
        let expander = QueryExpander::new();
        let variants = expander.expand("fix the database error");
        // Should contain synonym variants
        assert!(variants.iter().any(|v| v.contains("bug")
            || v.contains("issue")
            || v.contains("repair")
            || v.contains("patch")));
    }
}
