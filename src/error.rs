//! Machine-first output: every command prints exactly one JSON envelope on
//! stdout. Exit codes: 0 ok, 1 config invalid, 2 environment/usage error
//! (3 reserved: confirmation required, 4 reserved: apply failed — v1).

use serde::Serialize;
use serde_json::{json, Value};

/// Envelope schema version, bumped only on breaking shape changes.
pub const V: u32 = 1;

#[derive(Debug, Serialize)]
pub struct Detail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Suggestion {
    pub command: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct CliError {
    pub exit: i32,
    pub code: &'static str,
    pub message: String,
    pub details: Vec<Detail>,
    pub suggestions: Vec<Suggestion>,
    pub safe_to_retry: bool,
}

impl CliError {
    pub fn env(code: &'static str, message: String) -> Self {
        CliError {
            exit: 2,
            code,
            message,
            details: Vec::new(),
            suggestions: Vec::new(),
            safe_to_retry: true,
        }
    }

    pub fn to_json(&self) -> String {
        let mut o = json!({
            "ok": false,
            "v": V,
            "code": self.code,
            "message": self.message,
            "suggestions": self.suggestions,
            "safe_to_retry": self.safe_to_retry,
        });
        if !self.details.is_empty() {
            o["details"] = json!(self.details);
        }
        serde_json::to_string_pretty(&o).expect("envelope serializes")
    }
}

pub fn ok_envelope(data: Value) -> String {
    serde_json::to_string_pretty(&json!({ "ok": true, "v": V, "data": data }))
        .expect("envelope serializes")
}
