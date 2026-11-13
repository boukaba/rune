//! Rune runtime facade: event loop, timers, and host integration.
//!
//! The engine is synchronous; a runtime owns the job queues around it. The
//! first queue is timers (`setTimeout`/`clearTimeout`), with callbacks and
//! their arguments retained as GC roots until they run.

use rune_core::gc::SemiSpace;
use rune_core::value::Value;
use rune_embed::Context;
use rune_interpreter::vm::Vm;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Timer {
    id: i32,
    callback: usize,
    args: Vec<usize>,
    due: Instant,
    cancelled: bool,
}

#[derive(Default)]
struct TimerQueue {
    next_id: i32,
    timers: Vec<Timer>,
}

/// A Rune runtime with a host event loop.
pub struct Runtime {
    ctx: Context,
    timers: Rc<RefCell<TimerQueue>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    /// Create a runtime with the standard Web timer globals installed.
    pub fn new() -> Self {
        let mut ctx = Context::new();
        let timers = Rc::new(RefCell::new(TimerQueue::default()));

        let queue = timers.clone();
        ctx.register_host_fn("setTimeout", 1, move |_gc, _this, args, vm| {
            let callback = args.first().copied().unwrap_or(Value::undefined());
            if !is_callable(callback) {
                return Value::undefined();
            }
            let delay = args.get(1).and_then(|v| v.as_smi()).unwrap_or(0).max(0) as u64;
            let callback_root = vm.retain_value(callback);
            let arg_roots: Vec<usize> = args.iter().skip(2).map(|v| vm.retain_value(*v)).collect();
            let mut q = queue.borrow_mut();
            let id = q.next_id;
            q.next_id = q.next_id.wrapping_add(1);
            q.timers.push(Timer {
                id,
                callback: callback_root,
                args: arg_roots,
                due: Instant::now() + Duration::from_millis(delay),
                cancelled: false,
            });
            Value::smi(id)
        });

        let queue = timers.clone();
        ctx.register_host_fn("clearTimeout", 1, move |_gc, _this, args, vm| {
            let id = args.first().and_then(|v| v.as_smi()).unwrap_or(-1);
            let mut q = queue.borrow_mut();
            for timer in &mut q.timers {
                if timer.id == id && !timer.cancelled {
                    timer.cancelled = true;
                    vm.release_value(timer.callback);
                    for root in &timer.args {
                        vm.release_value(*root);
                    }
                }
            }
            Value::undefined()
        });

        Self { ctx, timers }
    }

    /// Evaluate a script, then run the timer/microtask jobs it queued.
    pub fn eval(&mut self, source: &str) -> Result<Value, String> {
        let value = self.ctx.eval(source)?;
        self.run_jobs()?;
        Ok(value)
    }

    /// Access the underlying context (host registration, values, snapshots).
    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.ctx
    }

    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// Run all currently queued timers in due-time order, draining microtasks
    /// after each callback. Timers added by callbacks are processed too.
    pub fn run_jobs(&mut self) -> Result<(), String> {
        loop {
            let next = {
                let mut q = self.timers.borrow_mut();
                if q.timers.is_empty() {
                    None
                } else {
                    let mut best = 0usize;
                    for i in 1..q.timers.len() {
                        if q.timers[i].due < q.timers[best].due {
                            best = i;
                        }
                    }
                    Some(q.timers.remove(best))
                }
            };
            let Some(timer) = next else {
                break;
            };
            if timer.cancelled {
                continue;
            }
            let now = Instant::now();
            if timer.due > now {
                std::thread::sleep(timer.due - now);
            }
            let callback = self.ctx.retained_value(timer.callback);
            let args: Vec<Value> = timer
                .args
                .iter()
                .map(|id| self.ctx.retained_value(*id))
                .collect();
            self.ctx.release_value(timer.callback);
            for root in &timer.args {
                self.ctx.release_value(*root);
            }
            self.ctx.call_value(callback, &args)?;
            self.ctx.drain_microtasks();
        }
        Ok(())
    }
}

fn is_callable(value: Value) -> bool {
    if value.as_smi().is_some_and(|s| s < 0) {
        return true;
    }
    value.heap_ptr().is_some_and(|p| unsafe {
        (*(p as *const rune_core::gc::GcHeader)).tag() == rune_core::gc::TAG_FUNC
    })
}

/// Compile-time assertion that the host API types are in scope (doc anchor).
#[allow(dead_code)]
fn _host_signature(_f: fn(&mut SemiSpace, Value, &[Value], &mut Vm) -> Value) {}
