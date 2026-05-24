//! Turning the regex text inputs into `FilterRegexes`.

use regex::Regex;
use rssfilter_core::FilterRegexes;

/// Compile a single optional regex from a text input (empty input = no filter).
fn compile(input: &str) -> Result<Vec<Regex>, regex::Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(vec![Regex::new(trimmed)?])
    }
}

/// Compile the three filter inputs into a [`FilterRegexes`]. The first invalid
/// pattern short-circuits with its error message.
pub fn compile_all(title: &str, guid: &str, link: &str) -> Result<FilterRegexes, String> {
    let one = |s: &str| compile(s).map_err(|e| e.to_string());
    Ok(FilterRegexes {
        title_regexes: one(title)?,
        guid_regexes: one(guid)?,
        link_regexes: one(link)?,
    })
}
