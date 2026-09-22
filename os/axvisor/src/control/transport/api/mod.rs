//! Request handlers for the management API (`http-axum` feature).
//!
//! Each handler owns one URL and the shape of its JSON; the domain work it
//! asks for lives in [`crate::control::domain`]. The URLs are declared in
//! [`crate::control::capability`] and registered in
//! [`super::server::management_router`].

pub mod vm;
