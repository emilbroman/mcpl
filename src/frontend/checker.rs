use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::types::{
    BuiltinType, Field, FunctionType, Parameter, RecordType, TupleItem, TupleType, Type, union,
};

use super::{
    ast::*,
    builtins::Builtin,
    diagnostic::{Category, Diagnostic},
    environment::{Binding, Environment},
    parser::{parse_expression, parse_program},
    relations::{assignable, disjoint, element_type, equatable, property_type, resolve, widen},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Root {
    Session(String),
    Local { scope: usize, name: String },
    Server(String),
    Builtin(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValuePath {
    pub root: Root,
    pub keys: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MemberAccess {
    pub key: String,
    pub span: Span,
    pub ty: Type,
    pub optional: bool,
}

#[derive(Clone, Debug)]
pub struct Resolution {
    pub root: Option<Root>,
    pub members: Vec<MemberAccess>,
}

#[derive(Clone, Debug)]
pub struct ExpressionInfo {
    /// Precise literals are retained for assignability; use display_type for listings.
    pub ty: Type,
    pub display_type: Type,
    pub builtin: Option<String>,
    pub path: Option<ValuePath>,
    pub resolution: Option<Resolution>,
}

impl ExpressionInfo {
    fn new(ty: Type) -> Self {
        Self {
            display_type: ty.clone(),
            ty,
            builtin: None,
            path: None,
            resolution: None,
        }
    }
    fn binding(&self) -> Binding {
        Binding {
            ty: self.ty.clone(),
            display_type: self.display_type.clone(),
            builtin: self.builtin.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CheckedStatement {
    pub statement: Statement,
    pub name: String,
    pub binding: Binding,
    /// Commit this counter only after this statement succeeds at runtime.
    pub next_result: u64,
}

#[derive(Clone, Debug)]
pub struct CheckedProgram {
    pub program: Program,
    pub statements: Vec<CheckedStatement>,
    pub expressions: BTreeMap<Span, ExpressionInfo>,
}

#[derive(Clone, Debug)]
pub struct CheckedExpression {
    pub expression: Expr,
    pub info: ExpressionInfo,
    pub expressions: BTreeMap<Span, ExpressionInfo>,
}

pub fn check_program(
    source: &str,
    environment: &Environment,
) -> Result<CheckedProgram, Vec<Diagnostic>> {
    let program = parse_program(source).map_err(|e| vec![e])?;
    on_stack(|| Checker::new(environment).program(program))
}

pub fn check_expression(
    source: &str,
    environment: &Environment,
) -> Result<CheckedExpression, Vec<Diagnostic>> {
    let expression = parse_expression(source).map_err(|e| vec![e])?;
    on_stack(|| {
        let mut checker = Checker::new(environment);
        let info = checker.infer(&expression, None);
        checker.finish()?;
        Ok(CheckedExpression {
            expression,
            info,
            expressions: checker.expressions,
        })
    })
}

fn on_stack<T: Send>(
    work: impl FnOnce() -> Result<T, Vec<Diagnostic>> + Send,
) -> Result<T, Vec<Diagnostic>> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("mcpl-check".into())
            .stack_size(16 * 1024 * 1024)
            .spawn_scoped(scope, work)
            .map_err(|_| {
                vec![Diagnostic::new(
                    Category::Limit,
                    "unable to allocate type-checker stack",
                    Span::default(),
                )]
            })?
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

struct Scope {
    id: usize,
    bindings: BTreeMap<String, Binding>,
}

struct Checker<'a> {
    env: &'a Environment,
    globals: BTreeMap<String, Binding>,
    reserved: BTreeSet<String>,
    next_result: u64,
    scopes: Vec<Scope>,
    next_scope: usize,
    facts: BTreeMap<ValuePath, Type>,
    expressions: BTreeMap<Span, ExpressionInfo>,
    errors: Vec<Diagnostic>,
    type_nodes: usize,
}

impl<'a> Checker<'a> {
    fn new(env: &'a Environment) -> Self {
        let mut errors = Vec::new();
        for name in env.servers.keys() {
            if env.builtins.contains_key(name) || env.bindings.contains_key(name) {
                errors.push(
                    Diagnostic::new(
                        Category::Type,
                        format!("ambiguous name '{name}' (server and builtin or binding)"),
                        Span::default(),
                    )
                    .with_hint("rename the server in configuration"),
                );
            }
        }
        for name in env.bindings.keys() {
            if env.builtins.contains_key(name) {
                errors.push(
                    Diagnostic::new(
                        Category::Type,
                        format!("{name} is already defined as a builtin"),
                        Span::default(),
                    )
                    .with_hint("choose a different session binding name"),
                );
            }
        }
        errors.truncate(10);
        Self {
            env,
            globals: env.bindings.clone(),
            reserved: env.reserved.clone(),
            next_result: env.next_result.max(1),
            scopes: vec![],
            next_scope: 0,
            facts: BTreeMap::new(),
            expressions: BTreeMap::new(),
            errors,
            type_nodes: 0,
        }
    }

    fn program(mut self, program: Program) -> Result<CheckedProgram, Vec<Diagnostic>> {
        let explicit: BTreeSet<_> = program
            .statements
            .iter()
            .filter_map(|s| s.name.as_ref().map(|n| n.text.clone()))
            .collect();
        let mut statements = vec![];
        for statement in &program.statements {
            let name = if let Some(name) = &statement.name {
                if self.is_reserved(&name.text) {
                    self.error(
                        format!("{} is already defined", name.text),
                        name.span,
                        "choose a new name; session bindings cannot be reassigned",
                    );
                }
                name.text.clone()
            } else {
                while self.is_reserved(&format!("res{}", self.next_result)) {
                    if !self.advance_counter(statement.span) {
                        break;
                    }
                }
                let name = format!("res{}", self.next_result);
                if explicit.contains(&name) {
                    self.error(
                        format!("{name} conflicts with an explicit declaration in this program"),
                        statement.span,
                        "choose a different explicit name",
                    );
                }
                self.advance_counter(statement.span);
                name
            };
            let value = self.infer(&statement.expression, None);
            let binding = value.binding();
            self.reserved.insert(name.clone());
            self.globals.insert(name.clone(), binding.clone());
            statements.push(CheckedStatement {
                statement: statement.clone(),
                name,
                binding,
                next_result: self.next_result,
            });
        }
        self.finish()?;
        Ok(CheckedProgram {
            program,
            statements,
            expressions: self.expressions,
        })
    }

    fn advance_counter(&mut self, span: Span) -> bool {
        if let Some(next) = self.next_result.checked_add(1) {
            self.next_result = next;
            true
        } else {
            self.report(Diagnostic::new(
                Category::Limit,
                "automatic binding names exhausted",
                span,
            ));
            false
        }
    }

    fn finish(&mut self) -> Result<(), Vec<Diagnostic>> {
        self.errors.sort_by_key(|e| e.span.start);
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(std::mem::take(&mut self.errors))
        }
    }

    fn is_reserved(&self, name: &str) -> bool {
        self.reserved.contains(name)
            || self.globals.contains_key(name)
            || self.env.servers.contains_key(name)
            || self.env.builtins.contains_key(name)
    }

    fn report(&mut self, error: Diagnostic) {
        if !self.errors.contains(&error) {
            self.errors.push(error);
            self.errors.sort_by_key(|error| error.span.start);
            self.errors.truncate(10);
        }
    }

    fn error(&mut self, message: impl Into<String>, span: Span, hint: impl Into<String>) {
        self.report(Diagnostic::new(Category::Type, message, span).with_hint(hint));
    }

    fn scope(&mut self) {
        let id = self.next_scope;
        self.next_scope += 1;
        self.scopes.push(Scope {
            id,
            bindings: BTreeMap::new(),
        });
    }

    fn local(&mut self, name: &Name, binding: Binding) {
        if self
            .scopes
            .last()
            .unwrap()
            .bindings
            .contains_key(&name.text)
        {
            self.error(
                format!("{} is already defined in this scope", name.text),
                name.span,
                "choose a new local name",
            );
        } else {
            self.scopes
                .last_mut()
                .unwrap()
                .bindings
                .insert(name.text.clone(), binding);
        }
    }

    fn infer(&mut self, expr: &Expr, expected: Option<&Type>) -> ExpressionInfo {
        if self.type_nodes > 100_000 {
            self.report(
                Diagnostic::new(
                    Category::Limit,
                    "static type expansion limit exceeded (100000 nodes)",
                    expr.span,
                )
                .with_hint("split the program or reduce nested type duplication"),
            );
            return ExpressionInfo::new(Type::Any);
        }
        let mut info = self.infer_inner(expr, expected);
        if let Type::Builtin(capability) = &info.ty
            && let Some(context) = expected.and_then(function)
            && let Some(builtin) = self.env.builtins.get(&capability.name)
        {
            let signature = self.contextual_builtin(builtin.clone(), &context, expr.span);
            info.ty = Type::Function(signature.clone());
            info.display_type = Type::Function(signature);
        }
        if let Some(path) = &info.path
            && let Some(ty) = self.facts.get(path)
        {
            info.ty = ty.clone();
            info.display_type = ty.clone();
        }
        self.type_nodes += type_size(&info.ty, 100_001);
        self.expressions.insert(expr.span, info.clone());
        info
    }

    fn infer_inner(&mut self, expr: &Expr, expected: Option<&Type>) -> ExpressionInfo {
        match &expr.kind {
            ExprKind::Literal(value) => {
                let ty = if value.is_null() {
                    Type::Null
                } else {
                    Type::Literal(value.clone())
                };
                let mut info = ExpressionInfo::new(ty.clone());
                info.display_type = widen(&ty);
                info
            }
            ExprKind::OverflowNumber(_) => {
                self.error(
                    "numeric literal is not finite",
                    expr.span,
                    "use a finite IEEE 754 number",
                );
                ExpressionInfo::new(Type::number())
            }
            ExprKind::Symbol(parts) => self.symbol(parts),
            ExprKind::Group(inner) => self.infer(inner, expected),
            ExprKind::Array(items) => {
                let context = expected.map(|ty| resolve(ty, &self.env.definitions));
                let tuple = match &context {
                    Some(Type::Tuple(t)) => Some(t),
                    _ => None,
                };
                let mut inferred = vec![];
                for (i, item) in items.iter().enumerate() {
                    let expected = match &context {
                        Some(Type::Array(ty)) => Some(ty.as_ref()),
                        Some(Type::Tuple(t)) => t.items.get(i).map(|s| &s.ty).or(t.rest.as_deref()),
                        _ => None,
                    };
                    inferred.push(self.infer(item, expected));
                }
                let shape = |display: bool| {
                    let types = inferred
                        .iter()
                        .map(|i| {
                            if display {
                                i.display_type.clone()
                            } else {
                                i.ty.clone()
                            }
                        })
                        .collect::<Vec<_>>();
                    if tuple.is_some() {
                        tuple_type(types)
                    } else {
                        Type::Array(Box::new(union(types)))
                    }
                };
                let mut info = ExpressionInfo::new(shape(false));
                info.display_type = shape(true);
                info
            }
            ExprKind::Object(fields) => {
                let context = expected.map(|ty| self.object_context(ty, fields));
                let mut actual = vec![];
                let mut display = vec![];
                for field in fields {
                    let expected = context
                        .as_ref()
                        .and_then(|ty| property_type(ty, &field.name.text, &self.env.definitions));
                    let value = self.infer(&field.value, expected.as_ref());
                    put_field(&mut actual, &field.name.text, value.ty);
                    put_field(&mut display, &field.name.text, value.display_type);
                }
                let mut info = ExpressionInfo::new(record(actual));
                info.display_type = record(display);
                info
            }
            ExprKind::Lambda { parameters, body } => {
                let signature =
                    expected.and_then(|ty| function(&resolve(ty, &self.env.definitions)));
                self.scope();
                let mut typed = vec![];
                for (i, name) in parameters.iter().enumerate() {
                    let ty = signature
                        .as_ref()
                        .and_then(|s| s.parameters.get(i))
                        .map_or(Type::Any, |p| p.ty.clone());
                    self.local(name, Binding::new(ty.clone()));
                    typed.push(Parameter {
                        name: name.text.clone(),
                        ty,
                    });
                }
                let result = self.infer(body, signature.as_ref().map(|s| s.result.as_ref()));
                self.scopes.pop();
                let mut info = ExpressionInfo::new(Type::Function(FunctionType {
                    parameters: typed.clone(),
                    result: Box::new(result.ty),
                }));
                info.display_type = Type::Function(FunctionType {
                    parameters: typed,
                    result: Box::new(result.display_type),
                });
                info
            }
            ExprKind::Sequence { items, value } => {
                self.scope();
                for item in items {
                    let info = self.infer(&item.expression, None);
                    if let Some(name) = &item.name {
                        self.local(name, info.binding());
                    }
                }
                let info = self.infer(value, expected);
                self.scopes.pop();
                info
            }
            ExprKind::Unary { op, operand } => {
                let operand_info = self.infer(operand, None);
                let target = match op {
                    UnaryOp::Not => Type::Boolean,
                    UnaryOp::Negate => Type::number(),
                };
                self.require(&operand_info.ty, &target, operand.span, "operand");
                let mut info = ExpressionInfo::new(target);
                if *op == UnaryOp::Negate
                    && let Some(number) = literal_number(operand)
                    && let Some(value) = serde_json::Number::from_f64(-number)
                {
                    info.ty = Type::Literal(Value::Number(value));
                }
                info
            }
            ExprKind::Binary { op, left, right } => self.binary(*op, left, right),
            ExprKind::Conditional { condition, yes, no } => {
                let condition_info = self.infer(condition, None);
                if !assignable(&condition_info.ty, &Type::Boolean, &self.env.definitions) {
                    self.error(
                        format!(
                            "conditional requires boolean, found {}",
                            condition_info.display_type
                        ),
                        condition.span,
                        "compare explicitly; MCPL has no truthiness",
                    );
                }
                let facts = self.facts.clone();
                self.narrow(condition, true);
                let a = self.infer(yes, expected);
                self.facts = facts.clone();
                self.narrow(condition, false);
                let b = self.infer(no, expected);
                self.facts = facts;
                let mut info = ExpressionInfo::new(union([a.ty, b.ty]));
                info.display_type = union([a.display_type, b.display_type]);
                if a.builtin == b.builtin {
                    info.builtin = a.builtin;
                }
                info
            }
            ExprKind::Call { callee, arguments } => {
                let function = self.infer(callee, None);
                if let Some(name) = &function.builtin
                    && let Some(builtin) = self.env.builtins.get(name)
                {
                    self.builtin_call(builtin.clone(), arguments, expr.span)
                } else {
                    let mut info = self.call(&function.ty, arguments, expr.span);
                    if let Some(display) =
                        callable_result(&function.display_type, &self.env.definitions)
                    {
                        info.display_type = display;
                    }
                    info
                }
            }
            ExprKind::Member { receiver, members } => {
                let info = self.infer(receiver, None);
                self.members(info, members, None)
            }
            ExprKind::Index { receiver, index } => self.index(receiver, index),
        }
    }

    fn object_context(&self, ty: &Type, fields: &[ObjectField]) -> Type {
        let ty = resolve(ty, &self.env.definitions);
        if let Type::Union(variants) = &ty {
            let candidates: Vec<_> = variants
                .iter()
                .filter(|variant| {
                    fields.iter().all(|field| {
                        let ExprKind::Literal(value) = &field.value.kind else {
                            return true;
                        };
                        property_type(variant, &field.name.text, &self.env.definitions).is_none_or(
                            |expected| {
                                assignable(
                                    &Type::Literal(value.clone()),
                                    &expected,
                                    &self.env.definitions,
                                )
                            },
                        )
                    })
                })
                .collect();
            if candidates.len() == 1 {
                return candidates[0].clone();
            }
        }
        ty
    }

    fn symbol(&mut self, parts: &[Name]) -> ExpressionInfo {
        for scope in self.scopes.iter().rev() {
            for end in (1..=parts.len()).rev() {
                let name = joined(&parts[..end]);
                if let Some(binding) = scope.bindings.get(&name) {
                    let root = Root::Local {
                        scope: scope.id,
                        name,
                    };
                    let mut info = binding_info(binding);
                    info.path = Some(ValuePath {
                        root: root.clone(),
                        keys: vec![],
                    });
                    return self.members(info, &parts[end..], Some(root));
                }
            }
        }
        for end in (1..=parts.len()).rev() {
            let name = joined(&parts[..end]);
            let found = if let Some(binding) = self.globals.get(&name) {
                Some((binding_info(binding), Root::Session(name.clone())))
            } else if let Some(ty) = self.env.servers.get(&name) {
                Some((ExpressionInfo::new(ty.clone()), Root::Server(name.clone())))
            } else if let Some(builtin) = self.env.builtins.get(&name) {
                let mut info = ExpressionInfo::new(Type::Builtin(BuiltinType {
                    name: name.clone(),
                    signature: builtin.signature.clone(),
                }));
                info.builtin = Some(name.clone());
                Some((info, Root::Builtin(name.clone())))
            } else {
                None
            };
            if let Some((mut info, root)) = found {
                info.path = Some(ValuePath {
                    root: root.clone(),
                    keys: vec![],
                });
                return self.members(info, &parts[end..], Some(root));
            }
        }
        let name = &parts[0].text;
        let mut names: Vec<_> = self
            .globals
            .keys()
            .chain(self.env.servers.keys())
            .chain(self.env.builtins.keys())
            .cloned()
            .collect();
        for scope in &self.scopes {
            names.extend(scope.bindings.keys().cloned());
        }
        let hint = suggestion(name, names)
            .map_or("use help to discover servers and built-ins".into(), |s| {
                format!("did you mean '{s}'?")
            });
        self.error(format!("unknown name '{name}'"), parts[0].span, hint);
        ExpressionInfo::new(Type::Any)
    }

    fn members(
        &mut self,
        mut info: ExpressionInfo,
        parts: &[Name],
        root: Option<Root>,
    ) -> ExpressionInfo {
        let mut accesses = vec![];
        let mut offset = 0;
        while offset < parts.len() {
            self.apply_fact(&mut info);
            let resolved = resolve(&info.ty, &self.env.definitions);
            let mut end = offset + 1;
            if !matches!(resolved, Type::Any) {
                for candidate in ((offset + 1)..=parts.len()).rev() {
                    if known_property(
                        &resolved,
                        &joined(&parts[offset..candidate]),
                        &self.env.definitions,
                    ) {
                        end = candidate;
                        break;
                    }
                }
            }
            let key = joined(&parts[offset..end]);
            let span = parts[offset].span.join(parts[end - 1].span);
            let optional = optional_property(&resolved, &key, &self.env.definitions);
            let ty = self.property(&info.ty, &key, span, info.path.as_ref());
            let display = property_type(&info.display_type, &key, &self.env.definitions)
                .unwrap_or_else(|| ty.clone());
            accesses.push(MemberAccess {
                key: key.clone(),
                span,
                ty: ty.clone(),
                optional,
            });
            if let Some(path) = &mut info.path {
                path.keys.push(key);
            }
            info.ty = ty;
            info.display_type = display;
            info.builtin = None;
            offset = end;
        }
        self.apply_fact(&mut info);
        info.resolution = Some(Resolution {
            root,
            members: accesses,
        });
        info
    }

    fn apply_fact(&self, info: &mut ExpressionInfo) {
        if let Some(path) = &info.path
            && let Some(ty) = self.facts.get(path)
        {
            info.ty = ty.clone();
            info.display_type = ty.clone();
        }
    }

    fn property(&mut self, ty: &Type, key: &str, span: Span, path: Option<&ValuePath>) -> Type {
        if let Some(ty) = property_type(ty, key, &self.env.definitions) {
            return ty;
        }
        let resolved = resolve(ty, &self.env.definitions);
        if matches!(resolved, Type::Array(_) | Type::Tuple(_))
            && self.env.builtins.contains_key(key)
        {
            let receiver = path.map(path_label).unwrap_or("xs".into());
            self.error(
                format!("{key} is not a property of {}", widen(ty)),
                span,
                format!("call {key}({receiver}, f)"),
            );
        } else {
            let fields = known_fields(&resolved);
            let hint = if fields.is_empty() {
                "check the value's type with help or show".into()
            } else {
                format!(
                    "available field{}: {}",
                    if fields.len() == 1 { "" } else { "s" },
                    fields.join(", ")
                )
            };
            self.error(
                format!("property '{key}' does not exist on {}", widen(ty)),
                span,
                hint,
            );
        }
        Type::Any
    }

    fn index(&mut self, receiver: &Expr, index: &Expr) -> ExpressionInfo {
        let base = self.infer(receiver, None);
        let key = self.infer(index, None);
        if let Some(text) = literal_string(index) {
            let ty = self.property(&base.ty, text, index.span, base.path.as_ref());
            let mut info = ExpressionInfo::new(ty.clone());
            info.display_type =
                property_type(&base.display_type, text, &self.env.definitions).unwrap_or(ty);
            info.path = base.path.map(|mut path| {
                path.keys.push(text.into());
                path
            });
            return info;
        }
        let mut info = ExpressionInfo::new(self.index_type(
            &base.ty,
            &key.ty,
            literal_number(index),
            index.span,
        ));
        info.display_type = self
            .index_display(&base.display_type, &key.ty, literal_number(index))
            .unwrap_or_else(|| info.ty.clone());
        if let Some(number) = literal_number(index) {
            info.path = base.path.map(|mut path| {
                path.keys.push(number.to_string());
                path
            });
        }
        info
    }

    fn index_display(&self, ty: &Type, key: &Type, number: Option<f64>) -> Option<Type> {
        self.index_display_at(ty, key, number, 0, &mut 16_384)
    }
    fn index_display_at(
        &self,
        ty: &Type,
        key: &Type,
        number: Option<f64>,
        depth: usize,
        work: &mut usize,
    ) -> Option<Type> {
        if depth >= 64 || *work == 0 {
            return None;
        }
        *work -= 1;
        match resolve(ty, &self.env.definitions) {
            Type::Array(item) => Some(*item),
            Type::Tuple(t) => number
                .and_then(|n| {
                    t.items
                        .get(n as usize)
                        .map(|i| i.ty.clone())
                        .or_else(|| t.rest.as_deref().cloned())
                })
                .or_else(|| element_type(ty, &self.env.definitions)),
            Type::Record(r) if assignable(key, &Type::String, &self.env.definitions) => {
                Some(record_values(&r))
            }
            Type::Server(tools) => Some(union(tools.into_iter().map(|(_, ty)| ty))),
            Type::Union(types) => types
                .iter()
                .map(|t| self.index_display_at(t, key, number, depth + 1, work))
                .collect::<Option<Vec<_>>>()
                .map(union),
            Type::Any => Some(Type::Any),
            _ => None,
        }
    }

    fn index_type(&mut self, ty: &Type, key: &Type, literal: Option<f64>, span: Span) -> Type {
        self.index_type_at(ty, key, literal, span, 0, &mut 16_384)
    }
    fn index_type_at(
        &mut self,
        ty: &Type,
        key: &Type,
        literal: Option<f64>,
        span: Span,
        depth: usize,
        work: &mut usize,
    ) -> Type {
        if depth >= 64 || *work == 0 {
            self.error(
                "cannot resolve recursive index type",
                span,
                "use a schema with a concrete array or record shape",
            );
            return Type::Any;
        }
        *work -= 1;
        match resolve(ty, &self.env.definitions) {
            Type::Any => Type::Any,
            Type::Union(types) => union(
                types
                    .iter()
                    .map(|ty| self.index_type_at(ty, key, literal, span, depth + 1, work))
                    .collect::<Vec<_>>(),
            ),
            Type::Array(item) => {
                self.require(key, &Type::number(), span, "array index");
                *item
            }
            Type::Tuple(tuple) => {
                self.require(key, &Type::number(), span, "tuple index");
                if let Some(n) = literal {
                    if n < 0.0
                        || n.fract() != 0.0
                        || (n >= tuple.items.len() as f64 && tuple.rest.is_none())
                    {
                        self.error(
                            format!("tuple index {n} out of range for {ty}"),
                            span,
                            "use a valid literal tuple position",
                        );
                        return Type::Any;
                    }
                    return tuple
                        .items
                        .get(n as usize)
                        .map(|i| i.ty.clone())
                        .or_else(|| tuple.rest.as_deref().cloned())
                        .unwrap_or(Type::Any);
                }
                element_type(ty, &self.env.definitions).unwrap_or(Type::Any)
            }
            Type::Record(record) => {
                self.require(key, &Type::String, span, "record index");
                record_values(&record)
            }
            Type::Server(tools) => {
                self.require(key, &Type::String, span, "server index");
                union(tools.into_iter().map(|(_, ty)| ty))
            }
            _ => {
                self.error(
                    format!("cannot index {}", widen(ty)),
                    span,
                    "index an array, tuple, record or server",
                );
                Type::Any
            }
        }
    }

    fn binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr) -> ExpressionInfo {
        let a = self.infer(left, None);
        let facts = self.facts.clone();
        if op == BinaryOp::And {
            self.narrow(left, true);
        }
        let b = self.infer(right, None);
        self.facts = facts;
        let boolean = matches!(
            op,
            BinaryOp::Equal
                | BinaryOp::NotEqual
                | BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual
                | BinaryOp::And
                | BinaryOp::Or
        );
        let ty = match op {
            BinaryOp::And | BinaryOp::Or => {
                self.require(&a.ty, &Type::Boolean, left.span, "logical operand");
                self.require(&b.ty, &Type::Boolean, right.span, "logical operand");
                Type::Boolean
            }
            BinaryOp::Equal | BinaryOp::NotEqual => {
                if !equatable(&a.ty, &self.env.definitions)
                    || !equatable(&b.ty, &self.env.definitions)
                {
                    self.error(
                        "equality requires JSON values, not functions, tools or servers",
                        left.span.join(right.span),
                        "compare a JSON field instead",
                    );
                } else if disjoint(&a.display_type, &b.display_type, &self.env.definitions) {
                    self.error(
                        format!(
                            "cannot compare disjoint types {} and {}",
                            a.display_type, b.display_type
                        ),
                        left.span.join(right.span),
                        "compare values of compatible types; MCPL does not coerce values",
                    );
                }
                Type::Boolean
            }
            BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Subtract => {
                self.require(&a.ty, &Type::number(), left.span, "arithmetic operand");
                self.require(&b.ty, &Type::number(), right.span, "arithmetic operand");
                Type::number()
            }
            _ => {
                let numbers = assignable(&a.ty, &Type::number(), &self.env.definitions)
                    && assignable(&b.ty, &Type::number(), &self.env.definitions);
                let strings = assignable(&a.ty, &Type::String, &self.env.definitions)
                    && assignable(&b.ty, &Type::String, &self.env.definitions);
                if !numbers && !strings {
                    self.error(format!("operator '{}' requires two numbers or two strings, found {} and {}", op.spelling(), a.display_type, b.display_type), left.span.join(right.span), "use str(...) for explicit string conversion; MCPL has no implicit coercions");
                }
                if boolean {
                    Type::Boolean
                } else if matches!(a.ty, Type::Any) && matches!(b.ty, Type::Any) {
                    Type::Any
                } else if strings && !numbers {
                    Type::String
                } else {
                    Type::number()
                }
            }
        };
        ExpressionInfo::new(ty)
    }

    fn require(&mut self, actual: &Type, expected: &Type, span: Span, label: &str) {
        if !assignable(actual, expected, &self.env.definitions) {
            self.error(
                format!("{label}: expected {expected}, found {}", widen(actual)),
                span,
                "pass a value of the expected type",
            );
        }
    }

    fn narrow(&mut self, condition: &Expr, truth: bool) {
        let condition = ungroup(condition);
        let ExprKind::Binary { op, left, right } = &condition.kind else {
            return;
        };
        if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
            let target = if is_null(right) {
                Some(left.as_ref())
            } else if *op == BinaryOp::NotEqual && is_null(left) {
                Some(right.as_ref())
            } else {
                None
            };
            if let Some(target) = target
                && let Some(info) = self.expressions.get(&target.span)
                && let Some(path) = &info.path
            {
                let keep_null = (*op == BinaryOp::Equal) == truth;
                let ty = resolve(&info.ty, &self.env.definitions);
                if !matches!(ty, Type::Any) {
                    let variants = alternatives(ty)
                        .into_iter()
                        .filter(|t| is_null_type(t) == keep_null)
                        .collect::<Vec<_>>();
                    if !variants.is_empty() {
                        self.facts.insert(path.clone(), union(variants));
                    }
                }
                return;
            }
        }
        if *op != BinaryOp::Equal {
            return;
        }
        let Some(literal) = literal_string(right) else {
            return;
        };
        let Some(info) = self.expressions.get(&left.span) else {
            return;
        };
        let Some(mut path) = info.path.clone() else {
            return;
        };
        let Some(key) = path.keys.pop() else {
            return;
        };
        let Some(ty) = self.path_type(&path) else {
            return;
        };
        let ty = resolve(&ty, &self.env.definitions);
        let Type::Union(variants) = ty else {
            return;
        };
        let matches = |ty: &Type| -> Option<bool> {
            match property_type(ty, &key, &self.env.definitions)? {
                Type::Literal(Value::String(value)) => Some(value == literal),
                _ => None,
            }
        };
        if variants.iter().any(|ty| matches(ty).is_none()) {
            return;
        }
        let kept: Vec<_> = variants
            .into_iter()
            .filter(|ty| matches(ty) == Some(truth))
            .collect();
        if !kept.is_empty() {
            self.facts.insert(path, union(kept));
        }
    }

    fn path_type(&self, path: &ValuePath) -> Option<Type> {
        if let Some(ty) = self.facts.get(path) {
            return Some(ty.clone());
        }
        let mut ty = match &path.root {
            Root::Session(name) => self.globals.get(name)?.ty.clone(),
            Root::Local { scope, name } => self
                .scopes
                .iter()
                .find(|s| s.id == *scope)?
                .bindings
                .get(name)?
                .ty
                .clone(),
            Root::Server(name) => self.env.servers.get(name)?.clone(),
            Root::Builtin(_) => return None,
        };
        let mut prefix = ValuePath {
            root: path.root.clone(),
            keys: vec![],
        };
        if let Some(narrowed) = self.facts.get(&prefix) {
            ty = narrowed.clone();
        }
        for key in &path.keys {
            prefix.keys.push(key.clone());
            if let Some(narrowed) = self.facts.get(&prefix) {
                ty = narrowed.clone();
                continue;
            }
            ty = path_member(&ty, key, &self.env.definitions, 0)?;
        }
        Some(ty)
    }

    fn call(&mut self, ty: &Type, arguments: &[Expr], span: Span) -> ExpressionInfo {
        let variants = alternatives(resolve(ty, &self.env.definitions));
        let mut results = vec![];
        let mut signatures = vec![];
        let mut inferred_by_builtin = false;
        for variant in variants {
            if let Type::Builtin(builtin) = &variant
                && let Some(builtin) = self.env.builtins.get(&builtin.name)
            {
                results.push(self.builtin_call(builtin.clone(), arguments, span).ty);
                inferred_by_builtin = true;
                continue;
            }
            if matches!(variant, Type::Any) {
                results.push(Type::Any);
                continue;
            }
            let Some(signature) = function(&resolve(&variant, &self.env.definitions)) else {
                self.error(
                    format!("{} is not callable", widen(&variant)),
                    span,
                    "call a tool, builtin or lambda",
                );
                results.push(Type::Any);
                continue;
            };
            self.arity(
                arguments.len(),
                signature.parameters.len(),
                signature.parameters.len(),
                span,
            );
            results.push(*signature.result.clone());
            signatures.push(signature);
        }
        if !signatures.is_empty() || !inferred_by_builtin {
            for (i, argument) in arguments.iter().enumerate() {
                let expected: Vec<_> = signatures
                    .iter()
                    .filter_map(|s| s.parameters.get(i).map(|p| &p.ty))
                    .collect();
                let context = call_context(&expected, &self.env.definitions);
                let actual = self.infer(argument, context.as_ref());
                for expected in expected {
                    self.argument(argument, &actual.ty, expected);
                }
            }
        }
        ExpressionInfo::new(union(results))
    }

    fn arity(&mut self, found: usize, min: usize, max: usize, span: Span) {
        if found < min || found > max {
            let expected = if min == max {
                min.to_string()
            } else {
                format!("{min} to {max}")
            };
            self.error(
                format!("expected {expected} arguments, found {found}"),
                span,
                "use help to inspect the callable signature",
            );
        }
    }

    fn argument(&mut self, expression: &Expr, actual: &Type, expected: &Type) {
        if assignable(actual, expected, &self.env.definitions) {
            return;
        }
        let expected = self.object_context(
            expected,
            match &ungroup(expression).kind {
                ExprKind::Object(fields) => fields,
                _ => &[],
            },
        );
        if let ExprKind::Object(fields) = &ungroup(expression).kind
            && let Type::Record(record) = &expected
        {
            for field in &record.fields {
                if !field.optional && !fields.iter().any(|f| f.name.text == field.name) {
                    self.error(
                        format!("missing required argument field '{}'", field.name),
                        expression.span,
                        format!("add '{}: {}' to the argument object", field.name, field.ty),
                    );
                }
            }
            for field in fields {
                let actual = self
                    .expressions
                    .get(&field.value.span)
                    .map(|i| i.ty.clone())
                    .unwrap_or(Type::Any);
                let expected = record
                    .fields
                    .iter()
                    .find(|f| f.name == field.name.text)
                    .map(|f| &f.ty)
                    .or(record.additional.as_deref());
                match expected {
                    None => self.error(
                        format!("unexpected argument field '{}'", field.name.text),
                        field.name.span,
                        "remove this field or inspect the tool signature with help",
                    ),
                    Some(expected) if !assignable(&actual, expected, &self.env.definitions) => {
                        let hint = if field.name.text == "url" && matches!(expected, Type::String) {
                            "use a URL string, for example \"https://example.com\""
                        } else {
                            "pass a value of the expected type"
                        };
                        self.error(
                            format!(
                                "argument field '{}': expected {expected}, found {}",
                                field.name.text,
                                widen(&actual)
                            ),
                            field.value.span,
                            hint,
                        );
                    }
                    _ => {}
                }
            }
        } else {
            self.require(actual, &expected, expression.span, "argument");
        }
    }
}

fn binding_info(binding: &Binding) -> ExpressionInfo {
    let mut info = ExpressionInfo::new(binding.ty.clone());
    info.display_type = binding.display_type.clone();
    info.builtin = binding.builtin.clone();
    info
}
fn joined(parts: &[Name]) -> String {
    parts
        .iter()
        .map(|n| n.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}
fn record(fields: Vec<Field>) -> Type {
    Type::Record(RecordType {
        fields,
        additional: None,
    })
}
fn put_field(fields: &mut Vec<Field>, name: &str, ty: Type) {
    if let Some(field) = fields.iter_mut().find(|f| f.name == name) {
        field.ty = ty;
    } else {
        fields.push(Field {
            name: name.into(),
            ty,
            optional: false,
        });
    }
}
fn tuple_type(types: Vec<Type>) -> Type {
    Type::Tuple(TupleType {
        items: types
            .into_iter()
            .map(|ty| TupleItem {
                ty,
                optional: false,
            })
            .collect(),
        rest: None,
    })
}
fn function(ty: &Type) -> Option<FunctionType> {
    match ty {
        Type::Function(f) => Some(f.clone()),
        Type::Tool(t) => Some(t.signature.clone()),
        Type::Builtin(b) => Some(b.signature.clone()),
        _ => None,
    }
}
fn ungroup(mut expr: &Expr) -> &Expr {
    while let ExprKind::Group(inner) = &expr.kind {
        expr = inner;
    }
    expr
}
fn literal_string(expr: &Expr) -> Option<&str> {
    if let ExprKind::Literal(Value::String(s)) = &ungroup(expr).kind {
        Some(s)
    } else {
        None
    }
}
fn literal_number(expr: &Expr) -> Option<f64> {
    match &ungroup(expr).kind {
        ExprKind::Literal(Value::Number(n)) => n.as_f64(),
        ExprKind::Unary {
            op: UnaryOp::Negate,
            operand,
        } => literal_number(operand).map(|n| -n),
        _ => None,
    }
}
fn is_null(expr: &Expr) -> bool {
    matches!(ungroup(expr).kind, ExprKind::Literal(Value::Null))
}
fn is_null_type(ty: &Type) -> bool {
    matches!(ty, Type::Null | Type::Literal(Value::Null))
}
fn alternatives(ty: Type) -> Vec<Type> {
    if let Type::Union(types) = ty {
        types
    } else {
        vec![ty]
    }
}
fn known_property(ty: &Type, key: &str, defs: &BTreeMap<String, Type>) -> bool {
    property_flag(ty, key, defs, false, &mut 16_384, 0)
}
fn optional_property(ty: &Type, key: &str, defs: &BTreeMap<String, Type>) -> bool {
    property_flag(ty, key, defs, true, &mut 16_384, 0)
}
fn property_flag(
    ty: &Type,
    key: &str,
    defs: &BTreeMap<String, Type>,
    optional: bool,
    work: &mut usize,
    depth: usize,
) -> bool {
    if *work == 0 || depth >= 64 {
        return false;
    }
    *work -= 1;
    match resolve(ty, defs) {
        Type::Record(r) => r
            .fields
            .iter()
            .any(|f| f.name == key && (!optional || f.optional)),
        Type::Server(tools) => !optional && tools.iter().any(|(n, _)| n == key),
        Type::Union(types) if optional => types
            .iter()
            .any(|t| property_flag(t, key, defs, optional, work, depth + 1)),
        Type::Union(types) => types
            .iter()
            .all(|t| property_flag(t, key, defs, optional, work, depth + 1)),
        _ => false,
    }
}

fn known_fields(ty: &Type) -> Vec<String> {
    match ty {
        Type::Record(r) => r.fields.iter().map(|f| f.name.clone()).collect(),
        Type::Server(t) => t.iter().map(|(n, _)| n.clone()).collect(),
        _ => vec![],
    }
}
fn record_values(record: &RecordType) -> Type {
    union(
        record
            .fields
            .iter()
            .map(|f| {
                if f.optional {
                    union([f.ty.clone(), Type::Null])
                } else {
                    f.ty.clone()
                }
            })
            .chain(record.additional.as_deref().cloned()),
    )
}
fn path_label(path: &ValuePath) -> String {
    let name = match &path.root {
        Root::Session(n) | Root::Builtin(n) | Root::Server(n) | Root::Local { name: n, .. } => n,
    };
    std::iter::once(name.as_str())
        .chain(path.keys.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(".")
}
fn suggestion(name: &str, names: Vec<String>) -> Option<String> {
    names
        .into_iter()
        .map(|candidate| (distance(name, &candidate), candidate))
        .filter(|(d, _)| *d <= 3)
        .min()
        .map(|(_, n)| n)
}
fn distance(a: &str, b: &str) -> usize {
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.bytes().enumerate() {
        let mut old = row[0];
        row[0] = i + 1;
        for (j, y) in b.bytes().enumerate() {
            let saved = row[j + 1];
            row[j + 1] = (row[j] + 1).min(saved + 1).min(old + usize::from(x != y));
            old = saved;
        }
    }
    row[b.len()]
}
fn type_size(ty: &Type, limit: usize) -> usize {
    let mut stack = vec![ty];
    let mut count = 0;
    while let Some(ty) = stack.pop() {
        count += 1;
        if count >= limit {
            return count;
        }
        match ty {
            Type::Array(t) => stack.push(t),
            Type::Tuple(t) => {
                stack.extend(t.items.iter().map(|i| &i.ty));
                stack.extend(t.rest.as_deref());
            }
            Type::Record(r) => {
                stack.extend(r.fields.iter().map(|f| &f.ty));
                stack.extend(r.additional.as_deref());
            }
            Type::Union(ts) => stack.extend(ts),
            Type::Server(ts) => stack.extend(ts.iter().map(|(_, t)| t)),
            Type::Function(f) => {
                stack.extend(f.parameters.iter().map(|p| &p.ty));
                stack.push(&f.result);
            }
            Type::Tool(t) => {
                stack.extend(t.signature.parameters.iter().map(|p| &p.ty));
                stack.push(&t.signature.result);
            }
            Type::Builtin(b) => {
                stack.extend(b.signature.parameters.iter().map(|p| &p.ty));
                stack.push(&b.signature.result);
            }
            _ => {}
        }
    }
    count
}

impl Checker<'_> {
    fn builtin_call(&mut self, builtin: Builtin, arguments: &[Expr], span: Span) -> ExpressionInfo {
        self.arity(arguments.len(), builtin.min_arity, builtin.max_arity, span);
        if builtin.name == "len" {
            let mut input = Type::Any;
            for argument in arguments {
                input = self.infer(argument, None).ty;
            }
            if !self.length_type(&input) {
                self.error(
                    format!(
                        "len expects an array, string or record, found {}",
                        widen(&input)
                    ),
                    span,
                    "pass a collection or string",
                );
            }
            return ExpressionInfo::new(Type::number());
        }
        if builtin.name == "contains" {
            let infos: Vec<_> = arguments.iter().map(|a| self.infer(a, None)).collect();
            if infos.len() == 2 {
                let a = &infos[0].ty;
                let b = &infos[1].ty;
                if assignable(a, &Type::String, &self.env.definitions) {
                    self.require(b, &Type::String, arguments[1].span, "argument");
                } else if let Some(element) = element_type(a, &self.env.definitions) {
                    self.require(b, &widen(&element), arguments[1].span, "argument");
                    if !equatable(&element, &self.env.definitions)
                        || !equatable(b, &self.env.definitions)
                    {
                        self.error(
                            "contains requires JSON array elements",
                            span,
                            "compare JSON values, not capabilities",
                        );
                    }
                } else {
                    self.error(
                        format!("contains expects a string or array, found {}", widen(a)),
                        arguments[0].span,
                        "pass a string or array as the first argument",
                    );
                }
            }
            return ExpressionInfo::new(Type::Boolean);
        }
        let mut substitutions = BTreeMap::new();
        let mut infos: Vec<Option<ExpressionInfo>> = vec![None; arguments.len()];
        // Infer data arguments before contextual callbacks; reduce's accumulator comes last.
        for (i, argument) in arguments.iter().enumerate() {
            let template = builtin.signature.parameters.get(i).map(|p| &p.ty);
            if template.is_some_and(|ty| matches!(ty, Type::Function(_))) {
                continue;
            }
            let context = template.map(|ty| substitute(ty, &substitutions));
            let info = self.infer(argument, context.as_ref());
            if let Some(template) = template {
                bind_generics(
                    template,
                    &info.display_type,
                    &mut substitutions,
                    &self.env.definitions,
                );
            }
            infos[i] = Some(info);
        }
        for (i, argument) in arguments.iter().enumerate() {
            if infos[i].is_some() {
                continue;
            }
            let template = &builtin.signature.parameters[i].ty;
            let context = substitute(template, &substitutions);
            let info = self.infer(argument, Some(&context));
            if let (Type::Function(template), Some(actual)) = (
                template,
                function(&resolve(&info.display_type, &self.env.definitions)),
            ) {
                // Result-only variables are inferred from callbacks, without relaxing inputs.
                bind_generics(
                    &template.result,
                    &actual.result,
                    &mut substitutions,
                    &self.env.definitions,
                );
            }
            infos[i] = Some(info);
        }
        for (i, argument) in arguments.iter().enumerate() {
            let Some(parameter) = builtin.signature.parameters.get(i) else {
                continue;
            };
            let expected = substitute(&parameter.ty, &substitutions);
            let actual = &infos[i].as_ref().unwrap().ty;
            self.argument(argument, actual, &expected);
        }
        if builtin.name == "sort_by"
            && let Some(key) = substitutions.get("K")
        {
            let numeric = assignable(key, &Type::number(), &self.env.definitions);
            let string = assignable(key, &Type::String, &self.env.definitions);
            if !numeric && !string {
                self.error(
                    format!("sort_by key must be number or string, found {key}"),
                    span,
                    "return keys of one primitive kind",
                );
            }
        }
        if builtin.name == "unique"
            && let Some(element) = substitutions.get("T")
            && !equatable(element, &self.env.definitions)
        {
            self.error(
                "unique requires JSON array elements",
                span,
                "project capabilities to JSON fields before comparing",
            );
        }
        ExpressionInfo::new(substitute(&builtin.signature.result, &substitutions))
    }

    fn length_type(&self, ty: &Type) -> bool {
        self.length_type_at(ty, 0)
    }
    fn length_type_at(&self, ty: &Type, depth: usize) -> bool {
        if depth >= 64 {
            return false;
        }
        match resolve(ty, &self.env.definitions) {
            Type::Any
            | Type::Array(_)
            | Type::Tuple(_)
            | Type::Record(_)
            | Type::String
            | Type::Literal(Value::String(_)) => true,
            Type::Union(types) => types.iter().all(|ty| self.length_type_at(ty, depth + 1)),
            _ => false,
        }
    }
}

fn generic(name: &str) -> bool {
    matches!(name, "T" | "U" | "A" | "B" | "K")
}

fn substitute(ty: &Type, substitutions: &BTreeMap<String, Type>) -> Type {
    match ty {
        Type::Named(name) if generic(name) => substitutions.get(name).cloned().unwrap_or(Type::Any),
        Type::Array(item) => Type::Array(Box::new(substitute(item, substitutions))),
        Type::Tuple(tuple) => Type::Tuple(TupleType {
            items: tuple
                .items
                .iter()
                .map(|i| TupleItem {
                    ty: substitute(&i.ty, substitutions),
                    optional: i.optional,
                })
                .collect(),
            rest: tuple
                .rest
                .as_ref()
                .map(|t| Box::new(substitute(t, substitutions))),
        }),
        Type::Record(record) => Type::Record(RecordType {
            fields: record
                .fields
                .iter()
                .map(|f| Field {
                    name: f.name.clone(),
                    ty: substitute(&f.ty, substitutions),
                    optional: f.optional,
                })
                .collect(),
            additional: record
                .additional
                .as_ref()
                .map(|t| Box::new(substitute(t, substitutions))),
        }),
        Type::Function(signature) => Type::Function(FunctionType {
            parameters: signature
                .parameters
                .iter()
                .map(|p| Parameter {
                    name: p.name.clone(),
                    ty: substitute(&p.ty, substitutions),
                })
                .collect(),
            result: Box::new(substitute(&signature.result, substitutions)),
        }),
        Type::Union(types) => union(types.iter().map(|ty| substitute(ty, substitutions))),
        _ => ty.clone(),
    }
}

fn bind_generics(
    template: &Type,
    actual: &Type,
    substitutions: &mut BTreeMap<String, Type>,
    definitions: &BTreeMap<String, Type>,
) {
    match template {
        Type::Named(name) if generic(name) => {
            substitutions
                .entry(name.clone())
                .or_insert_with(|| actual.clone());
        }
        Type::Array(item) => {
            if let Some(element) = element_type(actual, definitions) {
                bind_generics(item, &element, substitutions, definitions);
            }
        }
        Type::Tuple(tuple) => {
            if let Type::Tuple(actual) = resolve(actual, definitions) {
                for (template, actual) in tuple.items.iter().zip(actual.items) {
                    bind_generics(&template.ty, &actual.ty, substitutions, definitions);
                }
            }
        }
        Type::Record(record) => {
            if let Type::Record(actual) = resolve(actual, definitions)
                && let Some(additional) = &record.additional
            {
                let values = union(
                    actual
                        .fields
                        .iter()
                        .map(|f| f.ty.clone())
                        .chain(actual.additional.as_deref().cloned()),
                );
                bind_generics(additional, &values, substitutions, definitions);
            }
        }
        _ => {}
    }
}

impl Checker<'_> {
    fn contextual_builtin(
        &mut self,
        builtin: Builtin,
        context: &FunctionType,
        span: Span,
    ) -> FunctionType {
        self.arity(
            context.parameters.len(),
            builtin.min_arity,
            builtin.max_arity,
            span,
        );
        let mut substitutions = BTreeMap::new();
        for (template, parameter) in builtin.signature.parameters.iter().zip(&context.parameters) {
            bind_generics(
                &template.ty,
                &parameter.ty,
                &mut substitutions,
                &self.env.definitions,
            );
        }
        for (template, parameter) in builtin.signature.parameters.iter().zip(&context.parameters) {
            self.require(
                &parameter.ty,
                &substitute(&template.ty, &substitutions),
                span,
                "callback input",
            );
        }
        if builtin.name == "len"
            && let Some(parameter) = context.parameters.first()
            && !self.length_type(&parameter.ty)
        {
            self.error(
                format!(
                    "len expects an array, string or record, found {}",
                    parameter.ty
                ),
                span,
                "use a callback accepting the element type",
            );
        }
        if builtin.name == "contains" && context.parameters.len() == 2 {
            let first = &context.parameters[0].ty;
            let second = &context.parameters[1].ty;
            let expected = if assignable(first, &Type::String, &self.env.definitions) {
                Some(Type::String)
            } else {
                element_type(first, &self.env.definitions)
            };
            if let Some(expected) = expected {
                self.require(second, &expected, span, "callback input");
            } else {
                self.error(
                    "contains callback expects a string or array",
                    span,
                    "use a callback accepting the element type",
                );
            }
        }
        FunctionType {
            parameters: context.parameters.clone(),
            result: Box::new(substitute(&builtin.signature.result, &substitutions)),
        }
    }
}

fn callable_result(ty: &Type, definitions: &BTreeMap<String, Type>) -> Option<Type> {
    callable_result_at(ty, definitions, 0)
}
fn callable_result_at(
    ty: &Type,
    definitions: &BTreeMap<String, Type>,
    depth: usize,
) -> Option<Type> {
    if depth >= 64 {
        return None;
    }
    match resolve(ty, definitions) {
        Type::Function(signature) => Some(*signature.result),
        Type::Tool(tool) => Some(*tool.signature.result),
        Type::Any => Some(Type::Any),
        Type::Union(types) => types
            .iter()
            .map(|ty| callable_result_at(ty, definitions, depth + 1))
            .collect::<Option<Vec<_>>>()
            .map(union),
        _ => None,
    }
}

fn call_context(types: &[&Type], definitions: &BTreeMap<String, Type>) -> Option<Type> {
    if types.is_empty() {
        return None;
    }
    let signatures: Option<Vec<_>> = types
        .iter()
        .map(|ty| function(&resolve(ty, definitions)))
        .collect();
    if let Some(signatures) = signatures
        && signatures
            .iter()
            .all(|signature| signature.parameters.len() == signatures[0].parameters.len())
    {
        let parameters = signatures[0]
            .parameters
            .iter()
            .enumerate()
            .map(|(i, parameter)| Parameter {
                name: parameter.name.clone(),
                ty: union(signatures.iter().map(|s| s.parameters[i].ty.clone())),
            })
            .collect();
        return Some(Type::Function(FunctionType {
            parameters,
            result: Box::new(union(signatures.into_iter().map(|s| *s.result))),
        }));
    }
    Some(union(types.iter().map(|ty| (*ty).clone())))
}

fn path_member(
    ty: &Type,
    key: &str,
    definitions: &BTreeMap<String, Type>,
    depth: usize,
) -> Option<Type> {
    if depth >= 64 {
        return None;
    }
    match resolve(ty, definitions) {
        Type::Array(item) => Some(*item),
        Type::Tuple(tuple) => tuple
            .items
            .get(key.parse::<usize>().ok()?)
            .map(|i| i.ty.clone())
            .or_else(|| tuple.rest.as_deref().cloned()),
        Type::Union(types) => types
            .iter()
            .map(|ty| path_member(ty, key, definitions, depth + 1))
            .collect::<Option<Vec<_>>>()
            .map(union),
        _ => property_type(ty, key, definitions),
    }
}
