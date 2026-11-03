use crate::frontend::{Category, Diagnostic, Span};

pub const FUEL: u64 = 10_000_000;
pub const MAX_ITEMS: usize = 100_000;
pub const MAX_STRING: usize = 16 * 1024 * 1024;
pub const MAX_MEMORY: usize = 256 * 1024 * 1024;
pub const MAX_DEPTH: usize = 64;

#[derive(Debug)]
pub struct Budget {
    pub remaining: u64,
    pub calls: usize,
    pub allocated: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            remaining: FUEL,
            calls: 0,
            allocated: 0,
        }
    }
}
impl Budget {
    pub fn charge(&mut self, units: usize, span: Span) -> Result<(), Diagnostic> {
        if units as u64 > self.remaining {
            return Err(limit(
                "evaluation budget exhausted (10,000,000 units)",
                span,
            ));
        }
        self.remaining -= units as u64;
        Ok(())
    }
    pub fn allocation(&mut self, bytes: usize, span: Span) -> Result<(), Diagnostic> {
        if bytes > MAX_MEMORY {
            return Err(limit("session memory limit (256 MB)", span));
        }
        self.allocated = self.allocated.saturating_add(bytes);
        Ok(())
    }
    pub fn items(&mut self, count: usize, span: Span) -> Result<(), Diagnostic> {
        if count > MAX_ITEMS {
            return Err(limit("collection element limit (100,000)", span));
        }
        self.charge(count, span)
    }
    pub fn string(&mut self, bytes: usize, span: Span) -> Result<(), Diagnostic> {
        if bytes > MAX_STRING {
            return Err(limit("string length limit (16 MB)", span));
        }
        Ok(())
    }
}
pub fn runtime(message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::new(Category::Runtime, message, span)
        .with_hint("check the value and retry the expression")
}
pub fn limit(message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::new(Category::Limit, message, span)
        .with_hint("reduce the data size or split the program")
}
