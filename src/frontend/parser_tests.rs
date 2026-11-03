use super::*;

fn expression(source: &str) -> Expr {
    parse_expression(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"))
}

#[test]
fn accepts_complete_grammar_examples() {
    for source in [
        "",
        " // empty\r\n",
        "x=1; x;",
        "a.b=2; a.b;",
        "s.2fa-check({});",
        "server.-tool({}); server.true;",
        "x = -5; y = 1e-3; z = 2E+4;",
        "a-b; -a-b; - (a + b); !ok;",
        "f = () => null; f();",
        "f = (x, y.z) => x;",
        "f = (x, x) => x;",
        "x = [1, 2,]; y = {a.b: 1, \"true\": 2,};",
        "(snap = text(s.read({})); {url: snap[0], count: len(snap)});",
        "f(a = 1; a + 1, (x) => x);",
        "[x = 1; x, 2; 3]; {a: x = 1; x, b: 2; 3};",
        "f = (x) => (y = x + 1; y * 2);",
        "a ? (x = 1; x) : (() => null);",
    ] {
        parse_program(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
    }
}

#[test]
fn rejects_out_of_grammar_forms() {
    for source in [
        "x = 1",
        "1;;",
        ";",
        "let x = 1;",
        "x = a = 1; a + 1;",
        "(x = 1);",
        "(x = 1;);",
        "(1;);",
        "();",
        "[x = 1];",
        "f(x = 1);",
        "f(1,);",
        "f(,1);",
        "(x,) => x;",
        "(x: number) => x;",
        "(x = 1) => x;",
        "x => x;",
        "(true) => true;",
        "a + (x) => x;",
        "a ? x = 1; x : 2;",
        "a[b = 1; b];",
        "(x) => y = 1; y;",
        "x .y;",
        "x. y;",
        "x . y;",
        "x..y;",
        "x.;",
        ".x;",
        "true.x;",
        "{true: 1};",
        "{null: 1};",
        "{false: 1};",
        "{[x]: 1};",
        "{r\"key\": 1};",
        "{x};",
        "{x: 1,,};",
        "[1,,];",
        "[...x];",
        "f(x: 1);",
        "1 % 2;",
        "a ?? b;",
        "a !== b;",
        "'hello';",
        "`hello`;",
        "f /* not a block comment */;",
        "a\u{a0}+\u{a0}b;",
        "π;",
        "变量;",
        "+1;",
        "01;",
        "00;",
        ".5;",
        "1.;",
        "1e;",
        "1e+;",
        "1e-;",
        "1_000;",
        "0x10;",
        "2fa-check = 1;",
        "1e3x;",
        "1-2;",
    ] {
        assert!(
            parse_program(source).is_err(),
            "unexpected success: {source:?}"
        );
    }
}

#[test]
fn strings_decode_json_scalars_and_raw_text() {
    for (source, expected) in [
        (r#""héllo 世界 🦀""#, "héllo 世界 🦀"),
        (r#""\uD83E\uDD80""#, "🦀"),
        (r#""\"\\\/\b\f\n\r\t\u0041""#, "\"\\/\u{8}\u{c}\n\r\tA"),
        (r#"r"\d+\n""#, "\\d+\\n"),
        ("r\"one\ntwo\"", "one\ntwo"),
        ("r\"\u{0}\"", "\u{0}"),
    ] {
        assert_eq!(
            expression(source).kind,
            ExprKind::Literal(Value::String(expected.into()))
        );
    }
    for source in [
        r#""\d""#,
        r#""\x41""#,
        r#""\0""#,
        r#""\v""#,
        r#""\u{41}""#,
        r#""\uD800""#,
        r#""\uDC00""#,
        r#""\uD800\u0041""#,
        "\"line\nbreak\"",
        "\"\u{0}\"",
        "\"unterminated",
        "r\"unterminated",
    ] {
        assert!(
            parse_expression(source).is_err(),
            "unexpected success: {source:?}"
        );
    }
}

#[test]
fn surrogate_errors_suggest_pairing() {
    for source in [r#""\uD800""#, r#""\uDC00""#, r#""\uD800\u0041""#] {
        let error = parse_expression(source).unwrap_err();
        assert!(error.hint.unwrap().contains("surrogate pair"));
    }
}

#[test]
fn numeric_values_and_overflow() {
    for (source, expected) in [
        ("0", 0.0),
        ("1e-3", 0.001),
        ("2E+4", 20_000.0),
        ("1.25", 1.25),
    ] {
        let ExprKind::Literal(Value::Number(number)) = expression(source).kind else {
            panic!("not a number");
        };
        assert_eq!(number.as_f64(), Some(expected));
    }
    assert!(matches!(
        expression("1e9999").kind,
        ExprKind::OverflowNumber(_)
    ));
    assert!(matches!(
        expression("-1").kind,
        ExprKind::Unary {
            op: UnaryOp::Negate,
            ..
        }
    ));
    assert!(matches!(expression("Infinity").kind, ExprKind::Symbol(_)));
    assert!(matches!(expression("NaN").kind, ExprKind::Symbol(_)));
}

#[test]
fn binary_operator_whitespace_is_immediate_and_json_only() {
    for source in [
        "a==b", "a ==b", "a== b", "a +b", "a+ b", "a -b", "a /b", "a *b", "a ||b", "a &&b",
    ] {
        let error = parse_expression(source).unwrap_err();
        assert!(
            error.message.contains("requires whitespace on both sides"),
            "{source}: {error:?}"
        );
    }
    for source in [
        "a\t==\nb",
        "a // comment\n == b",
        "a + // comment\n b",
        "a\r+\tb",
    ] {
        assert!(parse_expression(source).is_ok(), "{source}");
    }
    assert!(parse_expression("a +// comment\nb").is_err());
    assert!(matches!(expression("a-b").kind, ExprKind::Symbol(_)));
    assert!(matches!(expression("-a-b").kind, ExprKind::Unary { .. }));
}

#[test]
fn stable_syntax_diagnostics() {
    let missing = parse_program("x = 1").unwrap_err();
    assert_eq!(missing.category, Category::Syntax);
    assert_eq!(missing.message, "expected ';' after statement");
    assert_eq!(missing.span, Span::new(5, 5));
    assert_eq!(
        missing.hint.as_deref(),
        Some("every statement must end with ';'")
    );
    let whitespace = parse_program("a==b;").unwrap_err();
    assert_eq!(
        whitespace.message,
        "binary operator '==' requires whitespace on both sides"
    );
    assert_eq!(whitespace.span, Span::new(1, 3));
    assert_eq!(
        whitespace.hint.as_deref(),
        Some("add whitespace on both sides of '=='")
    );
    let assignment = parse_program("x = a = 1; a + 1;").unwrap_err();
    assert_eq!(assignment.message, "unexpected '='");
    assert_eq!(assignment.span, Span::new(6, 7));
    assert_eq!(
        assignment.hint.as_deref(),
        Some("to sequence inside an expression, wrap it in parentheses: (a = 1; a + 1)")
    );
}

#[test]
fn earlier_parse_error_wins_over_later_lexical_error() {
    let error = parse_program("x = ; @").unwrap_err();
    assert_eq!(error.span, Span::new(4, 5));
    assert_eq!(error.message, "unexpected ';'");
}

#[test]
fn symbols_members_and_keys_keep_exact_spans() {
    let parsed = expression(" // note\nserver.2fa-check.true");
    let ExprKind::Symbol(names) = parsed.kind else {
        panic!("not a symbol");
    };
    assert_eq!(
        names,
        vec![
            Name {
                text: "server".into(),
                span: Span::new(9, 15)
            },
            Name {
                text: "2fa-check".into(),
                span: Span::new(16, 25)
            },
            Name {
                text: "true".into(),
                span: Span::new(26, 30)
            },
        ]
    );
    let ExprKind::Member { members, .. } = expression("(x).a.-b").kind else {
        panic!("not member access");
    };
    assert_eq!(members[0].span, Span::new(4, 5));
    assert_eq!(members[1].text, "-b");
    let ExprKind::Object(fields) = expression("{a.b: 1, \"é\": 2}").kind else {
        panic!("not an object");
    };
    assert_eq!(fields[0].name.text, "a.b");
    assert_eq!(fields[1].name.text, "é");
    assert_eq!(fields[1].name.span, Span::new(9, 13));
    assert!(parse_expression(&"a".repeat(128)).is_ok());
    assert!(parse_expression(&"a".repeat(129)).is_err());
}

#[test]
fn precedence_and_associativity() {
    let ExprKind::Binary {
        op: BinaryOp::Or,
        left,
        right,
    } = expression("a == b && c < d + e * f || g").kind
    else {
        panic!("or");
    };
    assert!(matches!(right.kind, ExprKind::Symbol(_)));
    let ExprKind::Binary {
        op: BinaryOp::And,
        left: equal,
        right: less,
    } = left.kind
    else {
        panic!("and");
    };
    assert!(matches!(
        equal.kind,
        ExprKind::Binary {
            op: BinaryOp::Equal,
            ..
        }
    ));
    let ExprKind::Binary {
        op: BinaryOp::Less,
        right: add,
        ..
    } = less.kind
    else {
        panic!("less");
    };
    let ExprKind::Binary {
        op: BinaryOp::Add,
        right: multiply,
        ..
    } = add.kind
    else {
        panic!("add");
    };
    assert!(matches!(
        multiply.kind,
        ExprKind::Binary {
            op: BinaryOp::Multiply,
            ..
        }
    ));
    let ExprKind::Binary {
        op: BinaryOp::Divide,
        left,
        ..
    } = expression("8 / 4 / 2").kind
    else {
        panic!("division");
    };
    assert!(matches!(
        left.kind,
        ExprKind::Binary {
            op: BinaryOp::Divide,
            ..
        }
    ));
    let ExprKind::Conditional { no, .. } = expression("a ? b : c ? d : e").kind else {
        panic!("conditional");
    };
    assert!(matches!(no.kind, ExprKind::Conditional { .. }));
    let ExprKind::Unary { operand, .. } = expression("!-f(1)[0].x").kind else {
        panic!("unary");
    };
    let ExprKind::Unary { operand, .. } = operand.kind else {
        panic!("negate");
    };
    assert!(matches!(operand.kind, ExprKind::Member { .. }));
    let ExprKind::Lambda { body, .. } = expression("(x) => x + 1").kind else {
        panic!("lambda");
    };
    assert!(matches!(
        body.kind,
        ExprKind::Binary {
            op: BinaryOp::Add,
            ..
        }
    ));
}

#[test]
fn sequence_items_exclude_the_final_value_and_groups_survive() {
    let ExprKind::Group(inner) = expression("(a = 1; f(a); a + 2)").kind else {
        panic!("group");
    };
    let ExprKind::Sequence { items, value } = inner.kind else {
        panic!("sequence");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name.as_ref().unwrap().text, "a");
    assert!(items[1].name.is_none());
    assert!(matches!(value.kind, ExprKind::Binary { .. }));
    assert!(matches!(expression("(1)").kind, ExprKind::Group(_)));
    assert!(parse_expression("1;").is_err());
    assert!(parse_expression("x = 1").is_err());
    assert!(parse_expression(" // leading\n 1 // trailing").is_ok());
}

#[test]
fn source_limit_counts_utf8_bytes() {
    assert!(parse_program(&" ".repeat(SOURCE_LIMIT)).is_ok());
    assert_eq!(
        parse_program(&" ".repeat(SOURCE_LIMIT + 1))
            .unwrap_err()
            .category,
        Category::Limit
    );
    let multibyte = format!("//{}", "é".repeat(SOURCE_LIMIT / 2));
    assert_eq!(
        parse_program(&multibyte).unwrap_err().category,
        Category::Limit
    );
}

#[test]
fn nesting_limits_cover_recursive_and_iterative_ast_shapes() {
    let grouped = format!("{}x{}", "(".repeat(255), ")".repeat(255));
    assert!(parse_expression(&grouped).is_ok());
    let nested = format!("{}x{}", "(".repeat(256), ")".repeat(256));
    assert_eq!(
        parse_expression(&nested).unwrap_err().category,
        Category::Limit
    );
    for source in [
        format!("{}x", "!".repeat(256)),
        format!("x{}", " + x".repeat(256)),
        format!("x{}", "()".repeat(256)),
        format!("x{}", "[0]".repeat(256)),
        format!("{}0{}", "[".repeat(256), "]".repeat(256)),
        format!("{}0{}", "{x:".repeat(256), "}".repeat(256)),
        format!("{}0", "() => ".repeat(256)),
        format!("{}0", "x ? 1 : ".repeat(256)),
    ] {
        assert_eq!(
            parse_expression(&source).unwrap_err().category,
            Category::Limit,
            "{source}"
        );
    }
    for source in [
        format!("{}x", "!".repeat(255)),
        format!("x{}", " + x".repeat(255)),
        format!("x{}", "()".repeat(255)),
    ] {
        assert!(parse_expression(&source).is_ok(), "{source}");
    }
}

#[test]
fn adversarial_inputs_return_without_host_stack_overflow() {
    for source in [
        "(".repeat(32_000),
        "!".repeat(32_000),
        "x + ".repeat(12_000),
        "x()".to_owned() + &"()".repeat(24_000),
        "[".repeat(32_000),
    ] {
        assert!(parse_expression(&source).is_err());
    }
}
