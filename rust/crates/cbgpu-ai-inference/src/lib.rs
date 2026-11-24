//! Native trusted local model worker. Generated FFI is isolated from safe job/prompt code.
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unsafe_op_in_unsafe_fn,
    clippy::all
)]
mod ffi;
pub mod native;
pub mod protocol;
mod qwen;
pub mod worker;
