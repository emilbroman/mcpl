use super::{
    builtins,
    limits::{Budget, MAX_ITEMS, MAX_MEMORY, limit, runtime},
    value::{Closure, Code, Value, ValueKind},
};
use crate::{
    frontend::{
        Diagnostic, Expr, Span,
        ast::{BinaryOp, ExprKind, Statement, UnaryOp},
        checker::{MemberAccess, Root},
    },
    registry::Registry,
    types::Type,
};
use std::{collections::BTreeMap, sync::Arc};

pub trait GlobalStore {
    fn lookup(&mut self, name: &str, span: Span) -> Result<Value, Diagnostic>;
    fn reserve(
        &mut self,
        _active: &[Value],
        _temporary_bytes: usize,
        _span: Span,
    ) -> Result<(), Diagnostic> {
        Ok(())
    }
}
pub struct Evaluator<'a, S: GlobalStore> {
    registry: &'a Registry,
    store: &'a mut S,
    pub budget: Budget,
}
#[derive(Clone, Default)]
struct Context {
    locals: Arc<BTreeMap<String, Value>>,
    globals: Arc<BTreeMap<String, Value>>,
    code: Option<Arc<Code>>,
}
impl Context {
    fn code(&self) -> &Arc<Code> {
        self.code.as_ref().unwrap()
    }
}
enum Action {
    Eval(Arc<Expr>, Context),
    Validate(Type, Arc<Code>, Span),
    Unary(UnaryOp, Span),
    BinaryLeft(BinaryOp, Arc<Expr>, Context, Span),
    BinaryRight(BinaryOp, Value, Span),
    Conditional(Arc<Expr>, Arc<Expr>, Context, Span),
    Array(usize, Span),
    Object(Vec<String>, Span),
    Call(Vec<Span>, Context, Span),
    Invoke(Value, Vec<Value>, Context, Span),
    Member(Vec<MemberAccess>),
    Index(bool, Span),
    Sequence(Arc<Vec<Statement>>, usize, Arc<Expr>, Context),
    Local(Arc<Vec<Statement>>, usize, Arc<Expr>, Context),
    Next(Box<Iteration>),
    Iterated(Box<Iteration>),
}
struct Iteration {
    name: String,
    xs: Value,
    callback: Value,
    index: usize,
    out: Vec<Value>,
    keys: Vec<Value>,
    groups: Vec<(String, Vec<Value>)>,
    acc: Value,
    ctx: Context,
    span: Span,
}
impl<'a, S: GlobalStore> Evaluator<'a, S> {
    pub fn new(registry: &'a Registry, store: &'a mut S) -> Self {
        Self {
            registry,
            store,
            budget: Budget::default(),
        }
    }
    pub fn store_mut(&mut self) -> &mut S {
        self.store
    }
    pub async fn eval(&mut self, expression: &Expr, code: Arc<Code>) -> Result<Value, Diagnostic> {
        let original = code.clone();
        let mut actions = vec![Action::Eval(
            Arc::new(expression.clone()),
            Context {
                code: Some(code),
                ..Default::default()
            },
        )];
        let mut values = Vec::<Value>::new();
        let mut current_span = expression.span;
        let mut foreign = false;
        let mut ticks = 0usize;
        let mut last_allocation = self.budget.allocated;
        let result: Result<Value, Diagnostic> = async {
            while let Some(action) = actions.pop() {
                if actions.len().saturating_mul(512) > MAX_MEMORY {
                    return Err(limit(
                        "evaluation temporary memory limit (256 MB)",
                        current_span,
                    ));
                }
                ticks += 1;
                if ticks.is_multiple_of(1024)
                    || self.budget.allocated.saturating_sub(last_allocation) >= 1024 * 1024
                {
                    last_allocation = self.budget.allocated;
                    tokio::task::yield_now().await;
                    self.budget
                        .charge(actions.len() + values.len(), current_span)?;
                    let mut active = values.clone();
                    action.roots(&mut active);
                    for action in &actions {
                        action.roots(&mut active);
                    }
                    self.budget.charge(active.len(), current_span)?;
                    self.store
                        .reserve(&active, actions.len().saturating_mul(512), current_span)?;
                }
                match action {
                    Action::Validate(ty, code, span) => {
                        let value = values.last().unwrap();
                        if !matches_type(value, &ty, &code.definitions, &mut self.budget, span, 0)?
                        {
                            return Err(runtime(
                                format!("function result does not match {ty}"),
                                span,
                            ));
                        }
                    }
                    Action::Eval(expr, ctx) => {
                        current_span = expr.span;
                        foreign = !Arc::ptr_eq(ctx.code(), &original);
                        self.budget.charge(1, expr.span)?;
                        let span = expr.span;
                        match &expr.kind {
                            ExprKind::Literal(json) => {
                                values.push(Value::from_json(json, &mut self.budget, span)?)
                            }
                            ExprKind::OverflowNumber(_) => {
                                return Err(runtime("non-finite numeric literal", span));
                            }
                            ExprKind::Group(inner) => {
                                actions.push(Action::Eval(Arc::new(*inner.clone()), ctx))
                            }
                            ExprKind::Symbol(_) => {
                                let resolution = ctx
                                    .code()
                                    .expressions
                                    .get(&span)
                                    .and_then(|i| i.resolution.as_ref())
                                    .ok_or_else(|| {
                                        runtime("missing checked symbol resolution", span)
                                    })?;
                                let root = resolution
                                    .root
                                    .as_ref()
                                    .ok_or_else(|| runtime("missing checked root", span))?;
                                let mut v = match root {
                                    Root::Local { name, .. } => {
                                        ctx.locals.get(name).cloned().ok_or_else(|| {
                                            runtime(format!("missing local '{name}'"), span)
                                        })?
                                    }
                                    Root::Session(name) => {
                                        if let Some(v) = ctx.globals.get(name) {
                                            v.clone()
                                        } else {
                                            self.store.lookup(name, span)?
                                        }
                                    }
                                    Root::Server(name) | Root::Builtin(name) => {
                                        self.store.lookup(name, span)?
                                    }
                                };
                                for member in &resolution.members {
                                    v = self.member(
                                        v,
                                        &member.key,
                                        member.optional,
                                        Some(&member.ty),
                                        member.span,
                                    )?;
                                }
                                values.push(v);
                            }
                            ExprKind::Array(xs) => {
                                self.budget.items(xs.len(), span)?;
                                actions.push(Action::Array(xs.len(), span));
                                for x in xs.iter().rev() {
                                    actions.push(Action::Eval(Arc::new(x.clone()), ctx.clone()));
                                }
                            }
                            ExprKind::Object(fields) => {
                                self.budget.items(fields.len(), span)?;
                                actions.push(Action::Object(
                                    fields.iter().map(|f| f.name.text.clone()).collect(),
                                    span,
                                ));
                                for f in fields.iter().rev() {
                                    actions
                                        .push(Action::Eval(Arc::new(f.value.clone()), ctx.clone()));
                                }
                            }
                            ExprKind::Unary { op, operand } => {
                                actions.push(Action::Unary(*op, span));
                                actions.push(Action::Eval(Arc::new(*operand.clone()), ctx));
                            }
                            ExprKind::Binary { op, left, right } => {
                                actions.push(Action::BinaryLeft(
                                    *op,
                                    Arc::new(*right.clone()),
                                    ctx.clone(),
                                    span,
                                ));
                                actions.push(Action::Eval(Arc::new(*left.clone()), ctx));
                            }
                            ExprKind::Conditional { condition, yes, no } => {
                                actions.push(Action::Conditional(
                                    Arc::new(*yes.clone()),
                                    Arc::new(*no.clone()),
                                    ctx.clone(),
                                    span,
                                ));
                                actions.push(Action::Eval(Arc::new(*condition.clone()), ctx));
                            }
                            ExprKind::Call { callee, arguments } => {
                                actions.push(Action::Call(
                                    arguments.iter().map(|a| a.span).collect(),
                                    ctx.clone(),
                                    span,
                                ));
                                for arg in arguments.iter().rev() {
                                    actions.push(Action::Eval(Arc::new(arg.clone()), ctx.clone()));
                                }
                                actions.push(Action::Eval(Arc::new(*callee.clone()), ctx));
                            }
                            ExprKind::Member { receiver, .. } => {
                                let members = ctx
                                    .code()
                                    .expressions
                                    .get(&span)
                                    .and_then(|i| i.resolution.as_ref())
                                    .map(|r| r.members.clone())
                                    .ok_or_else(|| {
                                        runtime("missing checked member resolution", span)
                                    })?;
                                actions.push(Action::Member(members));
                                actions.push(Action::Eval(Arc::new(*receiver.clone()), ctx));
                            }
                            ExprKind::Index { receiver, index } => {
                                let mut literal_index = index.as_ref();
                                while let ExprKind::Group(inner) = &literal_index.kind {
                                    literal_index = inner;
                                }
                                let optional =
                                    if let ExprKind::Literal(serde_json::Value::String(key)) =
                                        &literal_index.kind
                                    {
                                        ctx.code().expressions.get(&receiver.span).is_some_and(
                                            |i| {
                                                optional_field(
                                                    &i.ty,
                                                    key,
                                                    &ctx.code().definitions,
                                                    0,
                                                )
                                            },
                                        )
                                    } else {
                                        false
                                    };
                                actions.push(Action::Index(optional, span));
                                actions.push(Action::Eval(Arc::new(*index.clone()), ctx.clone()));
                                actions.push(Action::Eval(Arc::new(*receiver.clone()), ctx));
                            }
                            ExprKind::Sequence { items, value } => actions.push(Action::Sequence(
                                Arc::new(items.clone()),
                                0,
                                Arc::new(*value.clone()),
                                ctx,
                            )),
                            ExprKind::Lambda { parameters, body } => {
                                let mut captures = BTreeMap::new();
                                let mut globals = BTreeMap::new();
                                for (_, info) in ctx.code().expressions.range(
                                    Span::new(body.span.start, 0)..Span::new(body.span.end, 0),
                                ) {
                                    if let Some(root) =
                                        info.resolution.as_ref().and_then(|r| r.root.as_ref())
                                    {
                                        match root {
                                            Root::Session(name) if !globals.contains_key(name) => {
                                                let v = if let Some(v) = ctx.globals.get(name) {
                                                    v.clone()
                                                } else {
                                                    self.store.lookup(name, span)?
                                                };
                                                globals.insert(name.clone(), v);
                                            }
                                            Root::Local { name, .. } => {
                                                if let Some(v) = ctx.locals.get(name) {
                                                    captures.insert(name.clone(), v.clone());
                                                }
                                            }
                                            _ => (),
                                        }
                                    }
                                }
                                let ty = ctx
                                    .code()
                                    .expressions
                                    .get(&span)
                                    .map(|i| i.display_type.clone())
                                    .unwrap_or(Type::Any);
                                values.push(Value::new(
                                    ValueKind::Closure(Closure {
                                        parameters: parameters
                                            .iter()
                                            .map(|p| p.text.clone())
                                            .collect(),
                                        body: Arc::new(*body.clone()),
                                        captures,
                                        globals,
                                        code: ctx.code().clone(),
                                    }),
                                    ty,
                                    &mut self.budget,
                                    span,
                                )?);
                            }
                        }
                    }
                    Action::Unary(op, span) => {
                        let v = values.pop().unwrap();
                        values.push(match op {
                            UnaryOp::Not => Value::boolean(!v.as_bool(span)?),
                            UnaryOp::Negate => Value::number(-v.as_number(span)?, span)?,
                        });
                    }
                    Action::BinaryLeft(op, right, ctx, span) => {
                        let left = values.pop().unwrap();
                        if matches!(op, BinaryOp::And | BinaryOp::Or) {
                            let b = left.as_bool(span)?;
                            if (op == BinaryOp::And && !b) || (op == BinaryOp::Or && b) {
                                values.push(left);
                                continue;
                            }
                        }
                        actions.push(Action::BinaryRight(op, left, span));
                        actions.push(Action::Eval(right, ctx));
                    }
                    Action::BinaryRight(op, left, span) => {
                        let right = values.pop().unwrap();
                        values.push(self.binary(op, left, right, span)?);
                    }
                    Action::Conditional(yes, no, ctx, span) => {
                        let b = values.pop().unwrap().as_bool(span)?;
                        actions.push(Action::Eval(if b { yes } else { no }, ctx));
                    }
                    Action::Array(n, span) => {
                        let xs = values.split_off(values.len() - n);
                        values.push(Value::array(xs, &mut self.budget, span)?);
                    }
                    Action::Object(keys, span) => {
                        let xs = values.split_off(values.len() - keys.len());
                        let mut out: Vec<(String, Value)> = Vec::new();
                        for (k, v) in keys.into_iter().zip(xs) {
                            if let Some((_, old)) = out.iter_mut().find(|(key, _)| key == &k) {
                                *old = v;
                            } else {
                                out.push((k, v));
                            }
                        }
                        values.push(Value::object(out, &mut self.budget, span)?);
                    }
                    Action::Call(argument_spans, ctx, span) => {
                        let args = values.split_off(values.len() - argument_spans.len());
                        let callee = values.pop().unwrap();
                        if matches!(callee.kind(),ValueKind::Builtin(name) if name=="type_of")
                            && args.len() == 1
                            && let Some(info) = ctx.code().expressions.get(&argument_spans[0])
                            && info.ty != Type::Any
                        {
                            values.push(Value::string(
                                info.display_type.to_string(),
                                &mut self.budget,
                                span,
                            )?);
                            continue;
                        }
                        actions.push(Action::Invoke(callee, args, ctx, span));
                    }
                    Action::Member(members) => {
                        let mut v = values.pop().unwrap();
                        for member in members {
                            v = self.member(
                                v,
                                &member.key,
                                member.optional,
                                Some(&member.ty),
                                member.span,
                            )?;
                        }
                        values.push(v);
                    }
                    Action::Index(optional, span) => {
                        let index = values.pop().unwrap();
                        let receiver = values.pop().unwrap();
                        let v = match receiver.kind() {
                            ValueKind::Array(xs) => {
                                let n = index.as_number(span)?;
                                if n < 0.0 || n.fract() != 0.0 {
                                    return Err(runtime(
                                        "array index must be a nonnegative integer",
                                        span,
                                    ));
                                }
                                xs.get(n as usize).cloned().ok_or_else(|| {
                                    runtime(
                                        format!(
                                            "array index {n} out of range for length {}",
                                            xs.len()
                                        ),
                                        span,
                                    )
                                    .with_hint(
                                        if xs.is_empty() {
                                            "the array is empty".into()
                                        } else {
                                            format!("valid indexes are 0 through {}", xs.len() - 1)
                                        },
                                    )
                                })?
                            }
                            _ => {
                                self.member(receiver, index.as_str(span)?, optional, None, span)?
                            }
                        };
                        values.push(v);
                    }
                    Action::Sequence(items, i, last, ctx) => {
                        if i == items.len() {
                            actions.push(Action::Eval(last, ctx));
                        } else {
                            let expression = Arc::new(items[i].expression.clone());
                            actions.push(Action::Local(items, i, last, ctx.clone()));
                            actions.push(Action::Eval(expression, ctx));
                        }
                    }
                    Action::Local(items, i, last, mut ctx) => {
                        let v = values.pop().unwrap();
                        if let Some(name) = &items[i].name {
                            Arc::make_mut(&mut ctx.locals).insert(name.text.clone(), v);
                        }
                        actions.push(Action::Sequence(items, i + 1, last, ctx));
                    }
                    Action::Invoke(callee, args, ctx, span) => match callee.kind() {
                        ValueKind::Closure(c) => {
                            if c.parameters.len() != args.len() {
                                return Err(runtime(
                                    format!(
                                        "expected {} arguments, found {}",
                                        c.parameters.len(),
                                        args.len()
                                    ),
                                    span,
                                ));
                            }
                            if let Type::Function(signature) = callee.display_type() {
                                for (parameter, arg) in signature.parameters.iter().zip(&args) {
                                    if !matches_type(
                                        arg,
                                        &parameter.ty,
                                        &c.code.definitions,
                                        &mut self.budget,
                                        span,
                                        0,
                                    )? {
                                        return Err(runtime(
                                            format!(
                                                "argument '{}': expected {}, found {}",
                                                parameter.name,
                                                parameter.ty,
                                                arg.display_type()
                                            ),
                                            span,
                                        ));
                                    }
                                }
                                if *signature.result != Type::Any {
                                    actions.push(Action::Validate(
                                        *signature.result.clone(),
                                        c.code.clone(),
                                        span,
                                    ));
                                }
                            }
                            self.budget
                                .charge(c.captures.len() + c.globals.len() + args.len(), span)?;
                            let mut locals = c.captures.clone();
                            for (name, v) in c.parameters.iter().zip(args) {
                                locals.insert(name.clone(), v);
                            }
                            actions.push(Action::Eval(
                                c.body.clone(),
                                Context {
                                    locals: Arc::new(locals),
                                    globals: Arc::new(c.globals.clone()),
                                    code: Some(c.code.clone()),
                                },
                            ));
                        }
                        ValueKind::Builtin(name) => {
                            if matches!(
                                name.as_str(),
                                "map"
                                    | "filter"
                                    | "reduce"
                                    | "flat_map"
                                    | "find"
                                    | "any"
                                    | "all"
                                    | "sort_by"
                                    | "group_by"
                            ) {
                                let arity = if name == "reduce" { 3 } else { 2 };
                                if args.len() != arity {
                                    return Err(runtime(
                                        format!("{name} expects {arity} arguments"),
                                        span,
                                    ));
                                }
                                args[0].as_array(span)?;
                                let callback_arity = if name == "reduce" { 2 } else { 1 };
                                let accepts = match args[1].kind() {
                                    ValueKind::Closure(c) => c.parameters.len() == callback_arity,
                                    ValueKind::Tool { .. } => callback_arity == 1,
                                    ValueKind::Builtin(name) => {
                                        let (min, max) = match name.as_str() {
                                            "range" => (1, 2),
                                            "slice" | "substring" => (2, 3),
                                            _ => {
                                                let n = if let Type::Builtin(b) =
                                                    args[1].display_type()
                                                {
                                                    b.signature.parameters.len()
                                                } else {
                                                    0
                                                };
                                                (n, n)
                                            }
                                        };
                                        (min..=max).contains(&callback_arity)
                                    }
                                    _ => false,
                                };
                                if !accepts {
                                    return Err(runtime(
                                        format!(
                                            "{name} callback must accept {callback_arity} arguments"
                                        ),
                                        span,
                                    ));
                                }
                                let iteration = Iteration {
                                    name: name.clone(),
                                    xs: args[0].clone(),
                                    callback: args[1].clone(),
                                    index: 0,
                                    out: Vec::new(),
                                    keys: Vec::new(),
                                    groups: Vec::new(),
                                    acc: args.get(2).cloned().unwrap_or_else(Value::null),
                                    ctx,
                                    span,
                                };
                                actions.push(Action::Next(Box::new(iteration)));
                            } else {
                                values.push(builtins::call(name, args, &mut self.budget, span)?);
                            }
                        }
                        ValueKind::Tool { server, name } => {
                            if args.len() != 1 {
                                return Err(runtime(
                                    "tool expects exactly one object argument",
                                    span,
                                ));
                            }
                            args[0].as_object(span)?;
                            let json = args[0].to_json(&mut self.budget, span, false)?;
                            // Validate before counting requests actually sent.
                            self.registry
                                .validate_arguments(server, name, &json)
                                .map_err(|e| runtime(e.to_string(), span))?;
                            if self.budget.calls >= 50 {
                                return Err(limit("downstream call limit (50)", span));
                            }
                            self.budget.charge(10, span)?;
                            self.budget.calls += 1;
                            let result = self
                                .registry
                                .call_tool(server, name, json)
                                .await
                                .map_err(|e| tool_error(server, name, e, span))?;
                            let json = if self.registry.has_output_schema(server, name) {
                                result.structured_content.ok_or_else(|| {
                                    runtime("downstream returned no normalized value", span)
                                })?
                            } else {
                                serde_json::to_value(result.content)
                                    .map_err(|e| runtime(e.to_string(), span))?
                            };
                            values.push(Value::from_json(&json, &mut self.budget, span)?);
                        }
                        _ => return Err(runtime("value is not callable", span)),
                    },
                    Action::Next(it) => {
                        let xs = it.xs.as_array(it.span)?;
                        if it.index == xs.len() {
                            let result = match it.name.as_str() {
                                "reduce" => it.acc,
                                "find" => Value::null(),
                                "any" => Value::boolean(false),
                                "all" => Value::boolean(true),
                                "group_by" => {
                                    let mut out = Vec::new();
                                    for (k, group) in it.groups {
                                        out.push((
                                            k,
                                            Value::array(group, &mut self.budget, it.span)?,
                                        ));
                                    }
                                    Value::object(out, &mut self.budget, it.span)?
                                }
                                "sort_by" => {
                                    let n = xs.len();
                                    let log = if n <= 1 {
                                        0
                                    } else {
                                        usize::BITS as usize - (n - 1).leading_zeros() as usize
                                    };
                                    self.budget.charge(n.saturating_mul(log), it.span)?;
                                    let mut pairs: Vec<_> =
                                        it.keys.into_iter().zip(xs.iter().cloned()).collect();
                                    pairs.sort_by(|(a, _), (b, _)| match (a.kind(), b.kind()) {
                                        (ValueKind::Number(a), ValueKind::Number(b)) => {
                                            a.total_cmp(b)
                                        }
                                        (ValueKind::String(a), ValueKind::String(b)) => a.cmp(b),
                                        _ => std::cmp::Ordering::Equal,
                                    });
                                    Value::array(
                                        pairs.into_iter().map(|(_, v)| v).collect(),
                                        &mut self.budget,
                                        it.span,
                                    )?
                                }
                                _ => Value::array(it.out, &mut self.budget, it.span)?,
                            };
                            values.push(result);
                        } else {
                            self.budget.charge(1, it.span)?;
                            let args = if it.name == "reduce" {
                                vec![it.acc.clone(), xs[it.index].clone()]
                            } else {
                                vec![xs[it.index].clone()]
                            };
                            let callback = it.callback.clone();
                            let ctx = it.ctx.clone();
                            let span = it.span;
                            actions.push(Action::Iterated(it));
                            actions.push(Action::Invoke(callback, args, ctx, span));
                        }
                    }
                    Action::Iterated(mut it) => {
                        let result = values.pop().unwrap();
                        let input = it.xs.as_array(it.span)?[it.index].clone();
                        match it.name.as_str() {
                            "map" => it.out.push(result),
                            "reduce" => it.acc = result,
                            "filter" => {
                                if result.as_bool(it.span)? {
                                    it.out.push(input)
                                }
                            }
                            "flat_map" => {
                                let xs = result.as_array(it.span)?;
                                if it.out.len() + xs.len() > MAX_ITEMS {
                                    return Err(limit(
                                        "collection element limit (100,000)",
                                        it.span,
                                    ));
                                }
                                self.budget.charge(xs.len(), it.span)?;
                                it.out.extend_from_slice(xs);
                            }
                            "find" | "any" | "all" => {
                                let b = result.as_bool(it.span)?;
                                if (it.name == "all" && !b) || (it.name != "all" && b) {
                                    values.push(if it.name == "find" {
                                        input
                                    } else {
                                        Value::boolean(b)
                                    });
                                    continue;
                                }
                            }
                            "sort_by" => {
                                if !matches!(
                                    result.kind(),
                                    ValueKind::Number(_) | ValueKind::String(_)
                                ) {
                                    return Err(runtime(
                                        "sort_by keys must be numbers or strings",
                                        it.span,
                                    ));
                                }
                                if let Some(first) = it.keys.first()
                                    && std::mem::discriminant(first.kind())
                                        != std::mem::discriminant(result.kind())
                                {
                                    return Err(runtime(
                                        "sort_by keys must have one primitive kind",
                                        it.span,
                                    ));
                                }
                                it.keys.push(result);
                            }
                            "group_by" => {
                                let key = result.as_str(it.span)?;
                                self.budget
                                    .charge(key.chars().count() + it.groups.len(), it.span)?;
                                if let Some((_, group)) =
                                    it.groups.iter_mut().find(|(k, _)| k == key)
                                {
                                    group.push(input);
                                } else {
                                    it.groups.push((key.into(), vec![input]));
                                }
                            }
                            _ => unreachable!(),
                        }
                        it.index += 1;
                        actions.push(Action::Next(it));
                    }
                }
            }
            let result = values
                .pop()
                .ok_or_else(|| runtime("evaluation produced no value", expression.span))?;
            self.store
                .reserve(std::slice::from_ref(&result), 0, expression.span)?;
            Ok(result)
        }
        .await;
        result.map_err(|mut error| {
            if foreign {
                error.span = expression.span;
                error.message = format!("{} (in stored closure)", error.message);
            }
            error
        })
    }
    fn member(
        &mut self,
        receiver: Value,
        key: &str,
        optional: bool,
        ty: Option<&Type>,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        self.budget.charge(1, span)?;
        match receiver.kind() {
            ValueKind::Object(xs) => {
                self.budget.charge(xs.len() + key.chars().count(), span)?;
                xs.iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .or_else(|| optional.then(Value::null))
                    .ok_or_else(|| runtime(format!("property '{key}' does not exist"), span))
            }
            ValueKind::Server(server) => {
                let signature = ty
                    .cloned()
                    .or_else(|| {
                        if let Type::Server(tools) = receiver.display_type() {
                            tools
                                .iter()
                                .find(|(k, _)| k == key)
                                .map(|(_, ty)| ty.clone())
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| runtime(format!("unknown tool '{key}'"), span))?;
                Value::new(
                    ValueKind::Tool {
                        server: server.clone(),
                        name: key.into(),
                    },
                    signature,
                    &mut self.budget,
                    span,
                )
            }
            _ => Err(runtime(
                "property access requires an object or server",
                span,
            )),
        }
    }
    fn binary(
        &mut self,
        op: BinaryOp,
        a: Value,
        b: Value,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        use BinaryOp::*;
        match op {
            And | Or => Ok(Value::boolean(b.as_bool(span)?)),
            Equal | NotEqual => {
                let same = super::value::equal(&a, &b, &mut self.budget, span)?;
                Ok(Value::boolean(if op == Equal { same } else { !same }))
            }
            Add if matches!(a.kind(), ValueKind::String(_)) => {
                let a = a.as_str(span)?;
                let b = b.as_str(span)?;
                self.budget.string(a.len().saturating_add(b.len()), span)?;
                self.budget
                    .charge(a.chars().count() + b.chars().count(), span)?;
                Value::string(format!("{a}{b}"), &mut self.budget, span)
            }
            Less | LessEqual | Greater | GreaterEqual => {
                let ordering = match (a.kind(), b.kind()) {
                    (ValueKind::Number(a), ValueKind::Number(b)) => a.partial_cmp(b).unwrap(),
                    (ValueKind::String(a), ValueKind::String(b)) => {
                        self.budget
                            .charge(a.chars().count() + b.chars().count(), span)?;
                        a.cmp(b)
                    }
                    _ => {
                        return Err(runtime(
                            "comparison requires two numbers or two strings",
                            span,
                        ));
                    }
                };
                Ok(Value::boolean(match op {
                    Less => ordering.is_lt(),
                    LessEqual => ordering.is_le(),
                    Greater => ordering.is_gt(),
                    _ => ordering.is_ge(),
                }))
            }
            _ => {
                let a = a.as_number(span)?;
                let b = b.as_number(span)?;
                if op == Divide && b == 0.0 {
                    return Err(runtime("division by zero", span));
                }
                Value::number(
                    match op {
                        Add => a + b,
                        Subtract => a - b,
                        Multiply => a * b,
                        Divide => a / b,
                        _ => unreachable!(),
                    },
                    span,
                )
            }
        }
    }
}
fn optional_field(
    ty: &Type,
    key: &str,
    definitions: &BTreeMap<String, Type>,
    depth: usize,
) -> bool {
    if depth > 64 {
        return false;
    }
    match ty {
        Type::Record(r) => r.fields.iter().any(|f| f.name == key && f.optional),
        Type::Union(ts) => ts
            .iter()
            .any(|t| optional_field(t, key, definitions, depth + 1)),
        Type::Named(name) => definitions
            .get(name)
            .is_some_and(|t| optional_field(t, key, definitions, depth + 1)),
        _ => false,
    }
}
impl Action {
    fn roots(&self, out: &mut Vec<Value>) {
        fn context(ctx: &Context, out: &mut Vec<Value>) {
            out.extend(ctx.locals.values().cloned());
            out.extend(ctx.globals.values().cloned());
        }
        match self {
            Self::Eval(_, ctx)
            | Self::BinaryLeft(_, _, ctx, _)
            | Self::Conditional(_, _, ctx, _)
            | Self::Call(_, ctx, _)
            | Self::Sequence(_, _, _, ctx)
            | Self::Local(_, _, _, ctx) => context(ctx, out),
            Self::BinaryRight(_, v, _) => out.push(v.clone()),
            Self::Invoke(v, args, ctx, _) => {
                out.push(v.clone());
                out.extend(args.iter().cloned());
                context(ctx, out);
            }
            Self::Next(it) | Self::Iterated(it) => {
                out.push(it.xs.clone());
                out.push(it.callback.clone());
                out.push(it.acc.clone());
                out.extend(it.out.iter().chain(it.keys.iter()).cloned());
                for (_, group) in &it.groups {
                    out.extend(group.iter().cloned());
                }
                context(&it.ctx, out);
            }
            _ => (),
        }
    }
}

fn tool_error(
    server: &str,
    name: &str,
    error: crate::registry::CallError,
    span: Span,
) -> Diagnostic {
    let detail = if let crate::registry::CallError::ToolReported(result) = &error {
        let mut message = String::new();
        for block in &result.content {
            if let Some(text) = block.as_text() {
                if !message.is_empty() {
                    message.push('\n');
                }
                for c in text.text.chars().take(2048 - message.chars().count()) {
                    message.push(c);
                }
                if message.chars().count() >= 2048 {
                    break;
                }
            }
        }
        if message.is_empty() {
            error.to_string()
        } else {
            message
        }
    } else {
        error.to_string()
    };
    runtime(format!("{server}.{name}: {detail}"), span)
        .with_hint("inspect the arguments and retry the call as a separate program")
}

fn matches_type(
    value: &Value,
    ty: &Type,
    defs: &BTreeMap<String, Type>,
    budget: &mut Budget,
    span: Span,
    depth: usize,
) -> Result<bool, Diagnostic> {
    budget.charge(1, span)?;
    if depth > 64 {
        return Err(limit("runtime type validation depth limit", span));
    }
    Ok(match ty {
        Type::Any => true,
        Type::String => matches!(value.kind(), ValueKind::String(_)),
        Type::Boolean => matches!(value.kind(), ValueKind::Bool(_)),
        Type::Null => matches!(value.kind(), ValueKind::Null),
        Type::Number { integer } => {
            matches!(value.kind(),ValueKind::Number(n) if !integer||n.fract()==0.0)
        }
        Type::Literal(literal) => match (value.kind(), literal) {
            (ValueKind::Number(n), serde_json::Value::Number(l)) => l.as_f64() == Some(*n),
            (ValueKind::String(s), serde_json::Value::String(l)) => {
                budget.charge(s.len() + l.len(), span)?;
                s == l
            }
            (ValueKind::Bool(b), serde_json::Value::Bool(l)) => b == l,
            (ValueKind::Null, serde_json::Value::Null) => true,
            _ => false,
        },
        Type::Named(name) => {
            if let Some(ty) = defs.get(name) {
                matches_type(value, ty, defs, budget, span, depth + 1)?
            } else {
                false
            }
        }
        Type::Union(types) => {
            let mut valid = false;
            for ty in types {
                if matches_type(value, ty, defs, budget, span, depth + 1)? {
                    valid = true;
                    break;
                }
            }
            valid
        }
        Type::Array(item) => {
            if let ValueKind::Array(xs) = value.kind() {
                let mut valid = true;
                for x in xs {
                    if !matches_type(x, item, defs, budget, span, depth + 1)? {
                        valid = false;
                        break;
                    }
                }
                valid
            } else {
                false
            }
        }
        Type::Tuple(tuple) => {
            if let ValueKind::Array(xs) = value.kind() {
                let mut valid = true;
                for (i, item) in tuple.items.iter().enumerate() {
                    if let Some(v) = xs.get(i) {
                        valid &= matches_type(v, &item.ty, defs, budget, span, depth + 1)?;
                    } else if !item.optional {
                        valid = false;
                    }
                }
                if xs.len() > tuple.items.len() {
                    if let Some(rest) = &tuple.rest {
                        for v in &xs[tuple.items.len()..] {
                            valid &= matches_type(v, rest, defs, budget, span, depth + 1)?;
                        }
                    } else {
                        valid = false;
                    }
                }
                valid
            } else {
                false
            }
        }
        Type::Record(record) => {
            if let ValueKind::Object(xs) = value.kind() {
                let mut valid = true;
                for field in &record.fields {
                    budget.charge(xs.len() + field.name.len(), span)?;
                    if let Some((_, v)) = xs.iter().find(|(k, _)| k == &field.name) {
                        valid &= matches_type(v, &field.ty, defs, budget, span, depth + 1)?;
                    } else if !field.optional {
                        valid = false;
                    }
                }
                if let Some(additional) = &record.additional {
                    for (k, v) in xs {
                        budget.charge(record.fields.len() + k.len(), span)?;
                        if !record.fields.iter().any(|f| &f.name == k) {
                            valid &= matches_type(v, additional, defs, budget, span, depth + 1)?;
                        }
                    }
                }
                valid
            } else {
                false
            }
        }
        Type::Function(_) | Type::Builtin(_) | Type::Tool(_) => {
            matches!(
                value.kind(),
                ValueKind::Closure(_) | ValueKind::Builtin(_) | ValueKind::Tool { .. }
            ) && crate::frontend::relations::assignable(value.display_type(), ty, defs)
        }
        Type::Server(_) => {
            matches!(value.kind(), ValueKind::Server(_))
                && crate::frontend::relations::assignable(value.display_type(), ty, defs)
        }
    })
}
