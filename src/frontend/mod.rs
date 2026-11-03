//! Pure, preflight-only MCPL front end. No tools are invoked by parsing or checking.
pub mod ast;
pub mod builtins;
pub mod diagnostic;
pub mod environment;
pub mod parser;
pub mod relations;

pub use ast::{Expr, Program, Span};
pub use diagnostic::{Category, Diagnostic, render_diagnostics};
pub use environment::{Binding, Environment};
pub use parser::{parse_expression, parse_program};

pub mod checker;
pub use checker::{CheckedExpression, CheckedProgram, check_expression, check_program};
