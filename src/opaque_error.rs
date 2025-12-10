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
        let mut vec: Vec<(&str, String)> = Vec::new();

        self.span_trace.with_spans(|metadata, _fields| {
            vec.push((metadata.name(), _fields.into()));
            true
        });
        let str = vec.iter().rfold(String::new(), |mut acc, x| {
            let first = acc.len() != 0;
            if first {
                acc.push_str(":");
            }

            acc.push_str(x.0);
            acc.push_str("{");
            acc.push_str(&x.1);
            acc.push_str("}");

            acc
        });
        match String::from_utf8(strip_ansi_escapes::strip(str.clone())) {
            Ok(x) => x,
            Err(_) => str,
        }
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

impl From<crate::packets::ParseErrorTrace> for OpaqueError {
    fn from(value: crate::packets::ParseErrorTrace) -> Self {
        Self {
            span_trace: value.context,
            context: format!("{:?}", value.inner),
        }
    }
}
