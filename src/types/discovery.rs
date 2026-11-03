use std::collections::BTreeMap;

use crate::registry::ToolDefinition;

use super::{
    DerivedType, FunctionType, Parameter, ToolType, Type, content_result_type, content_type,
    derive_schema,
};

/// Input and output definitions share a collision-free naming scope.
pub fn derive_tool(server: &str, tool: &ToolDefinition) -> DerivedType {
    let mut result = DerivedType {
        ty: Type::Any,
        definitions: BTreeMap::new(),
        notes: Vec::new(),
    };
    let input = import(
        &mut result,
        derive_schema(&tool.input_schema),
        "/inputSchema",
    );
    let output = if let Some(schema) = &tool.output_schema {
        import(&mut result, derive_schema(schema), "/outputSchema")
    } else {
        result.definitions.insert("Content".into(), content_type());
        content_result_type()
    };
    result.ty = Type::Tool(ToolType {
        server: server.into(),
        name: tool.name.clone(),
        signature: FunctionType {
            parameters: vec![Parameter {
                name: "args".into(),
                ty: input,
            }],
            result: Box::new(output),
        },
    });
    result
}

/// Tool ordering follows the supplied discovery order, names are never rewritten.
pub fn derive_server<'a>(
    server: &str,
    tools: impl IntoIterator<Item = &'a ToolDefinition>,
) -> DerivedType {
    let mut result = DerivedType {
        ty: Type::Any,
        definitions: BTreeMap::new(),
        notes: Vec::new(),
    };
    let mut fields = Vec::new();
    for tool in tools {
        let path = format!("/tools/{}", tool.name.replace('~', "~0").replace('/', "~1"));
        let ty = import(&mut result, derive_tool(server, tool), &path);
        fields.push((tool.name.clone(), ty));
    }
    result.ty = Type::Server(fields);
    result
}

pub fn render_tool_signature(tool: &ToolType) -> String {
    format!("{}.{}: {}", tool.server, tool.name, tool.signature)
}

fn import(target: &mut DerivedType, mut source: DerivedType, path: &str) -> Type {
    let mut renames = BTreeMap::new();
    let mut reserved: Vec<String> = target
        .definitions
        .keys()
        .chain(source.definitions.keys())
        .cloned()
        .collect();
    for name in source.definitions.keys() {
        // Content is the one globally shared, fixed definition.
        if target.definitions.contains_key(name) && name != "Content" {
            let mut n = 2;
            let mut replacement = format!("{name}_{n}");
            while reserved.contains(&replacement) {
                n += 1;
                replacement = format!("{name}_{n}");
            }
            reserved.push(replacement.clone());
            renames.insert(name.clone(), replacement);
        }
    }
    rename(&mut source.ty, &renames);
    for (name, mut ty) in source.definitions {
        rename(&mut ty, &renames);
        target
            .definitions
            .insert(renames.get(&name).cloned().unwrap_or(name), ty);
    }
    target
        .notes
        .extend(source.notes.into_iter().map(|mut note| {
            note.path = format!("{path}{}", note.path);
            note
        }));
    source.ty
}

fn rename(ty: &mut Type, names: &BTreeMap<String, String>) {
    match ty {
        Type::Named(name) => {
            if let Some(new) = names.get(name) {
                *name = new.clone();
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
