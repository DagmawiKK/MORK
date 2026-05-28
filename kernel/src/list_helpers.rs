
use eval::{EvalScope, FuncType};
use eval_ffi::{EvalError, ExprSink, ExprSource, Tag};
use mork_expr::{Expr, ExprEnv, SourceItem};

pub fn expr_span(e: Expr) -> &'static [u8] {
    unsafe { e.span().as_ref().unwrap() }
}

pub fn exp_to_vec(tuple_expr: Expr) -> Result<Vec<Expr>, EvalError> {
    // 1. Read the raw byte directly from the expression's pointer
    let raw_byte = unsafe { *tuple_expr.ptr };
    // 2. Decode the raw byte into an MM2 Tag enum to find out what kind of AST node this is
    let expression_tag = mork_expr::byte_item(raw_byte);

    // 3. Check if the expression is a wrapper for a list of items
    match expression_tag {
        Tag::Arity(arity_size) => {
            // Initialize an environment to safely traverse this expression's bounds
            let mut parent_env = ExprEnv::new(0, tuple_expr);

            // Extract the child nodes into a collection of ExprEnvs.
            // Using `with_capacity` avoids unnecessary memory reallocations.
            // better than .collect() because we can pre-allocate the exact amount of space needed for the child environments, which can improve performance by reducing the number of memory allocations and copies.
            let mut child_envs = Vec::with_capacity(arity_size as usize);
            parent_env.args(&mut child_envs);

            // Finally, step through each child environment and resolve it back down into a raw `Expr`
            let mut extracted_expressions = Vec::with_capacity(arity_size as usize);
            for child_env in child_envs {
                // `.subsexpr()` isolates the child node into its own bounded Expr pointer
                let child_expr = child_env.subsexpr();
                extracted_expressions.push(child_expr);
            }
            Ok(extracted_expressions)
        }
        _ => {
            // If the tag is something else (like a Symbol or a Variable), it cannot be unpacked into a list
            Err(EvalError::from("expects a tuple/expression argument"))
        }
    }
}

pub fn vec_to_exp(sink: &mut ExprSink, items: &[Expr]) -> Result<(), EvalError> {
    sink.write(SourceItem::Tag(Tag::Arity(items.len() as u8)))?;
    for e in items {
        sink.extend_from_slice(expr_span(*e))?;
    }
    Ok(())
}

/// Extract the content bytes of a symbol expression, stripping the tag byte.
/// Returns None if the expression is not a symbol (e.g. an Arity node).
/// This is used by sort-atom to compare symbol contents lexicographically,
/// rather than comparing full spans which include the length-encoding tag byte.
pub fn expr_symbol_content(e: Expr) -> Option<&'static [u8]> {
    unsafe { e.symbol()?.as_ref() }
}

/// Parse each expression in a slice as an f64 number (stripping the tag byte),
/// returning an error if any element is not a valid numeric symbol.
/// Shared by min-atom, max-atom, and sort-math to avoid duplicating the
/// parse-to-f64 logic across multiple list-operation pure functions.
pub fn items_to_f64s(items: &[Expr]) -> Result<Vec<f64>, EvalError> {
    items.iter().map(|e| {
        let span = expr_span(*e);
        let s = unsafe { std::str::from_utf8_unchecked(
            span.get(1..).ok_or_else(|| EvalError::from("empty element"))?
        )};
        s.parse().map_err(|_| EvalError::from("element is not a number"))
    }).collect()
}