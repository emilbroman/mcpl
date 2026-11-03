use mcpl::{
    frontend::{Category, Diagnostic, Span},
    runtime::{Budget, Value, ValueKind, builtins, limits::MAX_STRING},
    types::Type,
};
use serde_json::{Value as Json, json};

fn values(args: Vec<Json>) -> Vec<Value> {
    args.into_iter()
        .map(|v| Value::from_json(&v, &mut Budget::default(), Span::default()).unwrap())
        .collect()
}

fn run(name: &str, args: Vec<Json>) -> Result<Value, Diagnostic> {
    builtins::call(name, values(args), &mut Budget::default(), Span::default())
}

fn expect(name: &str, args: Vec<Json>, expected: Json) {
    let result = run(name, args).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!(
        result
            .to_json(&mut Budget::default(), Span::default(), false)
            .unwrap(),
        expected,
        "{name}"
    );
}

#[test]
fn pure_collection_builtins() {
    expect("len", vec![json!([1, 2])], json!(2.0));
    expect("len", vec![json!({"x": 1})], json!(1.0));
    expect("reverse", vec![json!([1, 2, 3])], json!([3.0, 2.0, 1.0]));
    expect("slice", vec![json!([1, 2, 3]), json!(1)], json!([2.0, 3.0]));
    expect(
        "slice",
        vec![json!([1, 2, 3]), json!(2), json!(1)],
        json!([]),
    );
    expect("slice", vec![json!([1]), json!(1e100)], json!([]));
    expect("concat", vec![json!([1]), json!(["x"])], json!([1.0, "x"]));
    expect(
        "unique",
        vec![json!([{"a":1,"b":2}, {"b":2,"a":1}, null, null])],
        json!([{"a":1.0,"b":2.0}, null]),
    );
    expect(
        "zip",
        vec![json!([1, 2]), json!(["a"])],
        json!([[1.0, "a"]]),
    );
    expect("range", vec![json!(3)], json!([0.0, 1.0, 2.0]));
    expect(
        "range",
        vec![json!(-2), json!(2)],
        json!([-2.0, -1.0, 0.0, 1.0]),
    );
    expect("range", vec![json!(4), json!(1)], json!([]));
    expect(
        "range",
        vec![
            json!(9_007_199_254_740_990i64),
            json!(9_007_199_254_740_991i64),
        ],
        json!([9_007_199_254_740_990.0]),
    );
    expect("sum", vec![json!([2, -1, 3])], json!(4.0));
    expect("sum", vec![json!([])], json!(0.0));
    expect("min", vec![json!([2, -1, 3])], json!(-1.0));
    expect("max", vec![json!([2, -1, 3])], json!(3.0));
    expect("first", vec![json!(["x", "y"])], json!("x"));
    expect("last", vec![json!(["x", "y"])], json!("y"));
    assert_eq!(
        run("zip", vec![json!([1]), json!(["x"])])
            .unwrap()
            .as_array(Span::default())
            .unwrap()[0]
            .display_type()
            .to_string(),
        "[number, string]"
    );
}

#[test]
fn object_builtins_preserve_insertion_order() {
    expect("keys", vec![json!({"z":1,"a":2})], json!(["z", "a"]));
    expect("values", vec![json!({"z":1,"a":2})], json!([1.0, 2.0]));
    expect(
        "entries",
        vec![json!({"z":1,"a":2})],
        json!([["z", 1.0], ["a", 2.0]]),
    );
    let entries = run("entries", vec![json!({"z":1})]).unwrap();
    assert_eq!(
        entries.as_array(Span::default()).unwrap()[0]
            .display_type()
            .to_string(),
        "[string, number]"
    );
    let object = run(
        "from_entries",
        vec![json!([["z", 1], ["a", 2], ["z", 3], ["__proto__", null]])],
    )
    .unwrap();
    assert_eq!(
        object
            .as_object(Span::default())
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>(),
        ["z", "a", "__proto__"]
    );
    assert_eq!(
        object
            .to_json(&mut Budget::default(), Span::default(), false)
            .unwrap(),
        json!({"z":3.0,"a":2.0,"__proto__":null})
    );
    expect(
        "merge",
        vec![json!({"z":1,"a":2}), json!({"z":3,"b":4})],
        json!({"z":3.0,"a":2.0,"b":4.0}),
    );
    expect("has", vec![json!({"a":null}), json!("a")], json!(true));
    expect("has", vec![json!({"a":null}), json!("b")], json!(false));
}

#[test]
fn strings_use_unicode_scalars_and_literal_replacement() {
    expect("len", vec![json!("é🙂")], json!(2.0));
    expect("str", vec![json!("x")], json!("x"));
    expect("str", vec![json!(1)], json!("1"));
    expect("str", vec![json!(true)], json!("true"));
    expect("str", vec![json!(null)], json!("null"));
    expect("str", vec![json!([true, null])], json!("[true,null]"));
    expect("num", vec![json!("-12.5e+2")], json!(-1250.0));
    expect(
        "split",
        vec![json!("a,,b,"), json!(",")],
        json!(["a", "", "b", ""]),
    );
    expect("split", vec![json!("é🙂"), json!("")], json!(["é", "🙂"]));
    expect("split", vec![json!(""), json!("")], json!([]));
    expect("join", vec![json!(["é", "🙂"]), json!("-")], json!("é-🙂"));
    expect("join", vec![json!([]), json!("-")], json!(""));
    expect("trim", vec![json!("\u{2003}x\u{85}")], json!("x"));
    expect("lower", vec![json!("ΟΣİ")], json!("οσi\u{307}"));
    expect("lower", vec![json!("ΟΣ")], json!("ος"));
    expect("upper", vec![json!("straße")], json!("STRASSE"));
    expect("contains", vec![json!("é🙂"), json!("🙂")], json!(true));
    expect(
        "contains",
        vec![json!([{ "a": [1] }]), json!({"a":[1]})],
        json!(true),
    );
    expect("contains", vec![json!("a"), json!("")], json!(true));
    expect("starts_with", vec![json!("é🙂"), json!("é")], json!(true));
    expect("ends_with", vec![json!("é🙂"), json!("🙂")], json!(true));
    expect(
        "replace",
        vec![json!("aaaa"), json!("aa"), json!("$1")],
        json!("$1$1"),
    );
    expect(
        "substring",
        vec![json!("aé🙂z"), json!(1), json!(3)],
        json!("é🙂"),
    );
    expect("substring", vec![json!("é🙂"), json!(1)], json!("🙂"));
    expect(
        "substring",
        vec![json!("é🙂"), json!(2), json!(1)],
        json!(""),
    );
    expect("index_of", vec![json!("é🙂z"), json!("z")], json!(2.0));
    expect("index_of", vec![json!("é🙂z"), json!("x")], json!(-1.0));
    expect(
        "lines",
        vec![json!("a\r\nb\nc\r\n")],
        json!(["a", "b", "c"]),
    );
    expect("lines", vec![json!("a\r")], json!(["a\r"]));
    expect("lines", vec![json!("\n")], json!([""]));
    expect("lines", vec![json!("")], json!([]));
}

#[test]
fn regexes_are_linear_engine_with_optional_and_empty_captures() {
    expect(
        "regex_match",
        vec![json!("a12"), json!(r"\d+")],
        json!(true),
    );
    expect(
        "regex_match",
        vec![json!("a12"), json!(r"^\d+$")],
        json!(false),
    );
    expect(
        "regex_find_all",
        vec![json!("ab b"), json!("(a)?b")],
        json!([["ab", "a"], ["b", ""]]),
    );
    expect(
        "regex_find_all",
        vec![json!("é"), json!("")],
        json!([[""], [""]]),
    );
    expect("regex_find_all", vec![json!("abc"), json!("z")], json!([]));
    expect(
        "regex_replace",
        vec![json!("a12b3"), json!(r"\d+"), json!("$1")],
        json!("a$1b$1"),
    );
    expect(
        "regex_replace",
        vec![json!("é"), json!(""), json!("_")],
        json!("_é_"),
    );
    assert!(run("regex_match", vec![json!("a"), json!("(?=a)")]).is_err());
    assert!(run("regex_find_all", vec![json!("a"), json!(r"(a)\1")]).is_err());
}

#[test]
fn math_json_text_and_types() {
    expect("floor", vec![json!(-1.2)], json!(-2.0));
    expect("ceil", vec![json!(-1.2)], json!(-1.0));
    expect("round", vec![json!(-1.5)], json!(-2.0));
    expect("round", vec![json!(1.5)], json!(2.0));
    expect("abs", vec![json!(-1.5)], json!(1.5));
    expect(
        "parse_json",
        vec![json!(" {\"z\":1,\"a\":[true,null]} ")],
        json!({"z":1.0,"a":[true,null]}),
    );
    expect("to_json", vec![json!("a\n\"b")], json!("\"a\\n\\\"b\""));
    expect(
        "to_json",
        vec![json!({"z":true,"a":null})],
        json!("{\"z\":true,\"a\":null}"),
    );
    expect(
        "text",
        vec![
            json!([{"type":"image","data":"ignored"},{"type":"text","text":"a"},{"type":"text","text":"b"}]),
        ],
        json!("a\nb"),
    );
    expect("text", vec![json!([])], json!(""));
    expect("type_of", vec![json!("abc")], json!("string"));
    let parsed = run("parse_json", vec![json!("{\"z\":1,\"a\":2,\"z\":3}")]).unwrap();
    assert_eq!(
        parsed
            .as_object(Span::default())
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>(),
        ["z", "a"]
    );
}

#[test]
fn gradual_type_and_argument_errors() {
    for (name, args) in [
        ("slice", vec![json!([]), json!(-1)]),
        ("substring", vec![json!("x"), json!(0.1)]),
        ("range", vec![json!(-1)]),
        ("range", vec![json!(9_007_199_254_740_992i64)]),
        ("range", vec![json!(0.1), json!(1)]),
        ("sum", vec![json!([1, "x"])]),
        ("sum", vec![json!([1e308, 1e308])]),
        ("min", vec![json!([])]),
        ("max", vec![json!([])]),
        ("first", vec![json!([])]),
        ("last", vec![json!([])]),
        ("from_entries", vec![json!([[1, 2]])]),
        ("from_entries", vec![json!([["x", 1, 2]])]),
        ("join", vec![json!([1]), json!(",")]),
        ("replace", vec![json!("x"), json!(""), json!("y")]),
        ("text", vec![json!([{"type":"text","text":1}])]),
        ("text", vec![json!([true])]),
        ("contains", vec![json!("x"), json!(1)]),
        ("len", vec![json!(true)]),
    ] {
        assert_eq!(
            run(name, args).unwrap_err().category,
            Category::Runtime,
            "{name}"
        );
    }
    for s in [
        "", " 1", "1 ", "+1", "01", ".1", "1.", "1e", "NaN", "Infinity", "0x1", "1e999",
    ] {
        assert!(run("num", vec![json!(s)]).is_err(), "{s}");
    }
    for s in [
        "{x:1}",
        "[1,]",
        "[/*x*/1]",
        "1e999",
        "true false",
        "\"\\uD800\"",
    ] {
        assert!(run("parse_json", vec![json!(s)]).is_err(), "{s}");
    }
    for name in [
        "len",
        "reverse",
        "slice",
        "concat",
        "unique",
        "zip",
        "range",
        "sum",
        "min",
        "max",
        "first",
        "last",
        "keys",
        "values",
        "entries",
        "from_entries",
        "merge",
        "has",
        "str",
        "num",
        "split",
        "join",
        "trim",
        "lower",
        "upper",
        "contains",
        "starts_with",
        "ends_with",
        "replace",
        "substring",
        "index_of",
        "lines",
        "regex_match",
        "regex_find_all",
        "regex_replace",
        "floor",
        "ceil",
        "round",
        "abs",
        "parse_json",
        "to_json",
        "text",
        "type_of",
    ] {
        assert!(run(name, vec![]).is_err(), "{name}");
        assert!(run(name, vec![json!(null); 4]).is_err(), "{name}");
    }
}

#[test]
fn capability_containers_render_types_and_equality_fails() {
    let span = Span::default();
    let value = Value::new(
        ValueKind::Server("s".into()),
        Type::String,
        &mut Budget::default(),
        span,
    )
    .unwrap();
    for name in ["str", "to_json", "type_of"] {
        assert_eq!(
            builtins::call(name, vec![value.clone()], &mut Budget::default(), span)
                .unwrap()
                .as_str(span)
                .unwrap(),
            "string"
        );
    }
    let nested = Value::array(vec![value.clone()], &mut Budget::default(), span).unwrap();
    for name in ["str", "to_json"] {
        assert_eq!(
            builtins::call(name, vec![nested.clone()], &mut Budget::default(), span)
                .unwrap()
                .as_str(span)
                .unwrap(),
            "string[]"
        );
    }
    assert!(builtins::call("unique", vec![nested.clone()], &mut Budget::default(), span).is_err());
    assert!(
        builtins::call(
            "contains",
            vec![nested, Value::null()],
            &mut Budget::default(),
            span
        )
        .is_err()
    );
}

#[test]
fn bounds_and_fuel_are_checked_before_large_outputs() {
    let span = Span::default();
    assert_eq!(
        run("range", vec![json!(100_001)]).unwrap_err().category,
        Category::Limit
    );
    let input = Value::string("x".repeat(100_001), &mut Budget::default(), span).unwrap();
    let empty = Value::string(String::new(), &mut Budget::default(), span).unwrap();
    let error =
        builtins::call("split", vec![input, empty], &mut Budget::default(), span).unwrap_err();
    assert!(error.message.contains("collection element"), "{error:?}");
    let input = format!("[{}]", vec!["0"; 100_001].join(","));
    let error = run("parse_json", vec![json!(input)]).unwrap_err();
    assert!(error.message.contains("collection element"), "{error:?}");
    let input = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    assert_eq!(
        run("parse_json", vec![json!(input)]).unwrap_err().category,
        Category::Limit
    );
    let small = Value::string("aa".into(), &mut Budget::default(), span).unwrap();
    let search = Value::string("a".into(), &mut Budget::default(), span).unwrap();
    let huge = Value::string("x".repeat(MAX_STRING / 2 + 1), &mut Budget::default(), span).unwrap();
    let error = builtins::call(
        "replace",
        vec![small, search, huge],
        &mut Budget::default(),
        span,
    )
    .unwrap_err();
    assert!(error.message.contains("string length"), "{error:?}");
    let args = values(vec![json!("abc"), json!("z")]);
    let mut budget = Budget {
        remaining: 1,
        calls: 0,
        ..Budget::default()
    };
    assert_eq!(
        builtins::call("contains", args, &mut budget, span)
            .unwrap_err()
            .category,
        Category::Limit
    );
    let range = run("range", vec![json!(100_000)]).unwrap();
    assert_eq!(range.as_array(span).unwrap().len(), 100_000);
}

#[test]
fn downstream_decoded_strings_check_length_before_fuel_scan() {
    let json = serde_json::Value::String("x".repeat(16 * 1024 * 1024 + 1));
    let error = Value::from_json(&json, &mut Budget::default(), Span::default()).unwrap_err();
    assert_eq!(error.category, Category::Limit);
    assert!(error.message.contains("string length"));
}
