use super::limits::{Budget, MAX_DEPTH, MAX_MEMORY, limit, runtime};
use crate::{
    frontend::{Diagnostic, Span, checker::ExpressionInfo},
    types::{Field, RecordType, Type, union},
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct Code {
    pub source: Arc<str>,
    pub expressions: BTreeMap<Span, ExpressionInfo>,
    pub definitions: BTreeMap<String, Type>,
    pub bytes: usize,
}
impl Code {
    pub fn new(source: Arc<str>, expressions: BTreeMap<Span, ExpressionInfo>) -> Self {
        let bytes = source.len()
            + expressions
                .values()
                .map(|i| type_bytes(&i.ty) + type_bytes(&i.display_type) + 256)
                .sum::<usize>();
        Self {
            source,
            expressions,
            definitions: BTreeMap::new(),
            bytes,
        }
    }
    pub fn with_definitions(mut self, definitions: BTreeMap<String, Type>) -> Self {
        self.bytes += definitions
            .iter()
            .map(|(k, t)| k.len() + type_bytes(t))
            .sum::<usize>();
        self.definitions = definitions;
        self
    }
}
#[derive(Clone, Debug)]
pub struct Closure {
    pub parameters: Vec<String>,
    pub body: Arc<crate::frontend::Expr>,
    pub captures: BTreeMap<String, Value>,
    pub globals: BTreeMap<String, Value>,
    pub code: Arc<Code>,
}
#[derive(Debug)]
pub enum ValueKind {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
    Closure(Closure),
    Builtin(String),
    Server(String),
    Tool { server: String, name: String },
}
#[derive(Debug)]
pub struct ValueData {
    pub kind: ValueKind,
    pub ty: Arc<Type>,
    pub depth: usize,
    pub logical_bytes: usize,
    pub own_bytes: usize,
    pub capture_depth: usize,
}
#[derive(Clone, Debug)]
pub struct Value(pub Arc<ValueData>);
impl Value {
    pub fn retained_bytes(values: &[Value]) -> usize {
        retained_bytes(values)
    }
    pub fn server(name: String, ty: Type) -> Self {
        Self::new(
            ValueKind::Server(name),
            ty,
            &mut Budget::default(),
            Span::default(),
        )
        .expect("server capability")
    }
    pub fn builtin(name: String, ty: Type) -> Self {
        Self::new(
            ValueKind::Builtin(name),
            ty,
            &mut Budget::default(),
            Span::default(),
        )
        .expect("builtin capability")
    }
    pub fn kind(&self) -> &ValueKind {
        &self.0.kind
    }
    pub fn display_type(&self) -> &Type {
        &self.0.ty
    }
    pub fn depth(&self) -> usize {
        self.0.depth
    }
    pub fn logical_bytes(&self) -> usize {
        self.0.logical_bytes
    }
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    pub fn new(
        kind: ValueKind,
        ty: Type,
        budget: &mut Budget,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        let mut own = 64usize + type_bytes(&ty);
        let mut depth = 0;
        let mut logical = 0usize;
        let mut capture_depth = 0usize;
        match &kind {
            ValueKind::String(s) => {
                budget.string(s.len(), span)?;
                own += s.len();
            }
            ValueKind::Array(xs) => {
                budget.items(xs.len(), span)?;
                own += xs.len() * 8;
                depth = 1;
                for x in xs {
                    capture_depth = capture_depth.max(1 + x.0.capture_depth);
                    depth = depth.max(1 + x.depth());
                    logical = logical.saturating_add(x.logical_bytes());
                }
            }
            ValueKind::Object(xs) => {
                budget.items(xs.len(), span)?;
                own += xs.len() * 32;
                depth = 1;
                for (k, x) in xs {
                    capture_depth = capture_depth.max(1 + x.0.capture_depth);
                    budget.string(k.len(), span)?;
                    own += k.len();
                    depth = depth.max(1 + x.depth());
                    logical = logical.saturating_add(x.logical_bytes());
                }
            }
            ValueKind::Closure(c) => {
                own +=
                    c.parameters.iter().map(|x| x.len()).sum::<usize>() + expression_bytes(&c.body);
                for (k, v) in c.captures.iter().chain(c.globals.iter()) {
                    capture_depth = capture_depth.max(1 + v.0.capture_depth);
                    own += k.len() + 32;
                    logical = logical.saturating_add(v.logical_bytes());
                }
            }
            ValueKind::Builtin(s) | ValueKind::Server(s) => own += s.len(),
            ValueKind::Tool { server, name } => own += server.len() + name.len(),
            _ => (),
        }
        if capture_depth > MAX_DEPTH {
            return Err(limit("closure capture depth limit (64)", span));
        }
        if depth > MAX_DEPTH {
            return Err(limit("value nesting depth limit (64)", span));
        }
        logical = logical.saturating_add(own);
        if let ValueKind::Closure(c) = &kind {
            logical = logical.saturating_add(c.code.bytes);
        }
        budget.allocation(own, span)?;
        Ok(Self(Arc::new(ValueData {
            kind,
            ty: Arc::new(ty),
            depth,
            logical_bytes: logical,
            own_bytes: own,
            capture_depth,
        })))
    }
    pub fn null() -> Self {
        Self(Arc::new(ValueData {
            kind: ValueKind::Null,
            ty: Arc::new(Type::Null),
            depth: 0,
            logical_bytes: 64,
            own_bytes: 64,
            capture_depth: 0,
        }))
    }
    pub fn boolean(b: bool) -> Self {
        Self(Arc::new(ValueData {
            kind: ValueKind::Bool(b),
            ty: Arc::new(Type::Boolean),
            depth: 0,
            logical_bytes: 64,
            own_bytes: 64,
            capture_depth: 0,
        }))
    }
    pub fn number(n: f64, span: Span) -> Result<Self, Diagnostic> {
        if !n.is_finite() {
            return Err(runtime("non-finite numeric result", span));
        }
        Ok(Self(Arc::new(ValueData {
            kind: ValueKind::Number(n),
            ty: Arc::new(Type::number()),
            depth: 0,
            logical_bytes: 64,
            own_bytes: 64,
            capture_depth: 0,
        })))
    }
    pub fn string(s: String, budget: &mut Budget, span: Span) -> Result<Self, Diagnostic> {
        Self::new(ValueKind::String(s), Type::String, budget, span)
    }
    pub fn array(xs: Vec<Value>, budget: &mut Budget, span: Span) -> Result<Self, Diagnostic> {
        let ty = Type::Array(Box::new(array_type(&xs, budget, span)?));
        Self::new(ValueKind::Array(xs), ty, budget, span)
    }
    pub fn object(
        xs: Vec<(String, Value)>,
        budget: &mut Budget,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        let total = xs.iter().fold(0usize, |n, (_, v)| {
            n.saturating_add(type_bytes(v.display_type()))
        });
        if total > MAX_MEMORY / 4 {
            return Err(limit("runtime type size limit", span));
        }
        budget.charge(total / 64, span)?;
        let ty = Type::Record(RecordType {
            fields: xs
                .iter()
                .map(|(k, v)| Field {
                    name: k.clone(),
                    ty: v.display_type().clone(),
                    optional: false,
                })
                .collect(),
            additional: None,
        });
        Self::new(ValueKind::Object(xs), ty, budget, span)
    }
    pub fn as_number(&self, span: Span) -> Result<f64, Diagnostic> {
        if let ValueKind::Number(n) = self.kind() {
            Ok(*n)
        } else {
            Err(runtime("expected number", span))
        }
    }
    pub fn as_bool(&self, span: Span) -> Result<bool, Diagnostic> {
        if let ValueKind::Bool(b) = self.kind() {
            Ok(*b)
        } else {
            Err(runtime("expected boolean; MCPL has no truthiness", span))
        }
    }
    pub fn as_str(&self, span: Span) -> Result<&str, Diagnostic> {
        if let ValueKind::String(s) = self.kind() {
            Ok(s)
        } else {
            Err(runtime("expected string", span))
        }
    }
    pub fn as_array(&self, span: Span) -> Result<&[Value], Diagnostic> {
        if let ValueKind::Array(xs) = self.kind() {
            Ok(xs)
        } else {
            Err(runtime("expected array", span))
        }
    }
    pub fn as_object(&self, span: Span) -> Result<&[(String, Value)], Diagnostic> {
        if let ValueKind::Object(xs) = self.kind() {
            Ok(xs)
        } else {
            Err(runtime("expected object", span))
        }
    }
    pub fn from_json(
        json: &serde_json::Value,
        budget: &mut Budget,
        span: Span,
    ) -> Result<Self, Diagnostic> {
        Self::from_json_at(json, budget, span, 0)
    }
    fn from_json_at(
        json: &serde_json::Value,
        budget: &mut Budget,
        span: Span,
        depth: usize,
    ) -> Result<Self, Diagnostic> {
        budget.charge(1, span)?;
        if depth > MAX_DEPTH {
            return Err(limit("value nesting depth limit (64)", span));
        }
        match json {
            serde_json::Value::Null => Ok(Self::null()),
            serde_json::Value::Bool(b) => Ok(Self::boolean(*b)),
            serde_json::Value::Number(n) => Self::number(
                n.as_f64().ok_or_else(|| runtime("invalid number", span))?,
                span,
            ),
            serde_json::Value::String(s) => {
                budget.string(s.len(), span)?;
                budget.charge(s.chars().count(), span)?;
                Self::string(s.clone(), budget, span)
            }
            serde_json::Value::Array(xs) => {
                budget.items(xs.len(), span)?;
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(Self::from_json_at(x, budget, span, depth + 1)?);
                }
                Self::array(out, budget, span)
            }
            serde_json::Value::Object(xs) => {
                budget.items(xs.len(), span)?;
                let mut out = Vec::with_capacity(xs.len());
                for (k, x) in xs {
                    budget.charge(k.chars().count(), span)?;
                    out.push((k.clone(), Self::from_json_at(x, budget, span, depth + 1)?));
                }
                Self::object(out, budget, span)
            }
        }
    }
    pub fn to_json(
        &self,
        budget: &mut Budget,
        span: Span,
        allow_capability: bool,
    ) -> Result<serde_json::Value, Diagnostic> {
        budget.allocation(self.logical_bytes(), span)?;
        self.to_json_inner(budget, span, allow_capability)
    }
    fn to_json_inner(
        &self,
        budget: &mut Budget,
        span: Span,
        allow_capability: bool,
    ) -> Result<serde_json::Value, Diagnostic> {
        budget.charge(1, span)?;
        Ok(match self.kind() {
            ValueKind::Null => serde_json::Value::Null,
            ValueKind::Bool(b) => (*b).into(),
            ValueKind::Number(n) => serde_json::json!(n),
            ValueKind::String(s) => {
                budget.charge(s.chars().count(), span)?;
                s.clone().into()
            }
            ValueKind::Array(xs) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(x.to_json_inner(budget, span, false)?);
                }
                out.into()
            }
            ValueKind::Object(xs) => {
                let mut out = serde_json::Map::new();
                for (k, v) in xs {
                    budget.charge(k.chars().count(), span)?;
                    out.insert(k.clone(), v.to_json_inner(budget, span, false)?);
                }
                out.into()
            }
            _ if allow_capability => self.display_type().to_string().into(),
            _ => return Err(runtime("value contains a non-JSON capability", span)),
        })
    }
}
pub fn retained_bytes<'a>(values: impl IntoIterator<Item = &'a Value>) -> usize {
    let mut seen = HashSet::new();
    let mut codes = HashSet::new();
    let mut pending: Vec<_> = values.into_iter().collect();
    let mut bytes = 0usize;
    while let Some(v) = pending.pop() {
        if !seen.insert(v.identity()) {
            continue;
        }
        bytes = bytes.saturating_add(v.0.own_bytes);
        match v.kind() {
            ValueKind::Array(xs) => pending.extend(xs),
            ValueKind::Object(xs) => pending.extend(xs.iter().map(|(_, v)| v)),
            ValueKind::Closure(c) => {
                if codes.insert(Arc::as_ptr(&c.code) as usize) {
                    bytes = bytes.saturating_add(c.code.bytes);
                }
                pending.extend(c.captures.values().chain(c.globals.values()));
            }
            _ => (),
        }
    }
    bytes
}
pub fn equal(a: &Value, b: &Value, budget: &mut Budget, span: Span) -> Result<bool, Diagnostic> {
    require_json(a, budget, span)?;
    require_json(b, budget, span)?;
    equal_json(a, b, budget, span)
}

fn array_type(xs: &[Value], budget: &mut Budget, span: Span) -> Result<Type, Diagnostic> {
    let mut types: Vec<Type> = Vec::new();
    let mut size = 0usize;
    for value in xs {
        budget.charge(1 + types.len(), span)?;
        if types.iter().any(|t| t == value.display_type()) {
            continue;
        }
        size = size.saturating_add(type_bytes(value.display_type()));
        if size > MAX_MEMORY / 4 {
            return Err(limit("runtime type size limit", span));
        }
        budget.charge(type_bytes(value.display_type()) / 64, span)?;
        types.push(value.display_type().clone());
    }
    Ok(if types.is_empty() {
        Type::Any
    } else {
        union(types)
    })
}
pub fn type_bytes(ty: &Type) -> usize {
    let mut pending = vec![ty];
    let mut size = 0usize;
    while let Some(ty) = pending.pop() {
        size = size.saturating_add(64);
        match ty {
            Type::Array(t) => pending.push(t),
            Type::Union(ts) => pending.extend(ts),
            Type::Tuple(t) => {
                pending.extend(t.items.iter().map(|i| &i.ty));
                if let Some(r) = &t.rest {
                    pending.push(r)
                }
            }
            Type::Record(r) => {
                for f in &r.fields {
                    size = size.saturating_add(f.name.len());
                    pending.push(&f.ty)
                }
                if let Some(a) = &r.additional {
                    pending.push(a)
                }
            }
            Type::Function(f) => {
                pending.push(&f.result);
                for p in &f.parameters {
                    size += p.name.len();
                    pending.push(&p.ty)
                }
            }
            Type::Tool(t) => {
                size += t.server.len() + t.name.len();
                pending.push(&t.signature.result);
                pending.extend(t.signature.parameters.iter().map(|p| &p.ty));
            }
            Type::Builtin(b) => {
                size += b.name.len();
                pending.push(&b.signature.result);
                pending.extend(b.signature.parameters.iter().map(|p| &p.ty));
            }
            Type::Server(ts) => {
                for (k, t) in ts {
                    size += k.len();
                    pending.push(t)
                }
            }
            Type::Named(n) => size += n.len(),
            Type::Literal(v) => size += v.to_string().len(),
            _ => (),
        }
    }
    size
}

fn expression_bytes(expression: &crate::frontend::Expr) -> usize {
    use crate::frontend::ast::ExprKind;
    let mut pending = vec![expression];
    let mut bytes = 0usize;
    while let Some(expr) = pending.pop() {
        bytes += std::mem::size_of::<crate::frontend::Expr>();
        match &expr.kind {
            ExprKind::Literal(v) => bytes += v.to_string().len(),
            ExprKind::OverflowNumber(s) => bytes += s.len(),
            ExprKind::Symbol(names) => {
                bytes += names.iter().map(|n| n.text.len() + 32).sum::<usize>()
            }
            ExprKind::Array(xs) => pending.extend(xs),
            ExprKind::Object(xs) => {
                for f in xs {
                    bytes += f.name.text.len() + 64;
                    pending.push(&f.value)
                }
            }
            ExprKind::Lambda { parameters, body } => {
                bytes += parameters.iter().map(|n| n.text.len() + 32).sum::<usize>();
                pending.push(body)
            }
            ExprKind::Sequence { items, value } => {
                for item in items {
                    bytes += item.name.as_ref().map_or(0, |n| n.text.len() + 32);
                    pending.push(&item.expression)
                }
                pending.push(value)
            }
            ExprKind::Group(expr) | ExprKind::Unary { operand: expr, .. } => pending.push(expr),
            ExprKind::Binary { left, right, .. } => {
                pending.push(left);
                pending.push(right)
            }
            ExprKind::Conditional { condition, yes, no } => {
                pending.push(condition);
                pending.push(yes);
                pending.push(no)
            }
            ExprKind::Call { callee, arguments } => {
                pending.push(callee);
                pending.extend(arguments)
            }
            ExprKind::Index { receiver, index } => {
                pending.push(receiver);
                pending.push(index)
            }
            ExprKind::Member { receiver, members } => {
                pending.push(receiver);
                bytes += members.iter().map(|n| n.text.len() + 32).sum::<usize>();
            }
        }
    }
    bytes
}

pub fn require_json(value: &Value, budget: &mut Budget, span: Span) -> Result<(), Diagnostic> {
    budget.charge(1, span)?;
    match value.kind() {
        ValueKind::Array(xs) => {
            for x in xs {
                require_json(x, budget, span)?
            }
        }
        ValueKind::Object(xs) => {
            for (k, v) in xs {
                budget.charge(k.len(), span)?;
                require_json(v, budget, span)?
            }
        }
        ValueKind::String(s) => budget.charge(s.len(), span)?,
        ValueKind::Null | ValueKind::Bool(_) | ValueKind::Number(_) => (),
        _ => return Err(runtime("value contains a non-JSON capability", span)),
    }
    Ok(())
}
fn equal_json(a: &Value, b: &Value, budget: &mut Budget, span: Span) -> Result<bool, Diagnostic> {
    budget.charge(1, span)?;
    Ok(match (a.kind(), b.kind()) {
        (ValueKind::Null, ValueKind::Null) => true,
        (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
        (ValueKind::Number(a), ValueKind::Number(b)) => a == b,
        (ValueKind::String(a), ValueKind::String(b)) => {
            budget.charge(a.len() + b.len(), span)?;
            a == b
        }
        (ValueKind::Array(a), ValueKind::Array(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (a, b) in a.iter().zip(b) {
                if !equal_json(a, b, budget, span)? {
                    return Ok(false);
                }
            }
            true
        }
        (ValueKind::Object(a), ValueKind::Object(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (k, a) in a {
                budget.charge(b.len() + k.len(), span)?;
                let Some((_, b)) = b.iter().find(|(key, _)| key == k) else {
                    return Ok(false);
                };
                if !equal_json(a, b, budget, span)? {
                    return Ok(false);
                }
            }
            true
        }
        _ => false,
    })
}
