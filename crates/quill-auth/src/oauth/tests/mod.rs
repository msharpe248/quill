//! The flows, end to end, against a provider that is a `wiremock` server
//! on this computer and a user that is a few lines of test.
//!
//! Nothing here reaches past the loopback address: the provider's
//! addresses all come from the test's own mock server, and the stand-in
//! browser refuses to visit anything else. No test touches the Keychain.

pub(crate) mod support;

mod browser;
mod device;
mod secrecy;
mod tokens;
