#![feature(gen_blocks)]
#![feature(coroutine_trait)]
#![feature(coroutines)]
#![feature(stmt_expr_attributes)]
#![feature(more_float_constants)]

pub mod space;
mod sources;
mod sinks;
mod pure;
// Expose weightedsweep so it can be used by sinks and main (via mork::weightedsweep)
pub mod weightedsweep;
