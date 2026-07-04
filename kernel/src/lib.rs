#![feature(gen_blocks)]
#![feature(coroutine_trait)]
#![feature(coroutines)]
#![feature(stmt_expr_attributes)]
#![feature(more_float_constants)]

mod pure;

pub use sinks::WriteResourceRequest;
pub use sources::ResourceRequest;
mod sinks;
mod sources;
pub mod space;
