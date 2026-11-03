use crate::types::{FunctionType, Parameter, RecordType, TupleItem, TupleType, Type};

#[derive(Clone, Debug, PartialEq)]
pub struct Builtin {
    pub name: &'static str,
    pub min_arity: usize,
    pub max_arity: usize,
    pub signature: FunctionType,
}

pub fn builtins() -> Vec<Builtin> {
    use Type::{Any, Boolean, String as Str};
    let t = Type::Named("T".into());
    let u = Type::Named("U".into());
    let a = Type::Named("A".into());
    let b = Type::Named("B".into());
    let k = Type::Named("K".into());
    let number = Type::number();
    let predicate = callback([("x", t.clone())], Boolean);
    vec![
        builtin("len", 1, [("xs", Any)], number.clone()),
        builtin(
            "map",
            2,
            [
                ("xs", array(t.clone())),
                ("f", callback([("x", t.clone())], u.clone())),
            ],
            array(u.clone()),
        ),
        builtin(
            "filter",
            2,
            [("xs", array(t.clone())), ("f", predicate.clone())],
            array(t.clone()),
        ),
        builtin(
            "reduce",
            3,
            [
                ("xs", array(t.clone())),
                (
                    "f",
                    callback([("acc", u.clone()), ("x", t.clone())], u.clone()),
                ),
                ("init", u.clone()),
            ],
            u.clone(),
        ),
        builtin(
            "flat_map",
            2,
            [
                ("xs", array(t.clone())),
                ("f", callback([("x", t.clone())], array(u.clone()))),
            ],
            array(u.clone()),
        ),
        builtin(
            "find",
            2,
            [("xs", array(t.clone())), ("f", predicate.clone())],
            Type::Union(vec![t.clone(), Type::Null]),
        ),
        builtin(
            "any",
            2,
            [("xs", array(t.clone())), ("f", predicate.clone())],
            Boolean,
        ),
        builtin(
            "all",
            2,
            [("xs", array(t.clone())), ("f", predicate)],
            Boolean,
        ),
        builtin(
            "sort_by",
            2,
            [
                ("xs", array(t.clone())),
                ("f", callback([("x", t.clone())], k)),
            ],
            array(t.clone()),
        ),
        builtin("reverse", 1, [("xs", array(t.clone()))], array(t.clone())),
        builtin(
            "slice",
            2,
            [
                ("xs", array(t.clone())),
                ("start", number.clone()),
                ("end", number.clone()),
            ],
            array(t.clone()),
        ),
        builtin(
            "concat",
            2,
            [("xs", array(t.clone())), ("ys", array(u.clone()))],
            array(Type::Union(vec![t.clone(), u.clone()])),
        ),
        builtin("unique", 1, [("xs", array(t.clone()))], array(t.clone())),
        builtin(
            "group_by",
            2,
            [
                ("xs", array(t.clone())),
                ("f", callback([("x", t.clone())], Str)),
            ],
            record(array(t.clone())),
        ),
        builtin(
            "zip",
            2,
            [("xs", array(a.clone())), ("ys", array(b.clone()))],
            array(tuple([a, b])),
        ),
        builtin(
            "range",
            1,
            [("start", number.clone()), ("end", number.clone())],
            array(number.clone()),
        ),
        builtin("sum", 1, [("xs", array(number.clone()))], number.clone()),
        builtin("min", 1, [("xs", array(number.clone()))], number.clone()),
        builtin("max", 1, [("xs", array(number.clone()))], number.clone()),
        builtin("first", 1, [("xs", array(t.clone()))], t.clone()),
        builtin("last", 1, [("xs", array(t.clone()))], t.clone()),
        builtin("keys", 1, [("o", record(t.clone()))], array(Str)),
        builtin("values", 1, [("o", record(t.clone()))], array(t.clone())),
        builtin(
            "entries",
            1,
            [("o", record(t.clone()))],
            array(tuple([Str, t.clone()])),
        ),
        builtin(
            "from_entries",
            1,
            [("xs", array(tuple([Str, t.clone()])))],
            record(t.clone()),
        ),
        builtin(
            "merge",
            2,
            [("a", record(t.clone())), ("b", record(u.clone()))],
            record(Type::Union(vec![t.clone(), u])),
        ),
        builtin("has", 2, [("o", record(t)), ("key", Str)], Boolean),
        builtin("str", 1, [("value", Any)], Str),
        builtin("num", 1, [("s", Str)], number.clone()),
        builtin("split", 2, [("s", Str), ("separator", Str)], array(Str)),
        builtin("join", 2, [("xs", array(Str)), ("separator", Str)], Str),
        builtin("trim", 1, [("s", Str)], Str),
        builtin("lower", 1, [("s", Str)], Str),
        builtin("upper", 1, [("s", Str)], Str),
        builtin("contains", 2, [("s", Any), ("part", Any)], Boolean),
        builtin("starts_with", 2, [("s", Str), ("prefix", Str)], Boolean),
        builtin("ends_with", 2, [("s", Str), ("suffix", Str)], Boolean),
        builtin("replace", 3, [("s", Str), ("from", Str), ("to", Str)], Str),
        builtin(
            "substring",
            2,
            [
                ("s", Str),
                ("start", number.clone()),
                ("end", number.clone()),
            ],
            Str,
        ),
        builtin("index_of", 2, [("s", Str), ("part", Str)], number.clone()),
        builtin("lines", 1, [("s", Str)], array(Str)),
        builtin("regex_match", 2, [("s", Str), ("pattern", Str)], Boolean),
        builtin(
            "regex_find_all",
            2,
            [("s", Str), ("pattern", Str)],
            array(array(Str)),
        ),
        builtin(
            "regex_replace",
            3,
            [("s", Str), ("pattern", Str), ("replacement", Str)],
            Str,
        ),
        builtin("floor", 1, [("x", number.clone())], number.clone()),
        builtin("ceil", 1, [("x", number.clone())], number.clone()),
        builtin("round", 1, [("x", number.clone())], number.clone()),
        builtin("abs", 1, [("x", number.clone())], number),
        builtin("parse_json", 1, [("s", Str)], Any),
        builtin("to_json", 1, [("value", Any)], Str),
        builtin(
            "text",
            1,
            [("blocks", array(Type::Named("Content".into())))],
            Str,
        ),
        builtin("type_of", 1, [("value", Any)], Str),
    ]
}

fn builtin<const N: usize>(
    name: &'static str,
    min_arity: usize,
    parameters: [(&str, Type); N],
    result: Type,
) -> Builtin {
    Builtin {
        name,
        min_arity,
        max_arity: N,
        signature: signature(parameters, result),
    }
}

fn signature<const N: usize>(parameters: [(&str, Type); N], result: Type) -> FunctionType {
    FunctionType {
        parameters: parameters
            .into_iter()
            .map(|(name, ty)| Parameter {
                name: name.into(),
                ty,
            })
            .collect(),
        result: Box::new(result),
    }
}

fn callback<const N: usize>(parameters: [(&str, Type); N], result: Type) -> Type {
    Type::Function(signature(parameters, result))
}

fn array(item: Type) -> Type {
    Type::Array(Box::new(item))
}

fn record(value: Type) -> Type {
    Type::Record(RecordType {
        fields: vec![],
        additional: Some(Box::new(value)),
    })
}

fn tuple<const N: usize>(items: [Type; N]) -> Type {
    Type::Tuple(TupleType {
        items: items
            .into_iter()
            .map(|ty| TupleItem {
                ty,
                optional: false,
            })
            .collect(),
        rest: None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_matches_documented_signatures_and_arities() {
        let expected = [
            ("len", 1, 1, "(xs: any) => number"),
            ("map", 2, 2, "(xs: T[], f: (x: T) => U) => U[]"),
            ("filter", 2, 2, "(xs: T[], f: (x: T) => boolean) => T[]"),
            (
                "reduce",
                3,
                3,
                "(xs: T[], f: (acc: U, x: T) => U, init: U) => U",
            ),
            ("flat_map", 2, 2, "(xs: T[], f: (x: T) => U[]) => U[]"),
            ("find", 2, 2, "(xs: T[], f: (x: T) => boolean) => T | null"),
            ("any", 2, 2, "(xs: T[], f: (x: T) => boolean) => boolean"),
            ("all", 2, 2, "(xs: T[], f: (x: T) => boolean) => boolean"),
            ("sort_by", 2, 2, "(xs: T[], f: (x: T) => K) => T[]"),
            ("reverse", 1, 1, "(xs: T[]) => T[]"),
            (
                "slice",
                2,
                3,
                "(xs: T[], start: number, end: number) => T[]",
            ),
            ("concat", 2, 2, "(xs: T[], ys: U[]) => (T | U)[]"),
            ("unique", 1, 1, "(xs: T[]) => T[]"),
            (
                "group_by",
                2,
                2,
                "(xs: T[], f: (x: T) => string) => {[key: string]: T[]}",
            ),
            ("zip", 2, 2, "(xs: A[], ys: B[]) => [A, B][]"),
            ("range", 1, 2, "(start: number, end: number) => number[]"),
            ("sum", 1, 1, "(xs: number[]) => number"),
            ("min", 1, 1, "(xs: number[]) => number"),
            ("max", 1, 1, "(xs: number[]) => number"),
            ("first", 1, 1, "(xs: T[]) => T"),
            ("last", 1, 1, "(xs: T[]) => T"),
            ("keys", 1, 1, "(o: {[key: string]: T}) => string[]"),
            ("values", 1, 1, "(o: {[key: string]: T}) => T[]"),
            ("entries", 1, 1, "(o: {[key: string]: T}) => [string, T][]"),
            (
                "from_entries",
                1,
                1,
                "(xs: [string, T][]) => {[key: string]: T}",
            ),
            (
                "merge",
                2,
                2,
                "(a: {[key: string]: T}, b: {[key: string]: U}) => {[key: string]: T | U}",
            ),
            (
                "has",
                2,
                2,
                "(o: {[key: string]: T}, key: string) => boolean",
            ),
            ("str", 1, 1, "(value: any) => string"),
            ("num", 1, 1, "(s: string) => number"),
            ("split", 2, 2, "(s: string, separator: string) => string[]"),
            ("join", 2, 2, "(xs: string[], separator: string) => string"),
            ("trim", 1, 1, "(s: string) => string"),
            ("lower", 1, 1, "(s: string) => string"),
            ("upper", 1, 1, "(s: string) => string"),
            ("contains", 2, 2, "(s: any, part: any) => boolean"),
            (
                "starts_with",
                2,
                2,
                "(s: string, prefix: string) => boolean",
            ),
            ("ends_with", 2, 2, "(s: string, suffix: string) => boolean"),
            (
                "replace",
                3,
                3,
                "(s: string, from: string, to: string) => string",
            ),
            (
                "substring",
                2,
                3,
                "(s: string, start: number, end: number) => string",
            ),
            ("index_of", 2, 2, "(s: string, part: string) => number"),
            ("lines", 1, 1, "(s: string) => string[]"),
            (
                "regex_match",
                2,
                2,
                "(s: string, pattern: string) => boolean",
            ),
            (
                "regex_find_all",
                2,
                2,
                "(s: string, pattern: string) => string[][]",
            ),
            (
                "regex_replace",
                3,
                3,
                "(s: string, pattern: string, replacement: string) => string",
            ),
            ("floor", 1, 1, "(x: number) => number"),
            ("ceil", 1, 1, "(x: number) => number"),
            ("round", 1, 1, "(x: number) => number"),
            ("abs", 1, 1, "(x: number) => number"),
            ("parse_json", 1, 1, "(s: string) => any"),
            ("to_json", 1, 1, "(value: any) => string"),
            ("text", 1, 1, "(blocks: Content[]) => string"),
            ("type_of", 1, 1, "(value: any) => string"),
        ];
        let catalog = builtins();
        assert_eq!(catalog.len(), 52);
        let names: HashSet<_> = catalog.iter().map(|builtin| builtin.name).collect();
        assert_eq!(names.len(), catalog.len());
        for (name, min, max, signature) in expected {
            let builtin = catalog.iter().find(|builtin| builtin.name == name).unwrap();
            assert_eq!((builtin.min_arity, builtin.max_arity), (min, max), "{name}");
            assert_eq!(builtin.signature.parameters.len(), max, "{name}");
            assert_eq!(builtin.signature.to_string(), signature, "{name}");
        }
        assert!(!names.contains("help"));
        assert!(!names.contains("show"));
    }
}
