//! Gemini conversion, credential helpers, and the subscription quota SDK.
//! Use [`usage::Client`] to read provider/account limits from a running hey-proxy.
pub mod gemini;

pub mod credentials;
pub mod fallback;

/// Provider-independent subscription quota types and async HTTP client.
pub mod usage;
