//! Re-exports of fixed runtime primitives for generated schema crates from `type-bridge`.

pub use type_bridge::__codegen::*;
#[doc(hidden)]
pub use type_bridge::{
    FunctionArgument, FunctionCall, FunctionInput, FunctionScalarArgument, QuerySession,
};
