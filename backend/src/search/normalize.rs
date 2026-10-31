use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub fn normalize(value: &str) -> String {
    value
        .nfd()
        .flat_map(char::to_lowercase)
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn query(value: &str) -> Option<String> {
    let value = value.trim();
    (1..=200)
        .contains(&value.chars().count())
        .then(|| normalize(value))
}

pub fn terms(value: &str) -> Option<String> {
    let words: Vec<_> = value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|v| !v.is_empty())
        .map(|w| format!("\"{w}\"*"))
        .collect();
    (!words.is_empty()).then(|| words.join(" AND "))
}
