use std::collections::BTreeMap;

use crate::{
    frontend::{
        Environment, Span,
        checker::CheckedStatement,
        diagnostic::{Category, Diagnostic},
    },
    registry::Registry,
    runtime::{GlobalStore, Value},
    types::{Type, derive_server},
};

pub const SESSION_MEMORY: usize = 256 * 1024 * 1024;

struct Stored {
    value: Value,
    used: u64,
}

pub struct SessionState {
    pub environment: Environment,
    values: BTreeMap<String, Stored>,
    clock: u64,
    metadata: usize,
    memory_limit: usize,
    pub initialization_errors: Vec<Diagnostic>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new(&Registry::default())
    }
}

impl SessionState {
    pub fn new(registry: &Registry) -> Self {
        let mut environment = Environment::default();
        let mut initialization_errors = Vec::new();
        for (name, server) in registry.snapshot() {
            let tools: Vec<_> = server.tools.into_values().collect();
            if let Err(error) = environment.add_server(&name, derive_server(&name, &tools)) {
                initialization_errors.push(
                    Diagnostic::new(Category::Type, error, Span::default())
                        .with_hint("rename the server in the MCPL configuration"),
                );
            }
        }
        let metadata = environment
            .servers
            .iter()
            .chain(&environment.definitions)
            .map(|(name, ty)| name.len() + type_bytes(ty) + 128)
            .sum();
        Self {
            environment,
            values: BTreeMap::new(),
            clock: 0,
            metadata,
            memory_limit: SESSION_MEMORY,
            initialization_errors,
        }
    }

    /// Embedders may lower the compiled-in ceiling; the MCP surface cannot raise it.
    pub fn with_memory_limit(mut self, bytes: usize) -> Self {
        self.memory_limit = bytes.min(SESSION_MEMORY);
        self
    }

    pub fn commit(&mut self, statement: &CheckedStatement, value: Value) -> Result<(), Diagnostic> {
        let extra = 256
            + statement.name.len() * 3
            + type_bytes(&statement.binding.ty)
            + type_bytes(&statement.binding.display_type)
            + statement.binding.builtin.as_ref().map_or(0, String::len);
        self.make_room(
            std::slice::from_ref(&value),
            extra,
            statement.statement.span,
        )?;
        self.metadata += extra;
        self.clock += 1;
        self.values.insert(
            statement.name.clone(),
            Stored {
                value,
                used: self.clock,
            },
        );
        self.environment
            .bindings
            .insert(statement.name.clone(), statement.binding.clone());
        self.environment.reserved.insert(statement.name.clone());
        self.environment.next_result = statement.next_result;
        Ok(())
    }

    fn make_room(&mut self, active: &[Value], extra: usize, span: Span) -> Result<(), Diagnostic> {
        loop {
            let roots: Vec<_> = self
                .values
                .values()
                .map(|stored| stored.value.clone())
                .chain(active.iter().cloned())
                .collect();
            let upper = roots
                .iter()
                .fold(self.metadata.saturating_add(extra), |total, value| {
                    total.saturating_add(value.logical_bytes())
                });
            if upper <= self.memory_limit {
                return Ok(());
            }
            let used = Value::retained_bytes(&roots)
                .saturating_add(self.metadata)
                .saturating_add(extra);
            if used <= self.memory_limit {
                return Ok(());
            }
            let oldest = self
                .values
                .iter()
                .min_by_key(|(_, stored)| stored.used)
                .map(|(name, _)| name.clone());
            if let Some(name) = oldest {
                self.values.remove(&name);
            } else {
                return Err(Diagnostic::new(
                    Category::Limit,
                    format!(
                        "session memory limit exceeded ({} bytes)",
                        self.memory_limit
                    ),
                    span,
                )
                .with_hint("reduce retained data or start a new session"));
            }
        }
    }
}

impl GlobalStore for SessionState {
    fn lookup(&mut self, name: &str, span: Span) -> Result<Value, Diagnostic> {
        if let Some(ty) = self.environment.servers.get(name) {
            return Ok(Value::server(name.to_owned(), ty.clone()));
        }
        if let Some(builtin) = self.environment.builtins.get(name) {
            return Ok(Value::builtin(
                name.to_owned(),
                Type::Builtin(crate::types::BuiltinType {
                    name: name.to_owned(),
                    signature: builtin.signature.clone(),
                }),
            ));
        }
        match self.values.get_mut(name) {
            Some(stored) => {
                self.clock += 1;
                stored.used = self.clock;
                Ok(stored.value.clone())
            }
            None => Err(Diagnostic::new(
                Category::Runtime,
                format!("`{name}` was evicted (session memory limit); recompute it"),
                span,
            )
            .with_hint("rerun the computation under a new name")),
        }
    }

    fn reserve(
        &mut self,
        active: &[Value],
        temporary_bytes: usize,
        span: Span,
    ) -> Result<(), Diagnostic> {
        self.make_room(active, temporary_bytes, span)
    }
}

fn type_bytes(ty: &Type) -> usize {
    let mut pending = vec![ty];
    let mut total = 0;
    while let Some(ty) = pending.pop() {
        total += std::mem::size_of::<Type>();
        match ty {
            Type::Array(item) => pending.push(item),
            Type::Tuple(tuple) => {
                pending.extend(tuple.items.iter().map(|item| &item.ty));
                pending.extend(tuple.rest.as_deref());
            }
            Type::Record(record) => {
                for field in &record.fields {
                    total += field.name.len() + 32;
                    pending.push(&field.ty);
                }
                pending.extend(record.additional.as_deref());
            }
            Type::Union(items) => pending.extend(items),
            Type::Server(tools) => {
                for (name, ty) in tools {
                    total += name.len() + 32;
                    pending.push(ty);
                }
            }
            Type::Function(function) => {
                pending.push(&function.result);
                for p in &function.parameters {
                    total += p.name.len() + 32;
                    pending.push(&p.ty);
                }
            }
            Type::Tool(tool) => {
                total += tool.name.len() + tool.server.len();
                pending.push(&tool.signature.result);
                for p in &tool.signature.parameters {
                    total += p.name.len() + 32;
                    pending.push(&p.ty);
                }
            }
            Type::Builtin(builtin) => {
                total += builtin.name.len();
                pending.push(&builtin.signature.result);
                for p in &builtin.signature.parameters {
                    total += p.name.len() + 32;
                    pending.push(&p.ty);
                }
            }
            Type::Named(name) => total += name.len(),
            Type::Literal(value) => total += value.to_string().len(),
            _ => {}
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{frontend::check_program, runtime::Budget};

    fn string(value: &str) -> Value {
        Value::string(value.into(), &mut Budget::default(), Span::default()).unwrap()
    }
    fn commit(state: &mut SessionState, name: &str, value: Value) {
        let checked = check_program(&format!("{name} = \"\";"), &state.environment).unwrap();
        state.commit(&checked.statements[0], value).unwrap();
    }
    #[test]
    fn lru_uses_global_lookup_and_tombstones_are_budgeted() {
        let mut state = SessionState::default();
        commit(&mut state, "a", string(&"a".repeat(1000)));
        commit(&mut state, "b", string(&"b".repeat(1000)));
        state.lookup("a", Span::default()).unwrap();
        let value = string(&"c".repeat(1000));
        let extra =
            256 + 3 + type_bytes(&Type::Literal(json_string(""))) + type_bytes(&Type::String);
        let existing: Vec<_> = state.values.values().map(|s| s.value.clone()).collect();
        state.memory_limit = state.metadata + extra + Value::retained_bytes(&existing);
        commit(&mut state, "c", value);
        assert!(state.lookup("a", Span::default()).is_ok());
        assert!(
            state
                .lookup("b", Span::default())
                .unwrap_err()
                .message
                .contains("was evicted")
        );
        assert!(state.environment.bindings.contains_key("b"));
        assert!(check_program("b = 0;", &state.environment).is_err());
        state.memory_limit = state.metadata;
        let checked = check_program("d = 1;", &state.environment).unwrap();
        assert!(
            state
                .commit(
                    &checked.statements[0],
                    Value::number(1.0, Span::default()).unwrap()
                )
                .is_err()
        );
        assert!(!state.environment.bindings.contains_key("d"));
    }
    fn json_string(value: &str) -> serde_json::Value {
        serde_json::Value::String(value.into())
    }

    #[test]
    fn aliases_count_shared_storage_once_and_eviction_keeps_active_values() {
        let mut state = SessionState::default();
        let value = string(&"x".repeat(10000));
        commit(&mut state, "a", value.clone());
        commit(&mut state, "b", value.clone());
        let metadata = state.metadata;
        let one = Value::retained_bytes(std::slice::from_ref(&value));
        state.memory_limit = metadata + one;
        assert!(
            state
                .reserve(std::slice::from_ref(&value), 0, Span::default())
                .is_ok()
        );
        assert_eq!(state.values.len(), 2);
        state.memory_limit = metadata;
        assert!(
            state
                .reserve(std::slice::from_ref(&value), 0, Span::default())
                .is_err()
        );
        assert!(state.values.is_empty());
        assert_eq!(value.as_str(Span::default()).unwrap().len(), 10000);
    }
}
