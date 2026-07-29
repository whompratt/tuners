//! Typed app-facing API: the view structs and operations behind the desktop
//! app's commands. Transport-agnostic: serialization is
//! serde, and every builder is a pure function over engine state, so commands
//! and tests share the exact same surface. Wire names stay camelCase to match
//! the dashboard's existing JSON contract.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use specta::Type;

use crate::analysis::effects::Effects;

/// Command failure with enough structure for the frontend to distinguish
/// "confirm and retry with force" (Conflict) from plain errors: the typed
/// replacement for the HTTP status codes the dashboard used to branch on.
#[derive(Serialize, Type, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Serialize, Type, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    BadRequest,
    Forbidden,
    NotFound,
    Conflict,
    Internal,
}

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        ApiError {
            kind: ErrorKind::BadRequest,
            message: msg.into(),
        }
    }
    fn conflict(msg: impl Into<String>) -> Self {
        ApiError {
            kind: ErrorKind::Conflict,
            message: msg.into(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        ApiError {
            kind: ErrorKind::NotFound,
            message: msg.into(),
        }
    }
    fn internal(msg: impl ToString) -> Self {
        ApiError {
            kind: ErrorKind::Internal,
            message: msg.to_string(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Effect vector on the wire: sparse map keyed by `effects::FIELDS` keys;
/// absent fields are real absences, never zeroes.
fn effects_map(fx: &Effects) -> BTreeMap<String, f32> {
    fx.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

// ---------------------------------------------------------------- live state

mod live;
mod meta;
mod session;
mod sharing;
mod stints;
#[cfg(test)]
mod tests;
mod views;

pub use live::*;
pub use meta::*;
pub use session::*;
pub use sharing::*;
pub use stints::*;
pub use views::*;
