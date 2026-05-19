use anyhow::{Context, Result};
use pulldown_cmark::{html, Options, Parser};
use std::collections::HashMap;

/// Convert markdown content to HTML with a clean template
pub fn markdown_to_html(content: &str, title: &str) -> Result<String> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(content, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);

    let template = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{}</title>
    <style>
        body {{
            font-family: "Georgia", "Times New Roman", serif;
            line-height: 1.6;
            max-width: 780px;
            margin: 0 auto;
            padding: 40px 20px;
            color: #333;
            font-size: 16px;
        }}
        h1, h2, h3, h4, h5, h6 {{
            font-family: "Helvetica Neue", Arial, sans-serif;
            font-weight: 600;
            margin-top: 1.5em;
            margin-bottom: 0.5em;
            line-height: 1.3;
        }}
        h1 {{
            font-size: 2em;
            border-bottom: 2px solid #eee;
            padding-bottom: 0.3em;
        }}
        h2 {{
            font-size: 1.5em;
            border-bottom: 1px solid #eee;
            padding-bottom: 0.2em;
        }}
        h3 {{
            font-size: 1.25em;
        }}
        p {{
            margin: 1em 0;
            text-align: justify;
        }}
        code {{
            background: #f4f4f4;
            padding: 2px 6px;
            border-radius: 3px;
            font-family: "Courier New", monospace;
            font-size: 0.9em;
        }}
        pre {{
            background: #f4f4f4;
            padding: 15px;
            border-radius: 5px;
            overflow-x: auto;
            line-height: 1.4;
        }}
        pre code {{
            background: none;
            padding: 0;
        }}
        blockquote {{
            border-left: 4px solid #ddd;
            margin: 1em 0;
            padding-left: 1em;
            color: #666;
            font-style: italic;
        }}
        table {{
            border-collapse: collapse;
            width: 100%;
            margin: 1em 0;
        }}
        th, td {{
            border: 1px solid #ddd;
            padding: 8px 12px;
            text-align: left;
        }}
        th {{
            background: #f8f8f8;
            font-weight: 600;
        }}
        img {{
            max-width: 100%;
            height: auto;
            display: block;
            margin: 1em auto;
        }}
        ul, ol {{
            margin: 1em 0;
            padding-left: 2em;
        }}
        li {{
            margin: 0.3em 0;
        }}
        hr {{
            border: none;
            border-top: 2px solid #eee;
            margin: 2em 0;
        }}
        a {{
            color: #0066cc;
            text-decoration: none;
        }}
        .task-list-item {{
            list-style: none;
            margin-left: -1.5em;
        }}
        .task-list-item input {{
            margin-right: 0.5em;
        }}
    </style>
</head>
<body>
    <h1>{}</h1>
    {}
</body>
</html>"#,
        title, title, html_output
    );

    Ok(template)
}

/// Extract title from markdown (first H1 or filename)
pub fn extract_title(content: &str, fallback: &str) -> String {
    // Try to find first # heading
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            return trimmed[2..].trim().to_string();
        }
    }
    fallback.to_string()
}

/// Parse frontmatter from markdown content (simple YAML-like format)
pub fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut metadata = HashMap::new();
    let mut remaining = content.to_string();

    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        if let Some(end) = content[3..].find("---") {
            let frontmatter = &content[3..end + 3];
            remaining = content[end + 6..].to_string();

            for line in frontmatter.lines() {
                if let Some(colon) = line.find(':') {
                    let key = line[..colon].trim().to_string();
                    let value = line[colon + 1..].trim().to_string();
                    metadata.insert(key, value);
                }
            }
        }
    }

    (metadata, remaining)
}

/// Read a markdown file and convert to HTML
pub fn convert_file(path: &std::path::Path) -> Result<(String, String)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {:?}", path))?;

    let (metadata, body) = parse_frontmatter(&content);

    let title = metadata
        .get("title")
        .cloned()
        .unwrap_or_else(|| {
            let fallback = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled");
            extract_title(&body, fallback)
        });

    let html = markdown_to_html(&body, &title)?;

    Ok((title, html))
}

/// Read a markdown file and convert to PDF bytes
pub fn convert_file_to_pdf(path: &std::path::Path) -> Result<(String, Vec<u8>)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {:?}", path))?;

    let (metadata, body) = parse_frontmatter(&content);

    let title = metadata
        .get("title")
        .cloned()
        .unwrap_or_else(|| {
            let fallback = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled");
            extract_title(&body, fallback)
        });

    let pdf_bytes = markdown2pdf::parse_into_bytes(body, markdown2pdf::config::ConfigSource::Default, None)
        .with_context(|| "Failed to convert markdown to PDF")?;

    Ok((title, pdf_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title_from_h1() {
        let md = "# My Title\n\nSome content";
        assert_eq!(extract_title(md, "fallback"), "My Title");
    }

    #[test]
    fn test_extract_title_fallback() {
        let md = "Some content without heading";
        assert_eq!(extract_title(md, "fallback"), "fallback");
    }

    #[test]
    fn test_parse_frontmatter() {
        let md = "---\ntitle: Test Doc\nauthor: Me\n---\n\nContent here";
        let (meta, body) = parse_frontmatter(md);
        assert_eq!(meta.get("title"), Some(&"Test Doc".to_string()));
        assert_eq!(meta.get("author"), Some(&"Me".to_string()));
        assert!(body.contains("Content here"));
    }
}
