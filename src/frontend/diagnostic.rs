use std::fmt;

use super::ast::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Syntax,
    Type,
    Runtime,
    Limit,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Syntax => "syntax error",
            Self::Type => "type error",
            Self::Runtime => "runtime error",
            Self::Limit => "limit exceeded",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub category: Category,
    pub message: String,
    pub span: Span,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn new(category: Category, message: impl Into<String>, span: Span) -> Self {
        Self {
            category,
            message: message.into(),
            span,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Render one bounded source excerpt, using Unicode-scalar columns.
    pub fn render(&self, source: &str) -> String {
        let mut start = self.span.start.min(source.len());
        while !source.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = self.span.end.max(start).min(source.len());
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        let line_start = source[..start].rfind('\n').map_or(0, |n| n + 1);
        let line_end = source[start..]
            .find(['\n', '\r'])
            .map_or(source.len(), |n| start + n);
        let line = source[..line_start].bytes().filter(|b| *b == b'\n').count() + 1;
        let col = source[line_start..start].chars().count();
        let chars: Vec<_> = source[line_start..line_end].chars().collect();
        let left = col.saturating_sub(160);
        let right = chars.len().min(left + 320);
        let prefix = if left > 0 { "…" } else { "" };
        let suffix = if right < chars.len() { "…" } else { "" };
        let excerpt: String = chars[left..right].iter().collect();
        let marked = source[start..end.min(line_end)].chars().count().max(1);
        let offset = col - left + usize::from(left > 0);
        let underline = "^".repeat(marked.min(right.saturating_sub(col).max(1)));
        let continuation = if end > line_end { " …" } else { "" };
        let message = clip(&self.message, 4096);
        let mut text = format!(
            "{}: {message}\n --> {line}:{}\n  |\n{line} | {prefix}{excerpt}{suffix}\n  | {}{underline}{continuation}",
            self.category,
            col + 1,
            " ".repeat(offset)
        );
        if let Some(hint) = &self.hint {
            text.push_str("\n  hint: ");
            text.push_str(&clip(hint, 1024));
        }
        text
    }
}

fn clip(text: &str, limit: usize) -> String {
    let mut result: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        result.push('…');
    }
    result
}

pub fn render_diagnostics(diagnostics: &[Diagnostic], source: &str) -> String {
    diagnostics
        .iter()
        .map(|d| d.render(source))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_columns_multiline_and_clipping() {
        let source = "é\nαβ xyz\nend";
        let start = source.find("xyz").unwrap();
        let error = Diagnostic::new(Category::Type, "test", Span::new(start, source.len()));
        assert_eq!(
            error.render(source),
            "type error: test\n --> 2:4\n  |\n2 | αβ xyz\n  |    ^^^ …"
        );
        let source = format!("{}x", "é".repeat(10000));
        let error = Diagnostic::new(Category::Type, "test", Span::new(20000, 20001));
        assert!(error.render(&source).len() < 1000);
        assert!(error.render(&source).contains("1:10001"));
    }
}
