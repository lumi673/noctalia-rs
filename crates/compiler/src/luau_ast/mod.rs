mod ast;
mod lower_ir;
mod parse;
mod passes;
mod print;

#[cfg(test)]
mod tests;

pub(crate) use lower_ir::lower_ir;
pub(crate) use parse::parse_script;
pub(crate) use passes::{
    fold_constants, prune_callbacks, remove_dead_code, specialize_tables,
};
pub(crate) use print::print_script;
