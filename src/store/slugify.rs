/// Slugify a string: lowercase, replace non-alphanumeric with hyphens,
/// collapse consecutive hyphens, strip leading/trailing hyphens.
/// Common symbolic characters (+, #) are preserved as words to avoid
/// collisions (e.g., "C++" → "c-plus-plus", not "c").
pub fn slugify(s: &str) -> String {
    // Expand symbolic characters before slugifying
    let expanded = s
        .replace("++", "-plus-plus-")
        .replace('+', "-plus-")
        .replace('#', "-sharp-")
        .replace('&', "-and-");

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

/// Generate a deterministic entity ID from type and name.
///
/// Format: `{entity_type}:{slug}` where slug is the name lowercased with
/// non-alphanumeric characters replaced by hyphens, with consecutive hyphens
/// collapsed and leading/trailing hyphens stripped.
pub fn entity_id(entity_type: &str, name: &str) -> String {
    let slug = slugify(name);
    format!("{}:{}", entity_type, slug)
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
}
