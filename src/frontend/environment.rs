use std::collections::{BTreeMap, BTreeSet};

use crate::types::{DerivedType, FunctionType, Type, content_type};

use super::builtins::{Builtin, builtins};

#[derive(Clone, Debug)]
pub struct Binding {
    pub ty: Type,
    pub display_type: Type,
    pub builtin: Option<String>,
}

impl Binding {
    pub fn new(ty: Type) -> Self {
        Self {
            display_type: ty.clone(),
            ty,
            builtin: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Environment {
    pub bindings: BTreeMap<String, Binding>,
    pub servers: BTreeMap<String, Type>,
    pub builtins: BTreeMap<String, Builtin>,
    pub definitions: BTreeMap<String, Type>,
    pub reserved: BTreeSet<String>,
    pub next_result: u64,
}

impl Default for Environment {
    fn default() -> Self {
        let builtins: BTreeMap<_, _> = builtins()
            .into_iter()
            .map(|builtin| (builtin.name.to_owned(), builtin))
            .collect();
        Self {
            bindings: BTreeMap::new(),
            servers: BTreeMap::new(),
            reserved: builtins.keys().cloned().collect(),
            builtins,
            definitions: BTreeMap::from([("Content".into(), content_type())]),
            next_result: 1,
        }
    }
}

impl Environment {
    pub fn bind(&mut self, name: impl Into<String>, ty: Type) {
        let name = name.into();
        self.reserved.insert(name.clone());
        self.bindings.insert(name, Binding::new(ty));
    }

    pub fn add_server(
        &mut self,
        name: impl Into<String>,
        mut derived: DerivedType,
    ) -> Result<(), String> {
        let name = name.into();
        if !valid_symbol(&name) {
            return Err(format!(
                "invalid server name '{name}'; choose a 1–128 character ASCII symbol starting with a letter or '_', with no empty dotted segments or reserved literal root"
            ));
        }
        self.check_available(&name)?;
        if !matches!(derived.ty, Type::Server(_)) {
            return Err(format!("server '{name}' must have a server type"));
        }
        if let Some(content) = derived.definitions.get("Content")
            && (content != &content_type()
                || self
                    .definitions
                    .get("Content")
                    .is_some_and(|old| old != content))
        {
            return Err(format!(
                "server '{name}' defines a conflicting Content type; Content is a fixed global definition"
            ));
        }

        let mut renames = BTreeMap::new();
        let mut reserved: BTreeSet<_> = self
            .definitions
            .keys()
            .chain(derived.definitions.keys())
            .cloned()
            .collect();
        for definition in derived.definitions.keys() {
            if definition != "Content" && self.definitions.contains_key(definition) {
                let mut suffix = 2;
                let replacement = loop {
                    let candidate = format!("{definition}_{suffix}");
                    if reserved.insert(candidate.clone()) {
                        break candidate;
                    }
                    suffix += 1;
                };
                renames.insert(definition.clone(), replacement);
            }
        }
        rename(&mut derived.ty, &renames);
        for (definition, mut ty) in derived.definitions {
            rename(&mut ty, &renames);
            let definition = renames.get(&definition).cloned().unwrap_or(definition);
            self.definitions.entry(definition).or_insert(ty);
        }
        self.reserved.insert(name.clone());
        self.servers.insert(name, derived.ty);
        Ok(())
    }

    pub fn add_builtin(&mut self, builtin: Builtin) -> Result<(), String> {
        self.check_available(builtin.name)?;
        self.reserved.insert(builtin.name.to_owned());
        self.builtins.insert(builtin.name.to_owned(), builtin);
        Ok(())
    }

    fn check_available(&self, name: &str) -> Result<(), String> {
        if self.bindings.contains_key(name)
            || self.servers.contains_key(name)
            || self.builtins.contains_key(name)
            || self.reserved.contains(name)
        {
            return Err(format!(
                "global name '{name}' is already reserved; choose a different name"
            ));
        }
        Ok(())
    }
}

fn valid_symbol(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    let mut parts = name.split('.');
    let root = parts.next().unwrap_or_default();
    if !root
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || matches!(root, "true" | "false" | "null")
    {
        return false;
    }
    std::iter::once(root).chain(parts).all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn rename(ty: &mut Type, names: &BTreeMap<String, String>) {
    match ty {
        Type::Named(name) => {
            if let Some(replacement) = names.get(name) {
                *name = replacement.clone();
            }
        }
        Type::Array(item) => rename(item, names),
        Type::Tuple(tuple) => {
            for item in &mut tuple.items {
                rename(&mut item.ty, names);
            }
            if let Some(rest) = &mut tuple.rest {
                rename(rest, names);
            }
        }
        Type::Record(record) => {
            for field in &mut record.fields {
                rename(&mut field.ty, names);
            }
            if let Some(additional) = &mut record.additional {
                rename(additional, names);
            }
        }
        Type::Union(types) => {
            for ty in types {
                rename(ty, names);
            }
        }
        Type::Server(tools) => {
            for (_, ty) in tools {
                rename(ty, names);
            }
        }
        Type::Function(function) => rename_function(function, names),
        Type::Tool(tool) => rename_function(&mut tool.signature, names),
        _ => {}
    }
}

fn rename_function(function: &mut FunctionType, names: &BTreeMap<String, String>) {
    for parameter in &mut function.parameters {
        rename(&mut parameter.ty, names);
    }
    rename(&mut function.result, names);
}

#[cfg(test)]
mod tests {
    use crate::types::{Field, Parameter, RecordType, ToolType, TupleItem, TupleType};

    use super::*;

    fn server(ty: Type, definitions: BTreeMap<String, Type>) -> DerivedType {
        DerivedType {
            ty: Type::Server(vec![("tool".into(), ty)]),
            definitions,
            notes: vec![],
        }
    }

    fn empty_server() -> DerivedType {
        server(Type::Any, BTreeMap::new())
    }

    #[test]
    fn default_and_bind_preserve_types_and_reservations() {
        let mut environment = Environment::default();
        assert_eq!(environment.next_result, 1);
        assert_eq!(environment.definitions["Content"], content_type());
        assert_eq!(environment.builtins.len(), builtins().len());
        for name in environment.builtins.keys() {
            assert!(environment.reserved.contains(name));
        }
        environment.bind("value", Type::String);
        assert_eq!(environment.bindings["value"].ty, Type::String);
        assert_eq!(environment.bindings["value"].display_type, Type::String);
        assert_eq!(environment.bindings["value"].builtin, None);
        environment.bindings.remove("value");
        assert!(environment.add_server("value", empty_server()).is_err());
    }

    #[test]
    fn validates_server_symbols_without_restricting_member_names() {
        for name in [
            "x",
            "_tmp",
            "browser-navigate",
            "a.b",
            "a.2b",
            "a.-b",
            "x.true",
        ] {
            let mut environment = Environment::default();
            assert!(
                environment.add_server(name, empty_server()).is_ok(),
                "{name}"
            );
        }
        for name in [
            "",
            "2fa-check",
            "-x",
            ".a",
            "a.",
            "a..b",
            "true",
            "false.x",
            "null.x",
            "é",
            "a b",
            "a/b",
        ] {
            let mut environment = Environment::default();
            assert!(
                environment.add_server(name, empty_server()).is_err(),
                "{name}"
            );
            assert!(environment.servers.is_empty());
        }
        let mut environment = Environment::default();
        assert!(
            environment
                .add_server("a".repeat(128), empty_server())
                .is_ok()
        );
        assert!(
            environment
                .add_server("a".repeat(129), empty_server())
                .is_err()
        );
        assert!(
            environment
                .add_server(
                    "unusual",
                    DerivedType {
                        ty: Type::Server(vec![("tool..name".into(), Type::Any)]),
                        definitions: BTreeMap::new(),
                        notes: vec![],
                    }
                )
                .is_ok()
        );
    }

    #[test]
    fn rejects_all_collisions_even_when_maps_are_mutated_directly() {
        let mut environment = Environment::default();
        environment
            .bindings
            .insert("binding".into(), Binding::new(Type::Null));
        environment
            .servers
            .insert("server".into(), Type::Server(vec![]));
        environment.reserved.insert("evicted".into());
        for name in ["binding", "server", "evicted", "len"] {
            assert!(environment.add_server(name, empty_server()).is_err());
            let mut builtin = builtins().remove(0);
            builtin.name = name;
            assert!(environment.add_builtin(builtin).is_err());
        }
        let mut builtin = builtins().remove(0);
        builtin.name = "custom";
        environment.add_builtin(builtin).unwrap();
        assert!(environment.reserved.contains("custom"));
        assert!(environment.add_server("custom", empty_server()).is_err());
        assert_eq!(environment.bindings["binding"].ty, Type::Null);
    }

    #[test]
    fn failed_imports_do_not_modify_environment() {
        let mut environment = Environment::default();
        let original = environment.definitions.clone();
        let mut derived = empty_server();
        derived.ty = Type::Any;
        assert!(environment.add_server("invalid", derived).is_err());
        assert!(
            environment
                .add_server(
                    "conflict",
                    server(
                        Type::Any,
                        BTreeMap::from([
                            ("Content".into(), Type::String),
                            ("Other".into(), Type::Boolean),
                        ])
                    )
                )
                .is_err()
        );
        assert_eq!(environment.definitions, original);
        assert!(!environment.reserved.contains("invalid"));
        assert!(!environment.reserved.contains("conflict"));
        assert!(environment.servers.is_empty());
        environment
            .add_server("valid", server(Type::Any, original.clone()))
            .unwrap();
        assert_eq!(environment.definitions, original);
    }

    #[test]
    fn recursive_definitions_are_isolated_and_suffixes_skip_imported_names() {
        let node = |leaf| {
            Type::Record(RecordType {
                fields: vec![
                    Field {
                        name: "value".into(),
                        ty: leaf,
                        optional: false,
                    },
                    Field {
                        name: "next".into(),
                        ty: Type::Named("Node".into()),
                        optional: true,
                    },
                ],
                additional: None,
            })
        };
        let mut environment = Environment::default();
        environment
            .add_server(
                "server_one",
                server(
                    Type::Named("Node".into()),
                    BTreeMap::from([("Node".into(), node(Type::String))]),
                ),
            )
            .unwrap();
        let first = environment.servers["server_one"].clone();
        environment
            .add_server(
                "second",
                server(
                    Type::Named("Node".into()),
                    BTreeMap::from([
                        ("Node".into(), node(Type::Boolean)),
                        ("Node_2".into(), Type::Named("Node".into())),
                    ]),
                ),
            )
            .unwrap();
        assert_eq!(environment.servers["server_one"], first);
        assert_eq!(
            environment.servers["second"],
            Type::Server(vec![("tool".into(), Type::Named("Node_3".into()))])
        );
        assert_eq!(environment.definitions["Node"], node(Type::String));
        assert_eq!(
            environment.definitions["Node_2"],
            Type::Named("Node_3".into())
        );
        let Type::Record(second) = &environment.definitions["Node_3"] else {
            panic!("record")
        };
        assert_eq!(second.fields[0].ty, Type::Boolean);
        assert_eq!(second.fields[1].ty, Type::Named("Node_3".into()));
    }

    #[test]
    fn renames_all_nested_type_positions_simultaneously() {
        let named = Type::Named("Node".into());
        let signature = FunctionType {
            parameters: vec![Parameter {
                name: "input".into(),
                ty: named.clone(),
            }],
            result: Box::new(named.clone()),
        };
        let nested = Type::Union(vec![
            Type::Array(Box::new(named.clone())),
            Type::Tuple(TupleType {
                items: vec![TupleItem {
                    ty: named.clone(),
                    optional: false,
                }],
                rest: Some(Box::new(named.clone())),
            }),
            Type::Record(RecordType {
                fields: vec![Field {
                    name: "item".into(),
                    ty: named.clone(),
                    optional: false,
                }],
                additional: Some(Box::new(named.clone())),
            }),
            Type::Function(signature.clone()),
            Type::Tool(ToolType {
                server: "second".into(),
                name: "tool".into(),
                signature,
            }),
            Type::Named("Other".into()),
        ]);
        let mut environment = Environment::default();
        environment.definitions.insert("Node".into(), Type::String);
        environment
            .definitions
            .insert("Other".into(), Type::Boolean);
        environment
            .add_server(
                "second",
                server(
                    nested.clone(),
                    BTreeMap::from([("Node".into(), nested.clone()), ("Other".into(), named)]),
                ),
            )
            .unwrap();
        let mut expected = nested;
        rename(
            &mut expected,
            &BTreeMap::from([
                ("Node".into(), "Node_2".into()),
                ("Other".into(), "Other_2".into()),
            ]),
        );
        assert_eq!(
            environment.servers["second"],
            Type::Server(vec![("tool".into(), expected.clone())])
        );
        assert_eq!(environment.definitions["Node_2"], expected);
        assert_eq!(
            environment.definitions["Other_2"],
            Type::Named("Node_2".into())
        );
        assert_eq!(environment.definitions["Node"], Type::String);
        assert_eq!(environment.definitions["Other"], Type::Boolean);
        let printed = environment.definitions["Node_2"].to_string();
        assert!(!printed.contains("Node[]"));
        assert!(printed.contains("Node_2[]"));
        assert!(printed.contains("[Node_2, ...Node_2[]]"));
        assert!(printed.contains("{item: Node_2, [key: string]: Node_2}"));
        assert!(printed.contains("(input: Node_2) => Node_2"));
        assert!(printed.contains("Other_2"));
    }
}
