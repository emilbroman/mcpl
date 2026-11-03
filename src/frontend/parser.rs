use super::ast::*;
use super::diagnostic::{Category, Diagnostic};
use serde_json::Value;

const SOURCE_LIMIT: usize = 64 * 1024;
const DEPTH_LIMIT: usize = 256;

pub fn parse_program(source: &str) -> Result<Program, Diagnostic> {
    parse_on_stack(source, |parser| {
        let mut statements = Vec::new();
        while !parser.at(&TokenKind::Eof) {
            let start = parser.current().span.start;
            let name = parser.assignment_name();
            let expression = parser.expression(0, true)?.expr;
            let end = parser.terminator()?.end;
            statements.push(Statement {
                name,
                expression,
                span: Span::new(start, end),
            });
        }
        Ok(Program {
            statements,
            span: Span::new(0, source.len()),
        })
    })
}

pub fn parse_expression(source: &str) -> Result<Expr, Diagnostic> {
    parse_on_stack(source, |parser| {
        let expression = parser.expression(0, true)?.expr;
        if !parser.at(&TokenKind::Eof) {
            return Err(parser.unexpected());
        }
        Ok(expression)
    })
}

fn parse_on_stack<T: Send>(
    source: &str,
    parse: impl FnOnce(&mut Parser<'_>) -> Result<T, Diagnostic> + Send,
) -> Result<T, Diagnostic> {
    if source.len() > SOURCE_LIMIT {
        return Err(Diagnostic::new(
            Category::Limit,
            "program source exceeds 64 KB",
            Span::new(0, 0),
        )
        .with_hint("reduce the source size or split the program"));
    }
    // A bounded dedicated stack also protects callers using small executor/test stacks.
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn_scoped(scope, || parse(&mut Parser::new(source)))
            .map_err(|_| {
                Diagnostic::new(
                    Category::Limit,
                    "unable to allocate parser stack",
                    Span::new(0, 0),
                )
            })?;
        match thread.join() {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Name(Vec<Name>),
    Member(Vec<Name>),
    String(String),
    Number(String),
    Literal(Value),
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
    Comma,
    Colon,
    Semicolon,
    Question,
    Assign,
    Arrow,
    Bang,
    Binary(BinaryOp),
    Invalid(String, Option<String>),
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    span: Span,
}

fn whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}
fn part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}
fn first(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

struct Lexer<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Lexer<'a> {
    fn token(&mut self) -> Token {
        let bytes = self.source.as_bytes();
        loop {
            while self.position < bytes.len() && whitespace(bytes[self.position]) {
                self.position += 1;
            }
            if self.source[self.position..].starts_with("//") {
                while self.position < bytes.len() && !matches!(bytes[self.position], b'\r' | b'\n')
                {
                    self.position += 1;
                }
            } else {
                break;
            }
        }
        let start = self.position;
        if start == bytes.len() {
            return Token {
                kind: TokenKind::Eof,
                span: Span::new(start, start),
            };
        }
        let kind = match bytes[start] {
            b'r' if bytes.get(start + 1) == Some(&b'"') => {
                self.position += 2;
                let content = self.position;
                if let Some(end) = self.source[content..].find('"') {
                    self.position = content + end + 1;
                    TokenKind::String(self.source[content..content + end].to_owned())
                } else {
                    self.position = bytes.len();
                    TokenKind::Invalid("unterminated raw string".into(), None)
                }
            }
            b'"' => self.string(),
            b'0'..=b'9' => self.number(),
            byte if first(byte) => self.name(false),
            b'.' => self.name(true),
            _ => {
                let rest = &self.source[start..];
                let compound = [
                    ("=>", TokenKind::Arrow),
                    ("==", TokenKind::Binary(BinaryOp::Equal)),
                    ("!=", TokenKind::Binary(BinaryOp::NotEqual)),
                    ("<=", TokenKind::Binary(BinaryOp::LessEqual)),
                    (">=", TokenKind::Binary(BinaryOp::GreaterEqual)),
                    ("&&", TokenKind::Binary(BinaryOp::And)),
                    ("||", TokenKind::Binary(BinaryOp::Or)),
                ];
                if let Some((text, kind)) = compound
                    .into_iter()
                    .find(|(text, _)| rest.starts_with(text))
                {
                    self.position += text.len();
                    kind
                } else {
                    let ch = rest.chars().next().unwrap();
                    self.position += ch.len_utf8();
                    match ch {
                        '(' => TokenKind::LeftParen,
                        ')' => TokenKind::RightParen,
                        '[' => TokenKind::LeftBracket,
                        ']' => TokenKind::RightBracket,
                        '{' => TokenKind::LeftBrace,
                        '}' => TokenKind::RightBrace,
                        ',' => TokenKind::Comma,
                        ':' => TokenKind::Colon,
                        ';' => TokenKind::Semicolon,
                        '?' => TokenKind::Question,
                        '=' => TokenKind::Assign,
                        '!' => TokenKind::Bang,
                        '+' => TokenKind::Binary(BinaryOp::Add),
                        '-' => TokenKind::Binary(BinaryOp::Subtract),
                        '*' => TokenKind::Binary(BinaryOp::Multiply),
                        '/' => TokenKind::Binary(BinaryOp::Divide),
                        '<' => TokenKind::Binary(BinaryOp::Less),
                        '>' => TokenKind::Binary(BinaryOp::Greater),
                        _ => TokenKind::Invalid(format!("unexpected '{ch}'"), None),
                    }
                }
            }
        };
        Token {
            kind,
            span: Span::new(start, self.position),
        }
    }

    fn name(&mut self, member: bool) -> TokenKind {
        let bytes = self.source.as_bytes();
        let start = self.position;
        let mut names = Vec::new();
        if member {
            self.position += 1;
        }
        loop {
            let part_start = self.position;
            while self.position < bytes.len() && part(bytes[self.position]) {
                self.position += 1;
            }
            if part_start == self.position {
                return TokenKind::Invalid(
                    "expected a name segment after '.'".into(),
                    Some("use string indexing for names containing empty segments".into()),
                );
            }
            names.push(Name {
                text: self.source[part_start..self.position].to_owned(),
                span: Span::new(part_start, self.position),
            });
            if bytes.get(self.position) != Some(&b'.') {
                break;
            }
            self.position += 1;
        }
        if !member && self.position - start > 128 {
            return TokenKind::Invalid(
                "symbol exceeds 128 ASCII characters".into(),
                Some("use a shorter name or string indexing".into()),
            );
        }
        if !member && matches!(names[0].text.as_str(), "true" | "false" | "null") {
            if names.len() != 1 {
                return TokenKind::Invalid("reserved literal cannot be a symbol root".into(), None);
            }
            return TokenKind::Literal(match names[0].text.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => Value::Null,
            });
        }
        if member {
            TokenKind::Member(names)
        } else {
            TokenKind::Name(names)
        }
    }

    fn string(&mut self) -> TokenKind {
        let start = self.position;
        self.position += 1;
        let bytes = self.source.as_bytes();
        let mut escaped = false;
        while self.position < bytes.len() {
            let byte = bytes[self.position];
            self.position += 1;
            if byte == b'"' && !escaped {
                return match serde_json::from_str::<String>(&self.source[start..self.position]) {
                    Ok(value) => TokenKind::String(value),
                    Err(error) => {
                        let hint = if error.to_string().contains("surrogate")
                            || error.to_string().contains("hex escape")
                        {
                            Some("supply a valid UTF-16 surrogate pair".into())
                        } else {
                            Some(
                                "use JSON string escapes, or a raw string for regex patterns"
                                    .into(),
                            )
                        };
                        TokenKind::Invalid("invalid JSON string".into(), hint)
                    }
                };
            }
            escaped = byte == b'\\' && !escaped;
        }
        TokenKind::Invalid("unterminated string".into(), None)
    }

    fn number(&mut self) -> TokenKind {
        let start = self.position;
        let bytes = self.source.as_bytes();
        if bytes[self.position] == b'0' {
            self.position += 1;
        } else {
            while self.position < bytes.len() && bytes[self.position].is_ascii_digit() {
                self.position += 1;
            }
        }
        let mut valid = true;
        if bytes.get(self.position) == Some(&b'.')
            && bytes.get(self.position + 1).is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
            let digits = self.position;
            while self.position < bytes.len() && bytes[self.position].is_ascii_digit() {
                self.position += 1;
            }
            valid &= self.position > digits;
        }
        if matches!(bytes.get(self.position), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(bytes.get(self.position), Some(b'+' | b'-')) {
                self.position += 1;
            }
            let digits = self.position;
            while self.position < bytes.len() && bytes[self.position].is_ascii_digit() {
                self.position += 1;
            }
            valid &= self.position > digits;
        }
        if bytes.get(self.position).is_some_and(|byte| part(*byte)) {
            valid = false;
            while self.position < bytes.len()
                && (part(bytes[self.position]) || bytes[self.position] == b'.')
            {
                self.position += 1;
            }
        }
        if valid {
            TokenKind::Number(self.source[start..self.position].to_owned())
        } else {
            TokenKind::Invalid(
                "invalid JSON number or leading-digit name".into(),
                Some("choose a name starting with a letter or '_'".into()),
            )
        }
    }
}

struct Parsed {
    expr: Expr,
    depth: usize,
}

fn node(kind: ExprKind, span: Span, child_depth: usize) -> Result<Parsed, Diagnostic> {
    if child_depth >= DEPTH_LIMIT {
        return Err(depth_error(span));
    }
    Ok(Parsed {
        expr: Expr { kind, span },
        depth: child_depth + 1,
    })
}

fn depth_error(span: Span) -> Diagnostic {
    Diagnostic::new(Category::Limit, "expression nesting exceeds 256", span)
        .with_hint("reduce expression nesting or split the program")
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    position: usize,
    recursion: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        let mut lexer = Lexer {
            source,
            position: 0,
        };
        let mut tokens = Vec::new();
        loop {
            let token = lexer.token();
            let done = matches!(token.kind, TokenKind::Eof | TokenKind::Invalid(..));
            tokens.push(token);
            if done {
                break;
            }
        }
        Self {
            source,
            tokens,
            position: 0,
            recursion: 0,
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.position]
    }
    fn at(&self, kind: &TokenKind) -> bool {
        &self.current().kind == kind
    }
    fn take(&mut self) -> Token {
        let token = self.current().clone();
        if self.position + 1 < self.tokens.len() {
            self.position += 1;
        }
        token
    }
    fn consume(&mut self, kind: &TokenKind) -> Option<Span> {
        if self.at(kind) {
            Some(self.take().span)
        } else {
            None
        }
    }
    fn expect(&mut self, kind: &TokenKind, spelling: &str) -> Result<Span, Diagnostic> {
        self.consume(kind).ok_or_else(|| {
            if matches!(
                self.current().kind,
                TokenKind::Invalid(..) | TokenKind::Assign
            ) {
                self.unexpected()
            } else {
                Diagnostic::new(
                    Category::Syntax,
                    format!("expected '{spelling}'"),
                    self.current().span,
                )
            }
        })
    }
    fn unexpected(&self) -> Diagnostic {
        let token = self.current();
        match &token.kind {
            TokenKind::Invalid(message, hint) => Diagnostic {
                category: Category::Syntax,
                message: message.clone(),
                span: token.span,
                hint: hint.clone(),
            },
            TokenKind::Assign => Diagnostic::new(Category::Syntax, "unexpected '='", token.span)
                .with_hint(
                    "to sequence inside an expression, wrap it in parentheses: (a = 1; a + 1)",
                ),
            TokenKind::Eof => {
                Diagnostic::new(Category::Syntax, "unexpected end of input", token.span)
            }
            _ => Diagnostic::new(
                Category::Syntax,
                format!(
                    "unexpected '{}'",
                    &self.source[token.span.start..token.span.end]
                ),
                token.span,
            ),
        }
    }
    fn terminator(&mut self) -> Result<Span, Diagnostic> {
        if let Some(span) = self.consume(&TokenKind::Semicolon) {
            return Ok(span);
        }
        if matches!(
            self.current().kind,
            TokenKind::Assign | TokenKind::Invalid(..)
        ) {
            return Err(self.unexpected());
        }
        Err(Diagnostic::new(
            Category::Syntax,
            "expected ';' after statement",
            self.current().span,
        )
        .with_hint("every statement must end with ';'"))
    }
    fn joined_name(&self, span: Span) -> Name {
        Name {
            text: self.source[span.start..span.end].to_owned(),
            span,
        }
    }
    fn assignment_name(&mut self) -> Option<Name> {
        if matches!(self.current().kind, TokenKind::Name(_))
            && self
                .tokens
                .get(self.position + 1)
                .is_some_and(|token| token.kind == TokenKind::Assign)
        {
            let span = self.take().span;
            self.take();
            Some(self.joined_name(span))
        } else {
            None
        }
    }
    fn lambda_ahead(&self) -> bool {
        if !self.at(&TokenKind::LeftParen) {
            return false;
        }
        let mut position = self.position + 1;
        if matches!(
            self.tokens.get(position).map(|token| &token.kind),
            Some(TokenKind::Name(_))
        ) {
            position += 1;
            while self
                .tokens
                .get(position)
                .is_some_and(|token| token.kind == TokenKind::Comma)
            {
                position += 1;
                if !matches!(
                    self.tokens.get(position).map(|token| &token.kind),
                    Some(TokenKind::Name(_))
                ) {
                    return false;
                }
                position += 1;
            }
        }
        self.tokens
            .get(position)
            .is_some_and(|token| token.kind == TokenKind::RightParen)
            && self
                .tokens
                .get(position + 1)
                .is_some_and(|token| token.kind == TokenKind::Arrow)
    }

    fn expression(&mut self, minimum: u8, allow_lambda: bool) -> Result<Parsed, Diagnostic> {
        if self.recursion >= DEPTH_LIMIT {
            return Err(depth_error(self.current().span));
        }
        self.recursion += 1;
        let result = self.expression_inner(minimum, allow_lambda);
        self.recursion -= 1;
        result
    }

    fn expression_inner(&mut self, minimum: u8, allow_lambda: bool) -> Result<Parsed, Diagnostic> {
        if allow_lambda && self.lambda_ahead() {
            let start = self.take().span.start;
            let mut parameters = Vec::new();
            if !self.at(&TokenKind::RightParen) {
                loop {
                    let span = self.take().span;
                    parameters.push(self.joined_name(span));
                    if self.consume(&TokenKind::Comma).is_none() {
                        break;
                    }
                }
            }
            self.take();
            self.take();
            let body = self.expression(0, true)?;
            let span = Span::new(start, body.expr.span.end);
            return node(
                ExprKind::Lambda {
                    parameters,
                    body: Box::new(body.expr),
                },
                span,
                body.depth,
            );
        }
        let mut left =
            if self.at(&TokenKind::Bang) || self.at(&TokenKind::Binary(BinaryOp::Subtract)) {
                let token = self.take();
                let op = if token.kind == TokenKind::Bang {
                    UnaryOp::Not
                } else {
                    UnaryOp::Negate
                };
                let operand = self.expression(7, false)?;
                let span = token.span.join(operand.expr.span);
                node(
                    ExprKind::Unary {
                        op,
                        operand: Box::new(operand.expr),
                    },
                    span,
                    operand.depth,
                )?
            } else {
                self.postfix()?
            };
        loop {
            let TokenKind::Binary(op) = self.current().kind else {
                break;
            };
            let precedence = match op {
                BinaryOp::Or => 1,
                BinaryOp::And => 2,
                BinaryOp::Equal | BinaryOp::NotEqual => 3,
                BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual => 4,
                BinaryOp::Add | BinaryOp::Subtract => 5,
                BinaryOp::Multiply | BinaryOp::Divide => 6,
            };
            if precedence < minimum {
                break;
            }
            let span = self.take().span;
            let bytes = self.source.as_bytes();
            if span.start == 0
                || !whitespace(bytes[span.start - 1])
                || !bytes.get(span.end).is_some_and(|byte| whitespace(*byte))
            {
                return Err(Diagnostic::new(
                    Category::Syntax,
                    format!(
                        "binary operator '{}' requires whitespace on both sides",
                        op.spelling()
                    ),
                    span,
                )
                .with_hint(format!(
                    "add whitespace on both sides of '{}'",
                    op.spelling()
                )));
            }
            let right = self.expression(precedence + 1, false)?;
            let span = left.expr.span.join(right.expr.span);
            let depth = left.depth.max(right.depth);
            left = node(
                ExprKind::Binary {
                    op,
                    left: Box::new(left.expr),
                    right: Box::new(right.expr),
                },
                span,
                depth,
            )?;
        }
        if minimum == 0 && self.consume(&TokenKind::Question).is_some() {
            let yes = self.expression(0, true)?;
            self.expect(&TokenKind::Colon, ":")?;
            let no = self.expression(0, true)?;
            let span = left.expr.span.join(no.expr.span);
            let depth = left.depth.max(yes.depth).max(no.depth);
            left = node(
                ExprKind::Conditional {
                    condition: Box::new(left.expr),
                    yes: Box::new(yes.expr),
                    no: Box::new(no.expr),
                },
                span,
                depth,
            )?;
        }
        Ok(left)
    }

    fn postfix(&mut self) -> Result<Parsed, Diagnostic> {
        let mut value = self.primary()?;
        loop {
            match &self.current().kind {
                TokenKind::LeftParen => {
                    self.take();
                    let mut arguments = Vec::new();
                    let mut depth = value.depth;
                    if !self.at(&TokenKind::RightParen) {
                        loop {
                            let argument = self.sequence()?;
                            depth = depth.max(argument.depth);
                            arguments.push(argument.expr);
                            if self.consume(&TokenKind::Comma).is_none() {
                                break;
                            }
                        }
                    }
                    let end = self.expect(&TokenKind::RightParen, ")")?;
                    let span = value.expr.span.join(end);
                    value = node(
                        ExprKind::Call {
                            callee: Box::new(value.expr),
                            arguments,
                        },
                        span,
                        depth,
                    )?;
                }
                TokenKind::LeftBracket => {
                    self.take();
                    let index = self.expression(0, true)?;
                    let end = self.expect(&TokenKind::RightBracket, "]")?;
                    let span = value.expr.span.join(end);
                    let depth = value.depth.max(index.depth);
                    value = node(
                        ExprKind::Index {
                            receiver: Box::new(value.expr),
                            index: Box::new(index.expr),
                        },
                        span,
                        depth,
                    )?;
                }
                TokenKind::Member(_) if value.expr.span.end == self.current().span.start => {
                    let token = self.take();
                    let TokenKind::Member(members) = token.kind else {
                        unreachable!()
                    };
                    let span = value.expr.span.join(token.span);
                    value = node(
                        ExprKind::Member {
                            receiver: Box::new(value.expr),
                            members,
                        },
                        span,
                        value.depth,
                    )?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn primary(&mut self) -> Result<Parsed, Diagnostic> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Name(names) => {
                self.take();
                node(ExprKind::Symbol(names), token.span, 0)
            }
            TokenKind::String(value) => {
                self.take();
                node(ExprKind::Literal(Value::String(value)), token.span, 0)
            }
            TokenKind::Literal(value) => {
                self.take();
                node(ExprKind::Literal(value), token.span, 0)
            }
            TokenKind::Number(text) => {
                self.take();
                let kind = match text
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                {
                    Some(value) => ExprKind::Literal(Value::Number(value)),
                    None => ExprKind::OverflowNumber(text),
                };
                node(kind, token.span, 0)
            }
            TokenKind::LeftParen => {
                self.take();
                let inner = self.sequence()?;
                let end = self.expect(&TokenKind::RightParen, ")")?;
                node(
                    ExprKind::Group(Box::new(inner.expr)),
                    token.span.join(end),
                    inner.depth,
                )
            }
            TokenKind::LeftBracket => self.array(),
            TokenKind::LeftBrace => self.object(),
            _ => Err(self.unexpected()),
        }
    }

    fn sequence(&mut self) -> Result<Parsed, Diagnostic> {
        let start = self.current().span.start;
        let mut items = Vec::new();
        let mut depth = 0;
        loop {
            let item_start = self.current().span.start;
            let name = self.assignment_name();
            let value = self.expression(0, true)?;
            depth = depth.max(value.depth);
            if let Some(end) = self.consume(&TokenKind::Semicolon) {
                items.push(Statement {
                    name,
                    expression: value.expr,
                    span: Span::new(item_start, end.end),
                });
            } else if name.is_some() {
                return Err(Diagnostic::new(
                    Category::Syntax,
                    "a sequence must end in an expression, not an assignment",
                    value.expr.span,
                )
                .with_hint("end the local definition with ';', then provide a final expression"));
            } else {
                if items.is_empty() {
                    return Ok(value);
                }
                let span = Span::new(start, value.expr.span.end);
                return node(
                    ExprKind::Sequence {
                        items,
                        value: Box::new(value.expr),
                    },
                    span,
                    depth,
                );
            }
        }
    }

    fn array(&mut self) -> Result<Parsed, Diagnostic> {
        let start = self.take().span;
        let mut values = Vec::new();
        let mut depth = 0;
        while !self.at(&TokenKind::RightBracket) {
            let value = self.sequence()?;
            depth = depth.max(value.depth);
            values.push(value.expr);
            if self.consume(&TokenKind::Comma).is_none() {
                break;
            }
        }
        let end = self.expect(&TokenKind::RightBracket, "]")?;
        node(ExprKind::Array(values), start.join(end), depth)
    }

    fn object(&mut self) -> Result<Parsed, Diagnostic> {
        let start = self.take().span;
        let mut fields = Vec::new();
        let mut depth = 0;
        while !self.at(&TokenKind::RightBrace) {
            let name = match &self.current().kind {
                TokenKind::Name(_) => {
                    let span = self.take().span;
                    self.joined_name(span)
                }
                TokenKind::String(text)
                    if self.source.as_bytes()[self.current().span.start] == b'"' =>
                {
                    let text = text.clone();
                    Name {
                        text,
                        span: self.take().span,
                    }
                }
                _ => return Err(self.unexpected()),
            };
            self.expect(&TokenKind::Colon, ":")?;
            let value = self.sequence()?;
            depth = depth.max(value.depth);
            let span = name.span.join(value.expr.span);
            fields.push(ObjectField {
                name,
                value: value.expr,
                span,
            });
            if self.consume(&TokenKind::Comma).is_none() {
                break;
            }
        }
        let end = self.expect(&TokenKind::RightBrace, "}")?;
        node(ExprKind::Object(fields), start.join(end), depth)
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
