use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::Value;

use crate::types::{Field, FunctionType, RecordType, TupleItem, TupleType, Type, union};

const MAX_DEPTH: usize = 256;
const MAX_WORK: usize = 16_384;

fn dereference<'a>(mut ty: &'a Type, definitions: &'a BTreeMap<String, Type>) -> &'a Type {
    let mut seen = BTreeSet::new();
    while let Type::Named(name) = ty {
        if seen.len() >= MAX_DEPTH || !seen.insert(name) {
            return &Type::Any;
        }
        let Some(next) = definitions.get(name) else {
            return &Type::Any;
        };
        ty = next;
    }
    ty
}

pub fn resolve(ty: &Type, definitions: &BTreeMap<String, Type>) -> Type {
    match dereference(ty, definitions) {
        Type::Literal(value @ (Value::Array(_) | Value::Object(_))) => literal_shape(value),
        ty => ty.clone(),
    }
}

struct Relation<'a> {
    definitions: &'a BTreeMap<String, Type>,
    active: HashSet<(usize, usize)>,
    work: usize,
    depth: usize,
}

impl<'a> Relation<'a> {
    fn new(definitions: &'a BTreeMap<String, Type>) -> Self {
        Self {
            definitions,
            active: HashSet::new(),
            work: MAX_WORK,
            depth: 0,
        }
    }

    fn compare(&mut self, left: &Type, right: &Type, disjoint: bool) -> bool {
        if self.work == 0 || self.depth >= MAX_DEPTH {
            return false;
        }
        self.work -= 1;
        let left = dereference(left, self.definitions);
        let right = dereference(right, self.definitions);
        let key = (left as *const Type as usize, right as *const Type as usize);
        if !self.active.insert(key) {
            return !disjoint;
        }
        self.depth += 1;
        let result = if disjoint {
            self.disjoint_inner(left, right)
        } else {
            self.assign_inner(left, right)
        };
        self.depth -= 1;
        self.active.remove(&key);
        result
    }

    fn assign(&mut self, actual: &Type, expected: &Type) -> bool {
        self.compare(actual, expected, false)
    }

    fn assign_inner(&mut self, actual: &Type, expected: &Type) -> bool {
        match (actual, expected) {
            (Type::Any, _) | (_, Type::Any) => true,
            (Type::Builtin(builtin), expected) => {
                self.assign(&Type::Function(builtin.signature.clone()), expected)
            }
            (actual, Type::Builtin(builtin)) => {
                self.assign(actual, &Type::Function(builtin.signature.clone()))
            }
            (Type::Union(types), _) => {
                !types.is_empty() && types.iter().all(|ty| self.assign(ty, expected))
            }
            (_, Type::Union(types)) => types.iter().any(|ty| self.assign(actual, ty)),
            (Type::Literal(left), Type::Literal(right)) => literal_equal(left, right),
            (Type::Literal(value), _) => self.assign(&literal_shape(value), expected),
            (Type::Null, Type::Literal(Value::Null)) => true,
            (actual, Type::Literal(value @ (Value::Array(_) | Value::Object(_)))) => {
                self.assign(actual, &literal_shape(value))
            }
            (Type::String, Type::String)
            | (Type::Number { .. }, Type::Number { .. })
            | (Type::Boolean, Type::Boolean)
            | (Type::Null, Type::Null) => true,
            (Type::Array(left), Type::Array(right)) => self.assign(left, right),
            (Type::Tuple(left), Type::Array(right)) => {
                left.items.iter().all(|item| self.assign(&item.ty, right))
                    && left
                        .rest
                        .as_ref()
                        .is_none_or(|tail| self.assign(tail, right))
            }
            (Type::Array(left), Type::Tuple(right)) => {
                minimum_length(right) == 0
                    && right
                        .rest
                        .as_ref()
                        .is_some_and(|tail| self.assign(left, tail))
                    && right.items.iter().all(|item| self.assign(left, &item.ty))
            }
            (Type::Tuple(left), Type::Tuple(right)) => self.assign_tuple(left, right),
            (Type::Record(left), Type::Record(right)) => self.assign_record(left, right),
            (Type::Function(left), Type::Function(right)) => self.assign_function(left, right),
            (Type::Tool(left), Type::Function(right)) => {
                self.assign_function(&left.signature, right)
            }
            (Type::Function(left), Type::Tool(right)) => {
                self.assign_function(left, &right.signature)
            }
            (Type::Tool(left), Type::Tool(right)) => {
                self.assign_function(&left.signature, &right.signature)
            }
            (Type::Server(left), Type::Server(right)) => right.iter().all(|(name, ty)| {
                left.iter()
                    .find(|(key, _)| key == name)
                    .is_some_and(|(_, value)| self.assign(value, ty))
            }),
            _ => false,
        }
    }

    fn assign_function(&mut self, actual: &FunctionType, expected: &FunctionType) -> bool {
        actual.parameters.len() == expected.parameters.len()
            && actual
                .parameters
                .iter()
                .zip(&expected.parameters)
                .all(|(a, e)| self.assign(&e.ty, &a.ty))
            && self.assign(&actual.result, &expected.result)
    }

    fn assign_record(&mut self, actual: &RecordType, expected: &RecordType) -> bool {
        for field in &expected.fields {
            match actual
                .fields
                .iter()
                .find(|candidate| candidate.name == field.name)
            {
                Some(candidate) => {
                    if (!field.optional && candidate.optional)
                        || !self.assign(&candidate.ty, &field.ty)
                    {
                        return false;
                    }
                }
                None => {
                    if !field.optional
                        || actual
                            .additional
                            .as_ref()
                            .is_some_and(|ty| !self.assign(ty, &field.ty))
                    {
                        return false;
                    }
                }
            }
        }
        for field in &actual.fields {
            if !expected
                .fields
                .iter()
                .any(|candidate| candidate.name == field.name)
                && !expected
                    .additional
                    .as_ref()
                    .is_some_and(|ty| self.assign(&field.ty, ty))
            {
                return false;
            }
        }
        actual.additional.as_ref().is_none_or(|actual| {
            expected
                .additional
                .as_ref()
                .is_some_and(|expected| self.assign(actual, expected))
        })
    }

    fn assign_tuple(&mut self, actual: &TupleType, expected: &TupleType) -> bool {
        if minimum_length(actual) < minimum_length(expected)
            || (expected.rest.is_none()
                && (actual.rest.is_some() || actual.items.len() > expected.items.len()))
        {
            return false;
        }
        for index in 0..actual.items.len().max(expected.items.len()) {
            if let Some(actual) = tuple_slot(actual, index)
                && !tuple_slot(expected, index)
                    .is_some_and(|expected| self.assign(actual, expected))
            {
                return false;
            }
        }
        actual.rest.as_ref().is_none_or(|actual| {
            expected
                .rest
                .as_ref()
                .is_some_and(|expected| self.assign(actual, expected))
        })
    }

    fn apart(&mut self, left: &Type, right: &Type) -> bool {
        self.compare(left, right, true)
    }

    fn disjoint_inner(&mut self, left: &Type, right: &Type) -> bool {
        match (left, right) {
            (Type::Any, _) | (_, Type::Any) => false,
            (Type::Union(types), _) => {
                !types.is_empty() && types.iter().all(|ty| self.apart(ty, right))
            }
            (_, Type::Union(types)) => {
                !types.is_empty() && types.iter().all(|ty| self.apart(left, ty))
            }
            (Type::Literal(left), Type::Literal(right)) => !literal_equal(left, right),
            (Type::Literal(value), _) => self.apart(&literal_shape(value), right),
            (_, Type::Literal(value)) => self.apart(left, &literal_shape(value)),
            (Type::Record(left), Type::Record(right)) => {
                self.records_apart(left, right) || self.records_apart(right, left)
            }
            (Type::Array(_), Type::Array(_)) => false, // Empty arrays overlap regardless of element type.
            (Type::Tuple(left), Type::Tuple(right)) => {
                let required = minimum_length(left).max(minimum_length(right));
                (left.rest.is_none() && left.items.len() < required)
                    || (right.rest.is_none() && right.items.len() < required)
                    || (0..required).any(|index| {
                        match (tuple_slot(left, index), tuple_slot(right, index)) {
                            (Some(left), Some(right)) => self.apart(left, right),
                            _ => true,
                        }
                    })
            }
            (Type::Array(element), Type::Tuple(tuple))
            | (Type::Tuple(tuple), Type::Array(element)) => tuple
                .items
                .iter()
                .take(minimum_length(tuple))
                .any(|slot| self.apart(element, &slot.ty)),
            (Type::String, Type::String)
            | (Type::Number { .. }, Type::Number { .. })
            | (Type::Boolean, Type::Boolean)
            | (Type::Null, Type::Null) => false,
            (Type::Function(_) | Type::Tool(_) | Type::Builtin(_) | Type::Server(_), _)
            | (_, Type::Function(_) | Type::Tool(_) | Type::Builtin(_) | Type::Server(_)) => false,
            _ => true,
        }
    }

    fn records_apart(&mut self, left: &RecordType, right: &RecordType) -> bool {
        left.fields
            .iter()
            .filter(|field| !field.optional)
            .any(|field| {
                let other = right
                    .fields
                    .iter()
                    .find(|other| other.name == field.name)
                    .map(|other| &other.ty)
                    .or(right.additional.as_deref());
                other.is_none_or(|other| self.apart(&field.ty, other))
            })
    }
}

pub fn assignable(actual: &Type, expected: &Type, definitions: &BTreeMap<String, Type>) -> bool {
    Relation::new(definitions).assign(actual, expected)
}

pub fn disjoint(left: &Type, right: &Type, definitions: &BTreeMap<String, Type>) -> bool {
    Relation::new(definitions).apart(left, right)
}

fn minimum_length(tuple: &TupleType) -> usize {
    tuple
        .items
        .iter()
        .rposition(|item| !item.optional)
        .map_or(0, |index| index + 1)
}

fn tuple_slot(tuple: &TupleType, index: usize) -> Option<&Type> {
    tuple
        .items
        .get(index)
        .map(|item| &item.ty)
        .or(tuple.rest.as_deref())
}

fn literal_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| literal_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| literal_equal(value, other))
                })
        }
        _ => left == right,
    }
}

fn literal_shape(value: &Value) -> Type {
    match value {
        Value::Null => Type::Null,
        Value::Bool(_) => Type::Boolean,
        Value::Number(_) => Type::number(),
        Value::String(_) => Type::String,
        Value::Array(items) => Type::Tuple(TupleType {
            items: items
                .iter()
                .map(|value| TupleItem {
                    ty: Type::Literal(value.clone()),
                    optional: false,
                })
                .collect(),
            rest: None,
        }),
        Value::Object(fields) => Type::Record(RecordType {
            fields: fields
                .iter()
                .map(|(name, value)| Field {
                    name: name.clone(),
                    ty: Type::Literal(value.clone()),
                    optional: false,
                })
                .collect(),
            additional: None,
        }),
    }
}

pub fn widen(ty: &Type) -> Type {
    match ty {
        Type::Literal(value) => widen(&literal_shape(value)),
        Type::Array(item) => Type::Array(Box::new(widen(item))),
        Type::Tuple(tuple) => Type::Tuple(TupleType {
            items: tuple
                .items
                .iter()
                .map(|item| TupleItem {
                    ty: widen(&item.ty),
                    optional: item.optional,
                })
                .collect(),
            rest: tuple.rest.as_ref().map(|ty| Box::new(widen(ty))),
        }),
        Type::Record(record) => Type::Record(RecordType {
            fields: record
                .fields
                .iter()
                .map(|field| Field {
                    name: field.name.clone(),
                    ty: widen(&field.ty),
                    optional: field.optional,
                })
                .collect(),
            additional: record.additional.as_ref().map(|ty| Box::new(widen(ty))),
        }),
        Type::Union(types) => union(types.iter().map(widen)),
        _ => ty.clone(),
    }
}

pub fn equatable(ty: &Type, definitions: &BTreeMap<String, Type>) -> bool {
    fn visit(
        ty: &Type,
        definitions: &BTreeMap<String, Type>,
        active: &mut HashSet<usize>,
        work: &mut usize,
        depth: usize,
    ) -> bool {
        if *work == 0 || depth >= MAX_DEPTH {
            return false;
        }
        *work -= 1;
        let ty = dereference(ty, definitions);
        let key = ty as *const Type as usize;
        if !active.insert(key) {
            return true;
        }
        let mut next = |ty: &Type| visit(ty, definitions, active, work, depth + 1);
        let result = match ty {
            Type::Function(_) | Type::Tool(_) | Type::Builtin(_) | Type::Server(_) => false,
            Type::Array(item) => next(item),
            Type::Tuple(tuple) => {
                tuple.items.iter().all(|item| next(&item.ty))
                    && tuple.rest.as_ref().is_none_or(|ty| next(ty))
            }
            Type::Record(record) => {
                record.fields.iter().all(|field| next(&field.ty))
                    && record.additional.as_ref().is_none_or(|ty| next(ty))
            }
            Type::Union(types) => types.iter().all(next),
            _ => true,
        };
        active.remove(&key);
        result
    }
    let mut work = MAX_WORK;
    visit(ty, definitions, &mut HashSet::new(), &mut work, 0)
}

fn lookup(
    ty: &Type,
    key: Option<&str>,
    definitions: &BTreeMap<String, Type>,
    work: &mut usize,
    depth: usize,
) -> Option<Type> {
    if *work == 0 || depth >= MAX_DEPTH {
        return None;
    }
    *work -= 1;
    match dereference(ty, definitions) {
        Type::Literal(value @ (Value::Array(_) | Value::Object(_))) => {
            lookup(&literal_shape(value), key, definitions, work, depth + 1)
        }
        Type::Any => Some(Type::Any),
        Type::Union(types) if !types.is_empty() => types
            .iter()
            .map(|ty| lookup(ty, key, definitions, work, depth + 1))
            .collect::<Option<Vec<_>>>()
            .map(union),
        Type::Array(element) if key.is_none() => Some((**element).clone()),
        Type::Tuple(tuple) if key.is_none() => Some(union(
            tuple
                .items
                .iter()
                .map(|item| item.ty.clone())
                .chain(tuple.rest.iter().map(|ty| (**ty).clone())),
        )),
        Type::Record(record) => {
            let key = key?;
            record
                .fields
                .iter()
                .find(|field| field.name == key)
                .map(|field| {
                    if field.optional {
                        union([field.ty.clone(), Type::Null])
                    } else {
                        field.ty.clone()
                    }
                })
                .or_else(|| record.additional.as_deref().cloned())
        }
        Type::Server(tools) => tools
            .iter()
            .find(|(name, _)| Some(name.as_str()) == key)
            .map(|(_, ty)| ty.clone()),
        _ => None,
    }
}

pub fn element_type(ty: &Type, definitions: &BTreeMap<String, Type>) -> Option<Type> {
    let mut work = MAX_WORK;
    lookup(ty, None, definitions, &mut work, 0)
}

pub fn property_type(ty: &Type, key: &str, definitions: &BTreeMap<String, Type>) -> Option<Type> {
    let mut work = MAX_WORK;
    lookup(ty, Some(key), definitions, &mut work, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Parameter, ToolType};
    use serde_json::json;

    fn field(name: &str, ty: Type, optional: bool) -> Field {
        Field {
            name: name.into(),
            ty,
            optional,
        }
    }

    fn record(fields: Vec<Field>, additional: Option<Type>) -> Type {
        Type::Record(RecordType {
            fields,
            additional: additional.map(Box::new),
        })
    }

    fn tuple(items: Vec<(Type, bool)>, rest: Option<Type>) -> Type {
        Type::Tuple(TupleType {
            items: items
                .into_iter()
                .map(|(ty, optional)| TupleItem { ty, optional })
                .collect(),
            rest: rest.map(Box::new),
        })
    }

    fn function(parameter: Type, result: Type) -> Type {
        Type::Function(FunctionType {
            parameters: vec![Parameter {
                name: "x".into(),
                ty: parameter,
            }],
            result: Box::new(result),
        })
    }

    #[test]
    fn gradual_numeric_and_union_assignment() {
        let definitions = BTreeMap::new();
        assert!(assignable(&Type::Any, &Type::String, &definitions));
        assert!(assignable(&Type::String, &Type::Any, &definitions));
        assert!(assignable(
            &Type::number(),
            &Type::Number { integer: true },
            &definitions
        ));
        assert!(assignable(
            &Type::Literal(json!(1.5)),
            &Type::Number { integer: true },
            &definitions
        ));
        assert!(assignable(
            &Type::Literal(json!(1)),
            &Type::Literal(json!(1.0)),
            &definitions
        ));
        assert!(!assignable(
            &Type::number(),
            &Type::Literal(json!(1)),
            &definitions
        ));
        let either = union([Type::String, Type::number()]);
        assert!(assignable(&Type::String, &either, &definitions));
        assert!(assignable(&either, &either, &definitions));
        assert!(!assignable(&either, &Type::String, &definitions));
    }

    #[test]
    fn closed_records_and_required_fields() {
        let definitions = BTreeMap::new();
        let required = record(vec![field("x", Type::String, false)], None);
        let optional = record(vec![field("x", Type::String, true)], None);
        let empty = record(vec![], None);
        let extra = record(
            vec![
                field("x", Type::String, false),
                field("y", Type::number(), false),
            ],
            None,
        );
        assert!(assignable(&required, &optional, &definitions));
        assert!(!assignable(&optional, &required, &definitions));
        assert!(assignable(&empty, &optional, &definitions));
        assert!(!assignable(&empty, &required, &definitions));
        assert!(!assignable(&extra, &required, &definitions));
        assert!(!assignable(
            &record(vec![], Some(Type::String)),
            &required,
            &definitions
        ));
    }

    #[test]
    fn signatures_only_cover_unnamed_fields() {
        let definitions = BTreeMap::new();
        let target = record(
            vec![field("name", Type::String, false)],
            Some(Type::number()),
        );
        let valid = record(
            vec![
                field("name", Type::String, false),
                field("count", Type::number(), false),
            ],
            None,
        );
        let invalid = record(
            vec![
                field("name", Type::String, false),
                field("count", Type::Boolean, false),
            ],
            None,
        );
        assert!(assignable(&valid, &target, &definitions));
        assert!(!assignable(&invalid, &target, &definitions));
        assert_eq!(
            property_type(&target, "name", &definitions),
            Some(Type::String)
        );
        assert_eq!(
            property_type(&target, "count", &definitions),
            Some(Type::number())
        );
        let optional = record(vec![field("x", Type::String, true)], Some(Type::number()));
        assert!(!assignable(
            &record(vec![], Some(Type::number())),
            &optional,
            &definitions
        ));
    }

    #[test]
    fn optional_reads_and_union_access() {
        let definitions = BTreeMap::new();
        let optional = record(vec![field("x", Type::String, true)], None);
        assert_eq!(
            property_type(&optional, "x", &definitions),
            Some(union([Type::String, Type::Null]))
        );
        assert_eq!(property_type(&optional, "absent", &definitions), None);
        assert_eq!(
            property_type(&Type::Any, "anything", &definitions),
            Some(Type::Any)
        );
        let alternatives = union([
            optional.clone(),
            record(vec![field("x", Type::number(), false)], None),
        ]);
        assert_eq!(
            property_type(&alternatives, "x", &definitions),
            Some(union([Type::String, Type::Null, Type::number()]))
        );
        assert_eq!(
            property_type(&union([optional, Type::Null]), "x", &definitions),
            None
        );
    }

    #[test]
    fn tuple_assignment_respects_presence_and_rest() {
        let definitions = BTreeMap::new();
        let required = tuple(vec![(Type::String, false)], None);
        let optional = tuple(vec![(Type::String, true)], None);
        let rest = tuple(vec![(Type::String, true)], Some(Type::String));
        let array = Type::Array(Box::new(Type::String));
        assert!(assignable(&required, &optional, &definitions));
        assert!(!assignable(&optional, &required, &definitions));
        assert!(assignable(&required, &array, &definitions));
        assert!(assignable(&array, &rest, &definitions));
        assert!(!assignable(&array, &required, &definitions));
        assert!(!assignable(&rest, &optional, &definitions));
        assert!(assignable(&tuple(vec![], None), &array, &definitions));
        assert!(!assignable(
            &tuple(vec![(Type::String, false), (Type::number(), true)], None),
            &rest,
            &definitions
        ));
        assert_eq!(
            element_type(
                &tuple(vec![(Type::String, true)], Some(Type::number())),
                &definitions
            ),
            Some(union([Type::String, Type::number()]))
        );
        assert_eq!(
            element_type(&union([array, Type::Null]), &definitions),
            None
        );
    }

    #[test]
    fn functions_are_contravariant_with_exact_arity() {
        let definitions = BTreeMap::new();
        let broad = function(
            union([Type::String, Type::number()]),
            Type::Literal(json!(1)),
        );
        let narrow = function(Type::String, Type::number());
        assert!(assignable(&broad, &narrow, &definitions));
        assert!(!assignable(&narrow, &broad, &definitions));
        let zero = Type::Function(FunctionType {
            parameters: vec![],
            result: Box::new(Type::number()),
        });
        assert!(!assignable(&zero, &narrow, &definitions));
        let Type::Function(signature) = broad else {
            unreachable!()
        };
        let tool = Type::Tool(ToolType {
            server: "s".into(),
            name: "t".into(),
            signature,
        });
        assert!(assignable(&tool, &narrow, &definitions));
        let server = Type::Server(vec![("t".into(), tool.clone())]);
        assert_eq!(property_type(&server, "t", &definitions), Some(tool));
        assert!(!equatable(&server, &definitions));
    }

    #[test]
    fn equality_uses_numeric_not_json_representation_identity() {
        let definitions = BTreeMap::new();
        assert!(!disjoint(
            &Type::Literal(json!(1)),
            &Type::Literal(json!(1.0)),
            &definitions
        ));
        assert!(!disjoint(
            &Type::Literal(json!(-0.0)),
            &Type::Literal(json!(0)),
            &definitions
        ));
        assert!(!disjoint(
            &Type::Literal(json!({"a": [1, 2]})),
            &Type::Literal(json!({"a": [1.0, 2.0]})),
            &definitions
        ));
        assert!(disjoint(
            &Type::Literal(json!(1)),
            &Type::Literal(json!(2)),
            &definitions
        ));
        assert!(disjoint(&Type::String, &Type::number(), &definitions));
        assert!(!disjoint(&Type::Any, &Type::Null, &definitions));
        assert!(!disjoint(
            &union([Type::String, Type::number()]),
            &Type::number(),
            &definitions
        ));
    }

    #[test]
    fn equality_accounts_for_optional_fields_and_empty_arrays() {
        let definitions = BTreeMap::new();
        let a = record(vec![field("kind", Type::Literal(json!("a")), false)], None);
        let b = record(vec![field("kind", Type::Literal(json!("b")), false)], None);
        assert!(disjoint(&a, &b, &definitions));
        assert!(disjoint(&a, &record(vec![], None), &definitions));
        let optional_a = record(vec![field("x", Type::String, true)], None);
        let optional_b = record(vec![field("x", Type::number(), true)], None);
        assert!(!disjoint(&optional_a, &optional_b, &definitions));
        assert!(!disjoint(
            &Type::Array(Box::new(Type::String)),
            &Type::Array(Box::new(Type::number())),
            &definitions
        ));
        assert!(disjoint(
            &tuple(vec![(Type::String, false)], None),
            &Type::Array(Box::new(Type::number())),
            &definitions
        ));
        assert!(!disjoint(
            &tuple(vec![(Type::String, true)], None),
            &Type::Array(Box::new(Type::number())),
            &definitions
        ));
        assert!(disjoint(
            &tuple(vec![], None),
            &tuple(vec![(Type::String, false)], None),
            &definitions
        ));
    }

    #[test]
    fn nested_capabilities_are_not_equatable() {
        let definitions = BTreeMap::new();
        let capability = function(Type::Any, Type::Any);
        for ty in [
            capability.clone(),
            Type::Array(Box::new(capability.clone())),
            record(vec![field("f", capability.clone(), true)], None),
            record(vec![], Some(capability.clone())),
            tuple(vec![], Some(capability.clone())),
            union([Type::Null, capability]),
        ] {
            assert!(!equatable(&ty, &definitions));
        }
        assert!(equatable(&Type::Any, &definitions));
        assert!(equatable(&record(vec![], Some(Type::Any)), &definitions));
    }

    #[test]
    fn recursive_aliases_terminate_without_hiding_differences() {
        let node = |next: &str, value| {
            record(
                vec![
                    field("next", Type::Named(next.into()), true),
                    field("value", value, false),
                ],
                None,
            )
        };
        let definitions = BTreeMap::from([
            ("A".into(), node("A", Type::String)),
            ("B".into(), node("B", Type::String)),
            ("C".into(), node("C", Type::number())),
            ("Cycle".into(), Type::Named("Cycle".into())),
            ("Bad".into(), node("Bad", function(Type::Any, Type::Any))),
        ]);
        let a = Type::Named("A".into());
        let b = Type::Named("B".into());
        let c = Type::Named("C".into());
        assert!(assignable(&a, &b, &definitions));
        assert!(!assignable(&a, &c, &definitions));
        assert!(!disjoint(&a, &b, &definitions));
        assert!(disjoint(&a, &c, &definitions));
        assert!(equatable(&a, &definitions));
        assert!(!equatable(&Type::Named("Bad".into()), &definitions));
        assert_eq!(
            resolve(&Type::Named("Cycle".into()), &definitions),
            Type::Any
        );
        assert_eq!(property_type(&a, "value", &definitions), Some(Type::String));
    }

    #[test]
    fn recursive_union_queries_are_bounded() {
        let named = Type::Named("Loop".into());
        let definitions = BTreeMap::from([("Loop".into(), union([named.clone(), Type::String]))]);
        assert_eq!(property_type(&named, "x", &definitions), None);
        assert_eq!(element_type(&named, &definitions), None);
        assert!(!disjoint(&named, &Type::String, &definitions));
    }

    #[test]
    fn widening_is_recursive_but_does_not_expand_aliases_or_signatures() {
        let source = record(
            vec![
                field("kind", Type::Literal(json!("ok")), false),
                field(
                    "items",
                    Type::Array(Box::new(Type::Literal(json!(42)))),
                    true,
                ),
            ],
            None,
        );
        assert_eq!(
            widen(&source),
            record(
                vec![
                    field("kind", Type::String, false),
                    field("items", Type::Array(Box::new(Type::number())), true)
                ],
                None
            )
        );
        assert_eq!(
            widen(&union([Type::Literal(json!(1)), Type::Literal(json!(2))])),
            Type::number()
        );
        assert_eq!(widen(&Type::Named("A".into())), Type::Named("A".into()));
        let signature = function(Type::Literal(json!("only")), Type::Literal(json!(1)));
        assert_eq!(widen(&signature), signature);
    }
}
