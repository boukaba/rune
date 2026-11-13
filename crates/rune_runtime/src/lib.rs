//! Rune runtime facade: event loop, timers, and host integration.
//!
//! The engine is synchronous; a runtime owns the job queues around it. The
//! first queue is timers (`setTimeout`/`clearTimeout`), with callbacks and
//! their arguments retained as GC roots until they run.

use rune_core::gc::{GcHeader, SemiSpace, TAG_OBJECT};
use rune_core::object::JSObject;
use rune_core::shape::PropertyKey;
use rune_core::value::Value;
use rune_embed::Context;
use rune_interpreter::builtins::value_to_js_string;
use rune_interpreter::vm::Vm;
use std::cell::RefCell;
use std::collections::VecDeque;
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

/// A request handed to a runtime fetch transport.
#[derive(Clone, Debug, Default)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// A response returned by a runtime fetch transport.
#[derive(Clone, Debug, Default)]
pub struct FetchResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

type Fetcher = Box<dyn FnMut(&FetchRequest) -> Result<FetchResponse, String>>;

struct FetchJob {
    request: FetchRequest,
    resolve: usize,
    reject: usize,
}

/// A Rune runtime with a host event loop.
pub struct Runtime {
    ctx: Context,
    timers: Rc<RefCell<TimerQueue>>,
    fetches: Rc<RefCell<VecDeque<FetchJob>>>,
    fetcher: Fetcher,
    response_factory: usize,
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

        let fetches = Rc::new(RefCell::new(VecDeque::new()));
        let queue = fetches.clone();
        ctx.register_host_fn("fetch", 1, move |gc, _this, args, vm| {
            let input = args.first().copied().unwrap_or(Value::undefined());
            let url = input_url(input);
            let mut request = FetchRequest {
                url,
                method: "GET".to_string(),
                headers: Vec::new(),
                body: None,
            };
            if let Some(init) = args.get(1).copied().and_then(heap_object) {
                if let Some(m) = get_own(init, "method").map(value_to_js_string) {
                    request.method = m.to_ascii_uppercase();
                }
                if let Some(btxt) = get_own(init, "body") {
                    if !btxt.is_undefined() && !btxt.is_null() {
                        request.body = Some(value_to_js_string(btxt));
                    }
                }
                if let Some(hobj) = get_own(init, "headers").and_then(heap_object) {
                    let shape = unsafe { JSObject::shape_ptr(hobj) };
                    for name in &shape.key_names {
                        if let Some(slot) = shape.lookup(&PropertyKey::from_string(name)) {
                            let v = unsafe { JSObject::get_slot(hobj, slot) };
                            request.headers.push((name.clone(), value_to_js_string(v)));
                        }
                    }
                }
            }
            let (promise, resolve, reject) = vm.new_pending_promise(gc);
            let resolve_root = vm.retain_value(resolve);
            let reject_root = vm.retain_value(reject);
            queue.borrow_mut().push_back(FetchJob {
                request,
                resolve: resolve_root,
                reject: reject_root,
            });
            promise
        });

        let factory_src = r#"
            var __rune_make_response = function (body, status, statusText, url, ok, headersText) {
                var h = {};
                var lines = headersText ? headersText.split("\n") : [];
                for (var i = 0; i < lines.length; i++) {
                    var line = lines[i];
                    if (!line) { continue; }
                    var p = line.indexOf("\t");
                    if (p < 0) { continue; }
                    h[line.slice(0, p).toLowerCase()] = line.slice(p + 1);
                }
                return {
                    status: status,
                    statusText: statusText,
                    ok: ok,
                    url: url,
                    headers: {
                        get: function (n) {
                            var k = String(n).toLowerCase();
                            return h[k] === undefined ? null : h[k];
                        },
                        has: function (n) {
                            return h[String(n).toLowerCase()] !== undefined;
                        }
                    },
                    text: function () { return Promise.resolve(body); },
                    json: function () { return Promise.resolve(JSON.parse(body)); }
                };
            };
        "#;
        ctx.eval(factory_src)
            .expect("response factory script parses");
        let factory = ctx
            .eval("__rune_make_response")
            .expect("response factory installs");
        let response_factory = ctx.retain_value(factory);

        Self {
            ctx,
            timers,
            fetches,
            fetcher: Box::new(|_| Err("fetch transport not configured".to_string())),
            response_factory,
        }
    }

    /// Install the host transport used by `fetch` jobs.
    pub fn set_fetcher<F>(&mut self, fetcher: F)
    where
        F: FnMut(&FetchRequest) -> Result<FetchResponse, String> + 'static,
    {
        self.fetcher = Box::new(fetcher);
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
            // Host I/O jobs run before timers; their resolution may queue
            // microtasks/timers that the next iterations process.
            let fetch = self.fetches.borrow_mut().pop_front();
            if let Some(job) = fetch {
                let result = (self.fetcher)(&job.request);
                let resolve = self.ctx.retained_value(job.resolve);
                let reject = self.ctx.retained_value(job.reject);
                self.ctx.release_value(job.resolve);
                self.ctx.release_value(job.reject);
                match result {
                    Ok(response) => {
                        let body = self.ctx.allocate_string(&response.body);
                        let status = Value::smi(response.status as i32);
                        let status_text = self.ctx.allocate_string(&response.status_text);
                        let url = self.ctx.allocate_string(&job.request.url);
                        let ok = Value::boolean(response.status < 400);
                        let headers_text =
                            self.ctx.allocate_string(&headers_blob(&response.headers));
                        let factory = self.ctx.retained_value(self.response_factory);
                        let response = self.ctx.call_value(
                            factory,
                            &[body, status, status_text, url, ok, headers_text],
                        )?;
                        self.ctx.call_value(resolve, &[response])?;
                    }
                    Err(err) => {
                        let msg = self.ctx.allocate_string(&err);
                        self.ctx.call_value(reject, &[msg])?;
                    }
                }
                self.ctx.drain_microtasks();
                continue;
            }
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

fn heap_object(value: Value) -> Option<*mut JSObject> {
    let ptr = value.heap_ptr()?;
    if unsafe { (*(ptr as *const GcHeader)).tag() } != TAG_OBJECT {
        return None;
    }
    Some(ptr as *mut JSObject)
}

fn get_own(obj: *mut JSObject, name: &str) -> Option<Value> {
    let shape = unsafe { JSObject::shape_ptr(obj) };
    let slot = shape.lookup(&PropertyKey::from_string(name))?;
    Some(unsafe { JSObject::get_slot(obj, slot) })
}

fn input_url(value: Value) -> String {
    if let Some(obj) = heap_object(value) {
        if let Some(href) = get_own(obj, "href") {
            return value_to_js_string(href);
        }
    }
    value_to_js_string(value)
}

fn headers_blob(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(k, v)| {
            format!(
                "{}\t{}\n",
                k.replace(['\t', '\n', '\r'], " "),
                v.replace(['\t', '\n', '\r'], " ")
            )
        })
        .collect()
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
