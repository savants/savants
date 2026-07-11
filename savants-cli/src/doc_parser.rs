//! Documentation parser — splits markdown files into sections by heading.
//!
//! Each section becomes a searchable entity with file path, heading, content,
//! heading level, and line number. Used by `savants up` to index docs alongside
//! code, and by `savants ask` / `savants search` to return doc results.

use std::path::Path;
use walkdir::WalkDir;

/// A section of a markdown document, split by heading.
#[derive(Debug, Clone)]
pub struct DocSection {
    pub file: String,
    pub heading: String,
    pub content: String,
    pub level: usize,
    pub line: usize,
}

/// Parse all markdown files in a directory (recursively) into sections.
pub fn parse_docs(dir: &str) -> Vec<DocSection> {
    let mut sections = Vec::new();
    let base = Path::new(dir);

    let skip_dirs = [
        "node_modules", ".git", "target", "dist", "build",
        "__pycache__", ".venv", "venv", ".turbo", ".next",
    ];

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !skip_dirs.iter().any(|d| name == *d)
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match ext {
            "md" => {
                let rel_path = path.strip_prefix(base)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                if let Ok(content) = std::fs::read_to_string(path) {
                    let file_sections = parse_markdown(&content, &rel_path);
                    sections.extend(file_sections);
                }
            }
            "astro" => {
                // Extract text content from .astro files (strip HTML/JSX tags)
                let rel_path = path.strip_prefix(base)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                if let Ok(content) = std::fs::read_to_string(path) {
                    let text = extract_astro_text(&content);
                    if !text.trim().is_empty() {
                        let file_sections = parse_markdown(&text, &rel_path);
                        sections.extend(file_sections);
                    }
                }
            }
            _ => {}
        }
    }

    sections
}

/// Parse a single markdown string into sections split by headings.
pub fn parse_markdown(content: &str, file: &str) -> Vec<DocSection> {
    let mut sections = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_count = 0;
    let mut in_code_block = false;

    let mut current_heading = String::new();
    let mut current_level: usize = 0;
    let mut current_line: usize = 0;
    let mut current_content = String::new();

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();

        // Handle YAML frontmatter (between --- lines at the start)
        if trimmed == "---" {
            frontmatter_count += 1;
            if frontmatter_count == 1 && line_num <= 2 {
                in_frontmatter = true;
                continue;
            }
            if in_frontmatter && frontmatter_count == 2 {
                in_frontmatter = false;
                continue;
            }
        }
        if in_frontmatter {
            continue;
        }

        // Track code blocks — don't split on # inside them
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            current_content.push_str(line);
            current_content.push('\n');
            continue;
        }
        if in_code_block {
            current_content.push_str(line);
            current_content.push('\n');
            continue;
        }

        // Check for heading
        if let Some(heading_info) = parse_heading(trimmed) {
            // Save previous section if non-empty
            let content_trimmed = current_content.trim().to_string();
            if !content_trimmed.is_empty() && current_line > 0 {
                sections.push(DocSection {
                    file: file.to_string(),
                    heading: current_heading.clone(),
                    content: content_trimmed,
                    level: current_level,
                    line: current_line,
                });
            }

            // Start new section
            current_heading = heading_info.1.to_string();
            current_level = heading_info.0;
            current_line = line_num;
            current_content.clear();
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Save final section
    let content_trimmed = current_content.trim().to_string();
    if !content_trimmed.is_empty() {
        if current_line == 0 {
            // File with no headings — treat whole file as one section
            let filename = Path::new(file)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            sections.push(DocSection {
                file: file.to_string(),
                heading: filename,
                content: content_trimmed,
                level: 0,
                line: 1,
            });
        } else {
            sections.push(DocSection {
                file: file.to_string(),
                heading: current_heading,
                content: content_trimmed,
                level: current_level,
                line: current_line,
            });
        }
    }

    sections
}

/// Parse a markdown heading line. Returns (level, heading_text) or None.
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    if !line.starts_with('#') {
        return None;
    }

    let level = line.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }

    let rest = &line[level..];
    if !rest.starts_with(' ') && !rest.is_empty() {
        return None; // e.g. "#hashtag" is not a heading
    }

    let heading_text = rest.trim();
    if heading_text.is_empty() {
        return None;
    }

    Some((level, heading_text))
}

/// Extract readable text from an Astro file by stripping HTML/JSX tags.
fn extract_astro_text(content: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_frontmatter = false;
    let mut frontmatter_count = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip Astro frontmatter (between --- lines)
        if trimmed == "---" {
            frontmatter_count += 1;
            if frontmatter_count % 2 == 1 {
                in_frontmatter = true;
            } else {
                in_frontmatter = false;
            }
            continue;
        }
        if in_frontmatter {
            continue;
        }

        // Simple tag stripping — remove <...> sequences
        let mut line_text = String::new();
        for ch in line.chars() {
            if ch == '<' {
                in_tag = true;
            } else if ch == '>' {
                in_tag = false;
                line_text.push(' ');
            } else if !in_tag {
                line_text.push(ch);
            }
        }

        let cleaned = line_text.trim();
        if !cleaned.is_empty() {
            result.push_str(cleaned);
            result.push('\n');
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_heading() {
        assert_eq!(parse_heading("# Hello"), Some((1, "Hello")));
        assert_eq!(parse_heading("## Sub heading"), Some((2, "Sub heading")));
        assert_eq!(parse_heading("### Level 3"), Some((3, "Level 3")));
        assert_eq!(parse_heading("Not a heading"), None);
        assert_eq!(parse_heading("#hashtag"), None);
    }

    #[test]
    fn test_parse_markdown_basic() {
        let md = "# Title\nSome intro text.\n\n## Section A\nContent A.\n\n## Section B\nContent B.\n";
        let sections = parse_markdown(md, "test.md");
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].heading, "Title");
        assert_eq!(sections[0].level, 1);
        assert!(sections[0].content.contains("Some intro text"));
        assert_eq!(sections[1].heading, "Section A");
        assert_eq!(sections[2].heading, "Section B");
    }

    #[test]
    fn test_frontmatter_skipped() {
        let md = "---\ntitle: Test\n---\n# Real Heading\nContent here.\n";
        let sections = parse_markdown(md, "test.md");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading, "Real Heading");
    }

    #[test]
    fn test_code_block_not_split() {
        let md = "# Heading\nSome text.\n```rust\n# This is a comment not a heading\nfn main() {}\n```\nMore text.\n";
        let sections = parse_markdown(md, "test.md");
        assert_eq!(sections.len(), 1);
        assert!(sections[0].content.contains("# This is a comment"));
    }
}
