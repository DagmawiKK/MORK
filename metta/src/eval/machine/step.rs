//! Stepping logic for machine execution.
//!
//! This module advances the evaluator by repeatedly dispatching expressions,
//! applying continuation frames, and executing direct state transitions.

use super::budget::{atoms_of, ResultSet};
use crate::atom::Atom;
use crate::env::Env;
use crate::func::{FnTable, NDet};
use crate::parser::Expr;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

thread_local! {
    /// Per-thread cancellation flag. When set and raised, `run_rs` on this
    /// thread bails out early. Installed by the `(once (hyperpose ...))` racing
    /// path so losing branches stop instead of running to the bitter end.
    static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Install (or clear) the cancellation flag for the current thread.
pub(crate) fn set_cancel(flag: Option<Arc<AtomicBool>>) {
    CANCEL.with(|c| *c.borrow_mut() = flag);
}

/// Evaluate an expression to a nondeterministic result stream.
pub fn run_as_ndet(expr: &Expr, env: &Env, funcs: &FnTable) -> Result<NDet, String> {
    let results = run(expr, env, funcs)?;
    Ok(NDet::stream(results.into_iter()))
}

/// Evaluate an expression to an eager ordered result multiset.
pub(crate) fn run(expr: &Expr, env: &Env, funcs: &FnTable) -> Result<Vec<Atom>, String> {
    let mut budget = None;
    let results = run_rs(Arc::new(expr.clone()), env.clone(), funcs, &mut budget)?;
    Ok(atoms_of(&results))
}

/// Evaluate an expression with an optional reduction budget.
pub(crate) fn run_budgeted(
    expr: &Expr,
    env: &Env,
    funcs: &FnTable,
    budget: &mut Option<i64>,
) -> Result<Vec<Atom>, String> {
    let results = run_rs(Arc::new(expr.clone()), env.clone(), funcs, budget)?;
    Ok(atoms_of(&results))
}

/// Run the machine until it produces a final result set.
pub(crate) fn run_rs(
    root: Arc<Expr>,
    root_env: Env,
    funcs: &FnTable,
    budget: &mut Option<i64>,
) -> Result<ResultSet, String> {
    let mut work = Vec::with_capacity(64);
    work.push(super::task::Task::Eval { expr: root, env: root_env });
    let mut vals: Vec<ResultSet> = Vec::with_capacity(32);

    // Snapshot the thread's cancellation flag once; polled on a stride so the
    // hot loop pays nothing when no flag is installed (the common case).
    let cancel = CANCEL.with(|c| c.borrow().clone());
    let mut steps: u32 = 0;

    while let Some(task) = work.pop() {
        if let Some(flag) = &cancel {
            steps = steps.wrapping_add(1);
            if steps & 0x3ff == 0 && flag.load(Ordering::Relaxed) {
                return Ok(Vec::new());
            }
        }
        match task {
            super::task::Task::Eval { expr, env } => {
                super::dispatch::dispatch_expr(&expr, &env, funcs, &mut work, &mut vals)?;
            }
            super::task::Task::Apply(frame) => {
                super::apply::apply_frame(frame, funcs, &mut work, &mut vals)?;
            }
            super::task::Task::Transition(transition) => {
                let result_set = super::transition::apply_transition(transition, funcs, budget)?;
                vals.push(result_set);
            }
        }
    }

    Ok(vals.pop().unwrap_or_default())
}
