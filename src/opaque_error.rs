use std::{error::Error, fmt};
use tracing_error::SpanTrace;

#[derive(Debug)]
pub struct OpaqueError {
    span_trace: SpanTrace,
    pub context: String,
}

impl fmt::Display for OpaqueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // optional: only print span trace if requested "{:#}"
        if f.alternate() {
            let mut vec = Vec::new();

            self.span_trace.with_spans(|metadata, _fields| {
                vec.push(metadata.name());
                true
            });
            if vec.len() > 1 {
                write!(f, "trace = {}", vec.pop().unwrap())?;
            }
            vec.reverse();
            for s in vec {
                write!(f, "::{}", s)?;
            }
        };
        write!(f, " {}", self.context)?;
        Ok(())
    }
}
impl Error for OpaqueError {}
impl OpaqueError {
    pub fn create(str: &str) -> OpaqueError {
        Self {
            span_trace: SpanTrace::capture(),
            context: str.to_string(),
        }
    }
    pub fn get_span_trace(&self) -> String {
        let mut vec = Vec::new();

        self.span_trace.with_spans(|metadata, _fields| {
            vec.push(metadata.name());
            true
        });
        vec.iter().rfold(String::new(), |mut acc, x| {
            if acc.len() != 0 {
                acc.push_str("::");
            }
            acc.push_str(x);
            acc
        })
    }
}

impl From<String> for OpaqueError {
    fn from(value: String) -> Self {
        Self {
            span_trace: SpanTrace::capture(),
            context: value,
        }
    }
}

impl From<&str> for OpaqueError {
    fn from(value: &str) -> Self {
        Self {
            span_trace: SpanTrace::capture(),
            context: value.to_string(),
        }
    }
}
