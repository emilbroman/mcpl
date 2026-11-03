use mcpl::{
    frontend::{Category, Environment},
    help::render_help,
    registry::Registry,
    types::Type,
};

#[test]
fn overview_has_six_groups_and_every_builtin() {
    let environment = Environment::default();
    let registry = Registry::default();
    let text = render_help(None, &registry, &environment).unwrap();
    assert_eq!(
        text,
        render_help(Some(""), &registry, &environment).unwrap()
    );
    assert!(text.starts_with("Servers\n  (none configured)\n\nBuilt-ins\n"));
    for group in [
        "Collections",
        "Objects",
        "Strings",
        "Math",
        "JSON and MCP",
        "Meta",
    ] {
        assert!(text.contains(&format!("  {group}: ")));
    }
    for name in environment.builtins.keys() {
        assert!(
            text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|word| word == name)
        );
    }
    assert!(!text.contains("Content ="));
}

#[test]
fn all_52_builtins_have_documentation_not_just_signatures() {
    let environment = Environment::default();
    assert_eq!(environment.builtins.len(), 52);
    for name in environment.builtins.keys() {
        let text = render_help(Some(name), &Registry::default(), &environment).unwrap();
        assert!(text.starts_with(&format!("{name}(")), "{name}: {text}");
        assert!(text.contains("\n\n"), "{name}: {text}");
        assert!(
            text.contains("All operations enforce the"),
            "{name}: {text}"
        );
        assert!(!text.ends_with("… (output limit)"));
    }
}

#[test]
fn overloads_and_optional_arguments_are_not_lost_to_frontend_approximations() {
    let environment = Environment::default();
    let registry = Registry::default();
    let help = |name| render_help(Some(name), &registry, &environment).unwrap();
    let len = help("len");
    assert!(len.contains(
        "len(xs: T[]) => number\nlen(s: string) => number\nlen(o: {[key: string]: T}) => number"
    ));
    assert!(help("contains").contains("contains(xs: T[], value: T) => boolean"));
    assert!(
        help("range").contains(
            "range(n: number) => number[]\nrange(start: number, end: number) => number[]"
        )
    );
    assert!(help("slice").contains("end?: number"));
    assert!(help("substring").contains("end?: number"));
    assert!(help("reduce").contains("The argument order is `xs, f, init`."));
    assert!(help("regex_find_all").contains("unmatched optional captures become"));
    assert!(help("range").contains("2^53"));
    assert!(help("from_entries").contains("without moving the original key's position"));
    assert!(help("to_json").contains("Containers holding them are also not JSON"));
    assert!(help("text").contains("text(blocks: Content[]) => string"));
    assert!(!help("map").contains("Content ="));
}

#[test]
fn unknown_topics_are_type_errors_with_valid_suggestions() {
    let environment = Environment::default();
    let registry = Registry::default();
    let diagnostic = render_help(Some("reduc"), &registry, &environment).unwrap_err();
    assert_eq!(diagnostic.category, Category::Type);
    assert_eq!(diagnostic.hint.as_deref(), Some("did you mean 'reduce'?"));
    for expression in ["len([])", "(map)", "values[0]", "unknown.tool"] {
        assert!(render_help(Some(expression), &registry, &environment).is_err());
    }
}

#[test]
fn help_does_not_mutate_bindings_reserved_names_or_result_counter() {
    let mut environment = Environment::default();
    environment.bind("saved", Type::String);
    environment.reserved.insert("evicted".into());
    environment.next_result = 42;
    let previous = format!("{environment:?}");
    render_help(None, &Registry::default(), &environment).unwrap();
    render_help(Some("len"), &Registry::default(), &environment).unwrap();
    assert!(render_help(Some("saved"), &Registry::default(), &environment).is_err());
    assert_eq!(format!("{environment:?}"), previous);
}
