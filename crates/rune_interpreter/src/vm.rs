use crate::builtins::{
    Builtin, BuiltinFn, make_error, number_builtin, string_builtin, to_primitive_string,
    to_primitive_string_sync, value_to_js_string,
};
use crate::generator::Generator;
use crate::ic::{IcEntry, IcStats, InlineCache, LoopTrace, TraceOp};
use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};
use rune_core::array::RuneArray;
use rune_core::env::EnvObject;

use rune_core::accessor::AccessorPair;
use rune_core::date::{self, RuneDate};
use rune_core::function::{
    FUNC_FLAG_ASYNC, FUNC_FLAG_CLASS_CTOR, FUNC_FLAG_GENERATOR, FUNC_FLAG_STRICT, Func,
};
use rune_core::gc::{
    GcHeader, RootProvider, SemiSpace, TAG_ACCESSOR, TAG_ARRAY, TAG_ARRAY_BUFFER, TAG_DATE,
    TAG_FLOAT64, TAG_FUNC, TAG_MAP, TAG_OBJECT, TAG_PROMISE, TAG_REGEXP, TAG_SET, TAG_STRING,
    TAG_STRING_OBJ, TAG_TYPED_ARRAY,
};
use rune_core::map::{RuneMap, RuneSet};
use rune_core::object::JSObject;
use rune_core::promise::{PROMISE_FULFILLED, PROMISE_PENDING, PROMISE_REJECTED, Promise};
use rune_core::shape::{DENSE_ARRAY_SHAPE, PROTOTYPE_KEY, PropertyKey, Shape};
use rune_core::string::HeapString;
use rune_core::string_object::StringObject;
use rune_core::symbol::{register_symbol, symbol_description, symbol_display, symbol_for};
use rune_core::typedarray;
use rune_core::value::Value;
#[cfg(all(feature = "jit", target_arch = "aarch64"))]
use rune_jit_baseline::Aarch64CodeGen;
#[cfg(feature = "jit")]
use rune_jit_baseline::JIT_STACK_SIZE;
#[cfg(feature = "jit")]
use rune_jit_baseline::JitEntryFn;
// Without the `jit` feature rune_jit_baseline is not a dependency; mirror the
// constant so the Vm layout stays byte-identical across feature sets.
#[cfg(not(feature = "jit"))]
const JIT_STACK_SIZE: usize = 2048;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::collections::HashSet;

/// Identity of a hot loop: (program pointer as usize, back-edge target pc).
/// Distinct programs can share target pcs, so the pc alone is not a unique
/// key for trace recording/lookup.
type TraceKey = (usize, usize);

/// Convert a computed property key value to its string form (ToPropertyKey-lite).
fn property_key_string(val: Value) -> String {
    if val.is_undefined() {
        return "undefined".to_string();
    }
    if val.is_null() {
        return "null".to_string();
    }
    if val.is_boolean() {
        return if val.to_boolean().unwrap_or(false) {
            "true"
        } else {
            "false"
        }
        .to_string();
    }
    if let Some(n) = val.as_smi() {
        return n.to_string();
    }
    if val.is_float64() {
        let f = val.as_float64().unwrap_or(f64::NAN);
        // §7.1.12.1 Number::toString: non-finite values spell out
        if f.is_nan() {
            return "NaN".to_string();
        }
        if f == 0.0 {
            return "0".to_string();
        }
        if f.is_infinite() {
            return if f < 0.0 {
                "-Infinity".to_string()
            } else {
                "Infinity".to_string()
            };
        }
        return f.to_string();
    }
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            return unsafe { HeapString::to_string(ptr as *mut HeapString) };
        }
    }
    "[object Object]".to_string()
}

/// Callback for the `eval` builtin: parses and executes JS source, returns result.
pub type EvalFn = Box<dyn FnMut(&mut SemiSpace, &str) -> Result<Value, String>>;

struct Frame {
    locals: Vec<Value>,
    /// Lexical (let/const) slots for block-scoped bindings.
    lexical_slots: Vec<Value>,
    /// Parallel TDZ flags: true = binding is in temporal dead zone.
    lexical_tdz: Vec<bool>,
    /// Parallel const flags: true = binding is immutable.
    lexical_const: Vec<bool>,
    /// Stack of scope boundary indices into the lexical arrays.
    scope_boundaries: Vec<usize>,
    /// Number of arguments passed to this function (for `arguments` object).
    passed_argc: usize,
    pc: usize,
    stack_base: usize,
    prog: *const BytecodeProgram,
    generator_id: Option<usize>,
    this: Value,
    is_constructor_call: bool,
    constructed_object: Value,
    /// Pointer to this frame's lexical environment object (may be null).
    /// Set by MakeEnv at function entry. Child closures capture this pointer.
    env: *mut u8,
    /// Pointer to the Func struct of the function executing in this frame.
    /// Null for top-level script frames and generator resume frames.
    func_ptr: *mut u8,
    /// Private name IDs for the current class scope (RuneArray of SMI values).
    /// Set by PrivateNameScope, used by PrivateGet/Set/DefinePrivateField.
    /// This field exists on Frame rather than Func because the top-level
    /// script frame has no Func object (func_ptr is null).
    private_name_ids: *mut u8,
}

/// Result of the bytecode loop: normal return, generator yield, or throw.
#[derive(Debug)]
pub enum Exit {
    Return(Value),
    Yield(Value),
    Throw(Value),
}

/// Tracks a try-catch-finally block for exception unwinding.
#[derive(Copy, Clone)]
pub struct TryFrame {
    catch_pc: usize,
    finally_pc: usize,
    stack_depth: usize,
    frame_depth: usize,
    saved_exception: Option<Value>,
    in_catch: bool,
}

/// JIT bailout state, written by the bailout helper, read by vm.rs call site.
#[cfg(feature = "jit")]
#[derive(Clone, Debug)]
pub struct JitBailoutState {
    /// Bytecode PC where bailout occurred.
    pub bc_pc: usize,
    /// Set by bailout helper to signal a bailout. Checked by call site
    /// instead of `bc_pc != 0` because MakeArgumentsArray at PC 0 would
    /// collide with the "no bailout" sentinel.
    pub pending: bool,
    /// Snapshot of the JIT value stack at bailout.
    pub stack_snapshot: Vec<u64>,
    /// Reason tag (for stats/debugging).
    pub reason: rune_jit_baseline::BailoutReason,
}

#[cfg(feature = "jit")]
impl Default for JitBailoutState {
    fn default() -> Self {
        Self {
            bc_pc: 0,
            pending: false,
            stack_snapshot: Vec::new(),
            reason: rune_jit_baseline::BailoutReason::BailOnEntry,
        }
    }
}

/// JIT helper function pointers, stored at a fixed offset from vm_ptr
/// (offset JIT_HELPERS_OFFSET, right after jit_stack) so JIT code can load
/// and call them without cross-crate symbol resolution.
#[repr(C)]
pub struct JitHelpers {
    pub lexical_helper: usize,
    pub bailout_helper: usize,
    pub typeof_helper: usize,
    pub string_helper: usize,
    pub global_helper: usize,
    /// Helper that promotes Add operands to f64 on Smi overflow or non-Smi input.
    /// Called from JIT code to avoid bailing to the interpreter.
    pub float64_add_helper: usize,
    /// Call helper for JIT-to-JIT function calls (Phase E).
    pub call_helper: usize,
    /// JIT binary float helper: Div/Exp via one entry point (op id in x2).
    /// Replaces the former `_reserved` slot — JitHelpers layout size unchanged.
    pub jit_binop_helper: usize,
}

/// An ESM module record (§16.2.1.2 ModuleRecord, minimal form).
///
/// `env` holds the module's bindings (exported names and, for `export {a as b}`
/// renames, the export aliases). `namespace` caches the module namespace object
/// created on first `import * as ns`/`export * as ns` use.
#[derive(Clone, Debug)]
pub struct ModuleRecord {
    pub specifier: String,
    pub program: *const BytecodeProgram,
    /// 0 = unstarted, 1 = evaluating, 2 = evaluated (cycle guard).
    pub status: u8,
    pub env: HashMap<String, Value>,
    pub namespace: Option<Value>,
}

/// A cached call-IC entry: stores the expected callee Func pointer and its
/// JIT entry so the interpreter can skip the full Call dispatch on repeated
/// monomorphic calls to the same function.
#[derive(Clone, Debug)]
pub struct CallIcEntry {
    /// Heap pointer to the expected Func (monomorphic callee).
    pub func_ptr: *mut u8,
    /// Cached JIT entry for that Func (null if not yet compiled).
    pub jit_entry: *const u8,
    /// Number of arguments expected at this callsite.
    pub argc: usize,
}

impl Default for CallIcEntry {
    fn default() -> Self {
        Self {
            func_ptr: std::ptr::null_mut(),
            jit_entry: std::ptr::null(),
            argc: 0,
        }
    }
}

/// State for a pending assert.throws callback invocation.
/// Set by the assert_throws builtin, consumed by the Return/Throw handlers.
pub(crate) struct PendingAssert {
    /// The expected error constructor (e.g. TypeError handle).
    pub(crate) expected_error: Value,
    /// Number of frames on the stack when the assert was initiated.
    pub(crate) source_frame_depth: usize,
    /// Thunk frame index, pinned at arm time and NEVER rebased (B1f-4): the
    /// unwind path consumes when unwinding REACHES the thunk, while the
    /// Return path keeps following the dragged depth above. Without this,
    /// rebase drags the unwind check onto nested frames (eating throws meant
    /// for inner machines like IteratorClose) or past them (missing the thunk
    /// after a divert pops the inner frame). usize::MAX until the arm push.
    pub(crate) pinned_depth: usize,
}

/// What a pending_call frame's return needs (B1e: the generic arm must
/// stringify ToPrimitive results but pass .call/.apply results through).
#[derive(Clone, Copy)]
pub(crate) enum PendingCallCont {
    /// .call()/.apply(): push the raw result (legacy behavior).
    Raw,
    /// to_primitive_string user method: stringify a primitive result; on a
    /// non-primitive first result run valueOf (tried_valueof=false) or
    /// throw TypeError (true). `value` is the coerced object.
    ToPrimString { value: Value, tried_valueof: bool },
}

/// State for a pending Function.prototype.call invocation.
/// Set by the call builtin, consumed by the Return handler
/// when the target function's frame returns.
pub(crate) struct PendingCall {
    /// Number of frames on the stack when the call was initiated
    /// (the frame depth that, after pop, indicates the call returned).
    pub(crate) source_frame_depth: usize,
    pub(crate) cont: PendingCallCont,
}

/// State for a pending Promise constructor — stores the promise so it can be pushed
/// as the result after the executor callback returns.
#[allow(dead_code)]
pub(crate) struct PendingPromiseCtor {
    pub(crate) source_frame_depth: usize,
    pub(crate) promise: Value,
    pub(crate) resolve_handle: Value,
    pub(crate) reject_handle: Value,
    pub(crate) resolve_with_result: bool,
}

/// A microtask — a callback deferred to after the current synchronous task.
pub(crate) struct Microtask {
    pub(crate) callback: Value,
    pub(crate) args: Vec<Value>,
    pub(crate) promise_ctor: Option<PendingPromiseCtor>,
}

/// State for a pending primitive conversion (ToPrimitive via user-defined toString/valueOf).
/// Set by Opcode::Add, Opcode::ToString, etc., consumed by the Return handler.
pub(crate) struct PendingPrimitiveConversion {
    /// Frame depth when the conversion was requested.
    pub(crate) source_frame_depth: usize,
    /// The other operand (already primitive), to be pushed back after conversion completes.
    pub(crate) other_operand: Value,
}

/// Type of pending array operation (filter/map/reduce).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ArrayOpKind {
    Filter,
    Map,
    Reduce,
    /// B1c: reduceRight — same as Reduce, iterated backward.
    ReduceRight,
    ForEach,
    Find,
    FindIndex,
    /// B1c: findLast/findLastIndex — same as Find/FindIndex, backward.
    FindLast,
    FindLastIndex,
    Some,
    Every,
    FlatMap,
    /// B1b: indexOf/lastIndexOf/includes search (no user callback; the
    /// search value rides in `accumulator`, direction from the kind).
    IndexOf,
    LastIndexOf,
    Includes,
}

/// True for backward-iterated callback kinds (B1c). (LastIndexOf also
/// walks backward but through the search step, not the callback walk.)
pub(crate) fn array_op_is_backward(kind: ArrayOpKind) -> bool {
    matches!(
        kind,
        ArrayOpKind::ReduceRight | ArrayOpKind::FindLast | ArrayOpKind::FindLastIndex
    )
}

/// Pending Promise.prototype.finally operation.
/// Set by the finally builtin, consumed by the Return handler.
/// When onFinally returns, we settle the chained promise with
/// the original value (not the callback's return value).
pub(crate) struct PendingFinallyOp {
    pub(crate) promise: Value,
    pub(crate) orig_value: Value,
    pub(crate) is_reject: bool,
    pub(crate) source_frame_depth: usize,
}

/// State for a pending String.prototype.replace callback.
/// Set by the replace builtin, consumed by the Return handler.
/// Stores the match groups and input string so the Return handler
/// can construct the final result from the callback's return value.
pub(crate) struct PendingReplaceOp {
    pub(crate) source_frame_depth: usize,
    pub(crate) input: String,
    pub(crate) groups: Vec<(usize, usize)>,
}

/// State for a pending String.prototype.replaceAll callback.
/// Multi-match state machine: each callback return appends the substitution,
/// finds the next match, and either calls the callback again or finishes.
/// `search_str` is the plain-string pattern (mode A); `regex_pattern` is the
/// regex source (mode B, captures passed to the callback).
pub(crate) struct PendingReplaceAllOp {
    pub(crate) source_frame_depth: usize,
    pub(crate) input: String,
    pub(crate) search_str: String,
    pub(crate) regex_pattern: Option<String>,
    pub(crate) regex_flags: u32,
    pub(crate) fn_val: Value,
    pub(crate) next_pos: usize,
    pub(crate) accumulated: String,
    pub(crate) last_end: usize,
}

/// State for async generator resumption via bridge function (async_continue/async_reject).
pub(crate) struct PendingAsyncGen {
    pub(crate) gen_id: usize,
    pub(crate) arg: Value,
    #[allow(dead_code)]
    pub(crate) is_throw: bool,
}

/// Tracks an async function's outer Promise so the Return handler can resolve it.
pub(crate) struct AsyncTask {
    pub(crate) gen_id: usize,
    pub(crate) promise: *mut u8,
}

/// State for a pending accessor (getter/setter) call during property access.
/// Set by the LoadProperty or StoreProperty handler when the resolved property
/// value is an AccessorPair. Consumed by the Return handler when the getter/setter
/// frame returns.
pub(crate) struct PendingAccessorCall {
    pub(crate) source_frame_depth: usize,
    /// true if this is a getter call (load), false for a setter call (store)
    #[allow(dead_code)]
    pub(crate) is_getter: bool,
}

/// State for a pending well-known-symbol method dispatch (e.g. @@match, @@search,
/// @@split, @@replace from String.prototype methods). Set by the builtin when the
/// pattern argument is an object with a callable @@method; consumed by the Return
/// handler, which routes the method's return value back to the builtin's caller.
pub(crate) struct PendingSymbolDispatch {
    pub(crate) source_frame_depth: usize,
}

/// State for a pending Symbol(description)/Symbol.for(key) description coercion.
/// Set when the description/key is an object with a user-defined toString
/// (to_primitive_string set up a pending_call callback); consumed by the Return
/// handler, which wraps the toString result into a symbol value.
pub(crate) struct PendingSymbolCoercion {
    pub(crate) source_frame_depth: usize,
    /// true for Symbol.for (result is registered under the registry key),
    /// false for Symbol() (result becomes the symbol's description).
    pub(crate) is_for: bool,
}

/// State for a pending for..of iterator acquisition: the @@iterator method was
/// a JS function whose call returned. The Return handler validates the result,
/// loads the `next` method, and pushes [iterator, nextMethod] onto the stack.
pub(crate) struct PendingForOfInit {
    pub(crate) source_frame_depth: usize,
}

/// State for a pending for..of iteration step: the iterator's JS `next` method
/// returned. The Return handler checks done/value and either jumps to the loop
/// end (discarding [iterator, nextMethod]) or pushes the value and advances.
pub(crate) struct PendingForOfNext {
    pub(crate) source_frame_depth: usize,
    pub(crate) end_target: usize,
}

/// Pending `yield*` delegation step whose delegate `next` is a JS function.
/// Mirrors PendingForOfNext; the delegate return value becomes the yield*
/// expression value on done.
pub(crate) struct PendingYieldStarNext {
    pub(crate) source_frame_depth: usize,
    pub(crate) end_target: usize,
    /// Enclosing generator when the drain runs inside one (for divert).
    pub(crate) gen_id: Option<usize>,
}

pub(crate) enum YsGetOut {
    Ready(Value),
    Wait,
    Bail(Option<Exit>),
}

/// Outcome of the F3 property-access funnel (`vm_get_property`).
/// - `Ready(v)`: synchronous result; the opcode arm pushes it and advances.
/// - `Wait`: an accessor getter frame was pushed (via
///   `resolve_accessor_for_read`); the arm must `continue` without advancing
///   — the Return handler resumes the opcode.
pub(crate) enum PropGetOut {
    Ready(Value),
    Wait,
    /// An abrupt completion was raised inside the funnel (A1 null-check):
    /// `Some(exit)` propagates, `None` means handle_throw redirected to a
    /// catch/finally — the arm must `continue` without advancing.
    Bail(Option<Exit>),
}

/// Outcome of the F3 property-write funnel (`vm_set_property`).
/// - `Ready(v)`: synchronous; the arm pushes `v` (normally the stored value,
///   `undefined` for the getter-only skip path) and advances.
/// - `Wait`: an accessor setter frame was pushed; the arm must
///   `continue 'run` without advancing.
pub(crate) enum PropSetOut {
    Ready(Value),
    Wait,
    /// Same contract as `PropGetOut::Bail`.
    Bail(Option<Exit>),
}

/// Patch-site context for `vm_set_property`: the IC index plus the
/// program/pc of the issuing instruction for the 8-hit IC-patch counter.
/// `is_strict` selects throw vs silent-ignore when the store is rejected
/// (non-writable prop / non-extensible receiver, A4).
pub(crate) struct PropSetSite {
    pub ic_index: i64,
    pub prog_ptr: *const BytecodeProgram,
    pub pc: usize,
    pub is_strict: bool,
}

/// Whether a property key is the exact string `name` (A3 poison checks).
pub(crate) fn prop_key_is(raw_key: Value, name: &str) -> bool {
    raw_key.heap_ptr().is_some_and(|ptr| unsafe {
        (*(ptr as *const GcHeader)).tag() == TAG_STRING
            && HeapString::to_string(ptr as *mut HeapString) == name
    })
}

/// V8-style message for A1 RequireObjectCoercible failures:
/// "Cannot read/set properties of null/undefined (reading/setting key)".
/// test262 never asserts the text (only the TypeError), but quality matters.
pub(crate) fn null_prop_message(is_set: bool, obj_is_null: bool, key: Value) -> String {
    let recv = if obj_is_null { "null" } else { "undefined" };
    let (verb, gerund) = if is_set {
        ("set", "setting")
    } else {
        ("read", "reading")
    };
    let key_str = if let Some(ptr) = key.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            format!("'{}'", unsafe {
                HeapString::to_string(ptr as *mut HeapString)
            })
        } else {
            value_to_debug_string(key)
        }
    } else {
        value_to_debug_string(key)
    };
    format!("Cannot {verb} properties of {recv} ({gerund} {key_str})")
}

/// What a resumed async yield* property read continues as.
#[derive(Clone, Copy)]
pub(crate) enum YsGetPhase {
    /// Loaded delegate throw/return method → invoke or violation path.
    AfrMethod { is_throw: bool },
    /// Loaded IteratorClose return method (throw-violation path).
    CloseReturn,
    /// Loaded result.done → branch to value read or done path.
    StarDone { end_target: usize },
    /// Loaded result.value on the value path → push, advance.
    StarValue,
    /// Loaded result.value on the done path → drop pair, push, jump end.
    StarValueDone { end_target: usize },
    /// Loaded result.value in finish_* done path.
    FinishDone,
    /// Loaded result.value in finish_* value path.
    FinishValue { is_throw: bool },
}

/// Pending async property read for yield* machinery (accessor getters).
/// All Values are GC-rooted while pending.
pub(crate) struct PendingYieldStarGet {
    pub(crate) source_frame_depth: usize,
    pub(crate) gen_id: usize,
    pub(crate) phase: YsGetPhase,
    pub(crate) abrupt_arg: Value,
    pub(crate) iter: Value,
    /// Object under inspection (result object for Res phases).
    pub(crate) stash: Value,
}

/// Pending delegate throw()/return() call for yield* forwarding, or an
/// IteratorClose return() call. Values are GC-rooted while pending.
pub(crate) struct PendingYieldStarAbrupt {
    pub(crate) source_frame_depth: usize,
    pub(crate) gen_id: usize,
    pub(crate) is_throw: bool,
    pub(crate) abrupt_arg: Value,
    pub(crate) iter: Value,
    pub(crate) closing: bool,
}

/// Phase of a pending spread-drain (ToArrayFromIterable with JS callbacks).
#[derive(PartialEq, Clone, Copy)]
pub(crate) enum IterDrainState {
    /// Waiting for the user-defined @@iterator factory call to return.
    AwaitFactory,
    /// Waiting for the user-defined `next` call to return.
    AwaitNext,
}

/// State for a pending spread conversion (`[...x]`, `f(...x)`): the iterable's
/// @@iterator factory and/or `next` method are JS functions whose calls must be
/// resumed by the Return handler. The drained values accumulate into `result`.
pub(crate) struct PendingIterDrain {
    pub(crate) source_frame_depth: usize,
    pub(crate) state: IterDrainState,
    /// The iterator object (set once the factory returns).
    pub(crate) iter: Value,
    /// The `next` method (builtin handle or JS function).
    pub(crate) next: Value,
    /// Heap pointer to the result RuneArray being filled.
    pub(crate) result: *mut u8,
    /// The original iterable value (the factory's `this`).
    pub(crate) receiver: Value,
}

/// State machine for the Map/Set constructor filling from a user iterable
/// (the @@iterator factory and/or the `next` method are JS functions).
#[derive(Clone, Copy, Debug)]
pub(crate) enum CollectionCtorState {
    /// Waiting for the @@iterator factory call to return.
    AwaitFactory,
    /// Waiting for an iterator `next()` call to return.
    AwaitNext,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingCollectionCtor {
    pub(crate) source_frame_depth: usize,
    /// Stack depth of the `new Map()/Set()` caller — the state machine
    /// truncates to this depth whenever it resumes control.
    pub(crate) root_base: usize,
    pub(crate) state: CollectionCtorState,
    pub(crate) iter: Value,
    pub(crate) next: Value,
    pub(crate) collection: Value,
    pub(crate) is_map: bool,
}

/// State machine for Map/Set.prototype.forEach with a JS callback.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingCollectionForEach {
    pub(crate) source_frame_depth: usize,
    pub(crate) snapshot: *mut u8,
    pub(crate) idx: usize,
    pub(crate) found: usize,
    pub(crate) size: usize,
    pub(crate) is_map: bool,
    pub(crate) callback: Value,
    pub(crate) this_arg: Value,
    pub(crate) collection: Value,
}
/// One sortable element: the value plus its lazily resolved default-
/// comparator key (UTF-16 code units — precomputed keys would call ToString
/// even when no comparison runs, violating the 0/1-element observable
/// behavior). The key moves with the value through the merge.
pub(crate) struct SortItem {
    pub(crate) value: Value,
    pub(crate) key: Option<Vec<u16>>,
}

/// Sort machine phase (B1e): snapshot reads, then bottom-up merge with one
/// comparator round-trip per comparison, then writeback.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SortPhase {
    Read,
    Merge,
    Write,
}

/// What a pending ToPrimitive round-trip resolves (B1e): a coerced length
/// or a default-comparator string key for one merge side.
pub(crate) enum SortPrimPurpose {
    Length,
    /// Awaiting a length-getter frame (result coerces via ToLength).
    LengthGet,
    Key {
        is_left: bool,
    },
}

/// State machine for Array.prototype.sort/toSorted with a user comparator
/// or observable element access (B1e). Reads snapshot first
/// (SortIndexedProperties: HasProperty+Get per index, getters dispatched),
/// then merges stably (bottom-up, one comparator call per comparison),
/// then writes back with setter dispatch (sort) or into a fresh dense
/// array (toSorted, holes densified to undefined).
pub(crate) struct PendingSortOp {
    pub(crate) source_frame_depth: usize,
    pub(crate) source_val: Value,
    pub(crate) write_val: Value,
    /// undefined = default lexical comparator; else a callable.
    pub(crate) comparator: Value,
    /// toSorted (dense copy writeback) vs sort (in-place + hole deletes).
    pub(crate) is_copy: bool,
    /// toSorted reads through holes (every index via Get).
    pub(crate) read_all: bool,
    /// Fixed LengthOfArrayLike (spec: read once).
    pub(crate) len: u64,
    /// Length not yet resolved (first drive resolves, then reads).
    pub(crate) len_pending: bool,
    pub(crate) phase: SortPhase,
    // Read phase.
    pub(crate) read_idx: u64,
    pub(crate) await_read: bool,
    // ToPrimitive await (length coercion or string-key resolution).
    pub(crate) await_prim: bool,
    pub(crate) prim_value: Value,
    pub(crate) prim_tried_other: bool,
    pub(crate) prim_purpose: SortPrimPurpose,
    // Merge phase (bottom-up over items→aux).
    pub(crate) items: Vec<SortItem>,
    pub(crate) aux: Vec<SortItem>,
    pub(crate) width: usize,
    pub(crate) base: usize,
    pub(crate) i: usize,
    pub(crate) j: usize,
    pub(crate) k: usize,
    pub(crate) left_end: usize,
    pub(crate) right_end: usize,
    pub(crate) pair_end: usize,
    pub(crate) await_cmp: bool,
    // Write phase.
    pub(crate) write_idx: usize,
    pub(crate) await_set: bool,
    /// Func this machine is currently awaiting (copied from
    /// last_pushed_callee at push time). The Return arm only fires when
    /// the popped callee matches — nested foreign pushes share depths
    /// after rebase but never callees.
    pub(crate) await_callee: Value,
}

/// Array.from feed mode (B1f-4): iterable drain vs array-like index walk.
/// The iterable branch always goes through the @@iterator protocol (even for
/// arrays — custom @@iterator overrides must win); the array-like branch does
/// unconditional Gets (holes densify to undefined — no HasProperty gate).
pub(crate) enum FromFeed {
    /// Unconditional Get per index over a fixed length.
    ArrayLike { obj: Value, len: u64 },
    /// Iterator drain. `next` is resolved post-factory (undefined until the
    /// factory frame returns).
    Iter { iter: Value, next: Value },
}

/// PendingFromOp await discriminant (B1f-4). `None` while driving; set when a
/// JS frame is pushed so the Return arm can route its value back.
pub(crate) enum FromAwait {
    None,
    /// @@iterator() factory result → validate object, resolve next.
    Factory,
    /// Array-like element JS getter result → map/define it.
    Read,
    /// next() result → validate object, resolve done/value.
    Next,
    /// JS getter for done on an iterator result.
    Done,
    /// JS getter for value on an iterator result.
    Val,
    /// mapper() result → define it.
    Map,
    /// return() during IteratorClose → rethrow the stashed abrupt.
    Close,
}

/// Pending Array.from operation (B1f-4): feeds, maps, defines and closes per
/// §23.1.2.1. Drives synchronously (builtin next/mapfn inline) until a JS
/// frame is needed (Wait), then resumes in the Return arm by await_kind with
/// a callee-identity guard (sort pattern — nested foreign pushes share depths
/// after rebase but never callees).
pub(crate) struct PendingFromOp {
    pub(crate) source_frame_depth: usize,
    /// Fresh result (dense array, or plain object for Object-ctor dispatch).
    pub(crate) result: Value,
    pub(crate) feed: FromFeed,
    /// Next index to write (also the iterable length so far).
    pub(crate) k: u64,
    pub(crate) mapping: bool,
    pub(crate) mapper: Value,
    pub(crate) map_this: Value,
    pub(crate) await_kind: FromAwait,
    pub(crate) await_callee: Value,
    /// Staged iterator-result across done/value awaits.
    pub(crate) staged_value: Value,
    pub(crate) done_key: Value,
    pub(crate) value_key: Value,
    /// Abrupt being closed (IteratorClose then rethrow).
    pub(crate) closing_abrupt: Option<Value>,
}

/// Length-resolution suspension point (B1f-6b): which JS call is outstanding.
pub(crate) enum LengthStage {
    /// Length-slot JS getter outstanding (resume feeds its result to coerce).
    Get,
    /// ToPrimitive method call outstanding (0=@@toPrimitive, 1=valueOf,
    /// 2=toString); resume continues the chain after it.
    Call(u8),
}
/// Pending LengthOfArrayLike-with-frames (B1f-6b): resolves a JS-driven
/// length (getter / valueOf / toString / @@toPrimitive) then re-dispatches
/// the suspended builtin with the resolved value. Sync-resolvable lengths
/// never suspend (zero-cost fast path through length_stage).
pub(crate) struct PendingLengthOp {
    pub(crate) source_frame_depth: usize,
    /// Re-entry target once resolved (caller reruns; pre-length work in
    /// wired builtins is pure-or-alloc, so re-entry is side-effect free).
    pub(crate) caller: BuiltinFn,
    pub(crate) stash_this: Value,
    pub(crate) stash_args: Vec<Value>,
    /// In-flight value (getter result / object under coercion).
    pub(crate) staged: Value,
    /// Next ToPrimitive method to try (0=@@toPrimitive, 1=valueOf, 2=toString;
    /// 3+=exhausted). Persists across Call resumes.
    pub(crate) method_idx: u8,
    pub(crate) stage: LengthStage,
    pub(crate) await_callee: Value,
}

/// Species-Get suspension point (B1f-6c): which species Get is outstanding.
pub(crate) enum SpeciesStage {
    /// "constructor" JS getter outstanding (resume validates + reads species).
    GetCtor,
    /// @@species JS getter outstanding (resume validates the species).
    GetSpecies,
    /// Custom-ctor Construct outstanding (B1f-6d: resume installs the result).
    Construct,
    /// Array.of length-Set JS setter outstanding (B1f-6d: resume completes
    /// with the object — no re-dispatch, so the setter fires exactly once).
    OfLength,
}

/// Pending SpeciesConstructor-Gets (B1f-6c): runs the two Gets with frames
/// for JS getters, then re-dispatches the suspended builtin (which rebuilds
/// plain — Custom Construct lands in 6d). Sync-resolvable shapes never
/// suspend (zero-cost fast path through species_stage).
pub(crate) struct PendingSpeciesOp {
    pub(crate) source_frame_depth: usize,
    /// Re-entry target once resolved (caller reruns; pre-point work in
    /// wired builtins is pure-or-alloc, so re-entry is side-effect free).
    pub(crate) caller: BuiltinFn,
    pub(crate) stash_this: Value,
    pub(crate) stash_args: Vec<Value>,
    /// Resolved "constructor" value (set after the GetCtor leg).
    pub(crate) ctor: Value,
    /// Length argument for the custom-ctor Construct (B1f-6d: species len;
    /// callers pass it in because the machine re-drives from scratch).
    pub(crate) construct_len: u64,
    /// Fresh `this` of the outstanding Construct frame (B1f-6d): resume
    /// substitutes it when the ctor body returns a non-object.
    pub(crate) constructed_this: Value,
    pub(crate) stage: SpeciesStage,
    pub(crate) await_callee: Value,
}
/// Outcome of a builtin-driven Construct (B1f-6d): Done = built
/// synchronously (builtin ctors); Pushed = a JS ctor frame was pushed
/// (caller arms it as a Wait); Fail = spec TypeError.
pub(crate) enum ConstructOut {
    Done(Value),
    /// Frame pushed; payload is the fresh `this` (resume substitutes it
    /// when the body returns a non-object — New-path leniency).
    Pushed(Value),
    Fail(Value),
}

/// Set by the builtin function, consumed/updated by the Return handler.
pub(crate) struct ArrayOpState {
    pub(crate) kind: ArrayOpKind,
    /// Heap pointer to the source RuneArray being iterated (TAG_ARRAY only; use source_val for generic access).
    pub(crate) source: *mut u8,
    /// Heap pointer to the result RuneArray (for filter/map).
    pub(crate) result: *mut u8,
    /// The callback function value (a TAG_FUNC heap pointer).
    pub(crate) callback: Value,
    /// `this` value for the callback.
    pub(crate) this_val: Value,
    /// The original `this` value passed to the array builtin (TAG_ARRAY or TAG_OBJECT).
    pub(crate) source_val: Value,
    /// Current element index (0-based, post-increment after each callback).
    pub(crate) index: usize,
    /// Total number of elements in the source array.
    pub(crate) length: u32,
    /// Number of frames on the stack before the last callback frame was pushed.
    /// Set by the builtin before pushing the first callback frame.
    pub(crate) source_frame_depth: usize,
    /// Accumulator for reduce (set from initial value, updated per callback result).
    pub(crate) accumulator: Option<Value>,
    /// B1a: element-getter await — Some(index) while a JS getter frame
    /// pushed for that element is running (Return arm feeds its result back
    /// into the callback dispatch instead of processing a callback result).
    pub(crate) awaiting_element: Option<usize>,
    /// B1a: the awaited element feeds the reduce accumulator (not a callback
    /// argument). On resume the arm finishes reduce setup instead of
    /// dispatching. `index` holds the precomputed next-element index
    /// (usize::MAX = none remain → complete with the accumulator).
    pub(crate) awaiting_acc: bool,
    /// B1f-6d: non-dense species result to return at completion (Map/Filter/
    /// FlatMap only). The machine still fills a temp dense `result`
    /// (callbacks stay observable); completion swaps this in. Dense customs
    /// swap in as `result` directly and leave this None.
    pub(crate) species_fallback: Option<Value>,
}

/// Stack-based bytecode interpreter with call frame support.
#[repr(C)]
pub struct Vm {
    /// JIT-compiled trace value stack. Must remain the first field so that
    /// emitted AArch64 trace code can address it at offset 0 from the VM
    /// pointer (x19). Using heap memory for the JIT stack avoids macOS
    /// Apple Silicon restrictions on writes through the real stack pointer
    /// from JIT pages.
    ///
    /// Convention v2 (#4e-lite): divided into per-frame REGIONS claimed from
    /// `jit_stack_cursor` at each native entry (see
    /// `rune_jit_baseline::JIT_FRAME_BUDGET`). Nested native frames start
    /// ABOVE their caller's live slots — a callee can no longer clobber
    /// caller operands, and bailout snapshots are frame-relative.
    pub jit_stack: [u64; JIT_STACK_SIZE],
    /// JIT helper function pointer table. Must follow jit_stack immediately;
    /// emitted code loads helpers at `JIT_HELPERS_OFFSET` (= 2048*8) + idx*8.
    pub jit_helpers: JitHelpers,
    /// JIT stack base pointer, written by the JIT prologue.
    /// On AArch64: points to the base of jit_stack[] (== vm_ptr).
    /// On x86-64: points to the allocated native stack area (== initial rbx).
    pub jit_stack_base: u64,
    /// Monotonic region cursor (byte offset into jit_stack). Bumped by every
    /// native prologue by JIT_FRAME_BUDGET bytes; restored by epilogues and
    /// the overflow exit. Mirrors `[VM + JIT_CURSOR_OFFSET]`.
    pub jit_stack_cursor: u64,
    /// Pending-bailout flag read/written by emitted code at
    /// `JIT_FLAG_OFFSET` (was jit_stack[63] @ 504 before convention v2).
    pub jit_flag: u64,
    /// Bailout state, set by bailout helper during JIT execution.
    #[cfg(feature = "jit")]
    pub jit_bailout: JitBailoutState,
    /// Owned bailout tables, keyed by JIT entry pointer (see §10.3).
    #[cfg(feature = "jit")]
    pub bailout_tables: std::collections::HashMap<usize, Box<rune_jit_baseline::BailoutTable>>,
    pub stack: Vec<Value>,
    frames: Vec<Frame>,
    try_stack: Vec<TryFrame>,
    pub generators: Vec<Generator>,
    pub builtins: Vec<Builtin>,
    pub globals: HashMap<String, Value>,
    /// ESM module records, indexed by `modules[specifier]`. Programs are kept
    /// alive by the embedding Context; `program` points into its pinned heap.
    pub module_records: Vec<ModuleRecord>,
    /// Specifier → index into `module_records`.
    pub modules: HashMap<String, usize>,
    /// Module evaluation stack (record indices). The top entry is the module
    /// currently executing; module opcodes resolve against it.
    pub module_stack: Vec<usize>,
    /// While a module is evaluating, LoadGlobal/StoreGlobal are redirected
    /// into this module record's env instead of the shared globals map.
    pub globals_override: Option<usize>,
    /// Run-loop exit floor: a `Return` exits the loop when the frame count
    /// drops to this value (used for nested module evaluation).
    pub return_frame_floor: usize,
    /// While a generator's nested run_loop is active: the frame index the
    /// generator frame occupies. Return inside it must NOT run the module-
    /// style caller-stack truncation (the real caller lives outside this
    /// nested loop); the value travels via Exit::Return.
    nested_gen_floor: Option<usize>,
    /// Raw bits of the in-flight generator-return sentinel (see
    /// resume_generator_inner Return mode). While set, `handle_throw`
    /// skips user `catch` clauses for this value — return-completion runs
    /// `finally` blocks but is invisible to `catch`.
    generator_return_sentinel: Option<u64>,
    /// Set by `handle_throw` when it routes an exception into a user
    /// `catch` block (as opposed to a `finally` or unwinding out). Lets
    /// generator resumption know its injected abrupt was consumed.
    throw_routed_to_catch: bool,
    /// Stashed return value for an in-flight generator Return resumption,
    /// produced as the completion value when the sentinel unwinds out.
    generator_return_value: Option<Value>,
    /// Call-site ICs for monomorphic Call caching (Opcode::Call fast path).
    pub call_ics: Vec<CallIcEntry>,
    /// Reusable buffer for JIT locals Vec to avoid per-call heap allocation.
    /// Cleared and refilled on each JIT entry; sized to the largest locals set.
    pub jit_locals_buffer: Vec<Value>,
    /// Shape-Indexed Dispatch Tables for property access caching.
    pub ics: Vec<InlineCache>,
    /// Cached IC entries for bytecode-specialized callsites (LoadPropertyIC).
    pub ic_entries: Vec<IcEntry>,
    /// Per-callsite hit counters for bytecode patching threshold.
    ic_hit_counts: Vec<u32>,
    /// Aggregate IC statistics.
    pub ic_stats: IcStats,
    /// Cache of allocated HeapString pointers for each program's constant pool.
    /// Key: program pointer as usize, Value: Vec of string Value handles.
    string_cache: HashMap<usize, Vec<Value>>,
    /// Loop back-edge hotness: (prog_ptr, target_pc) → execution count.
    /// Back-edges are Jump targets where target < current_pc.
    /// Keyed by program pointer + pc because different functions can share
    /// the same target pc (e.g. two loops starting at pc 6 in different
    /// programs); a bare pc key would collide across programs and execute
    /// the wrong trace with the wrong frame.
    loop_counts: HashMap<TraceKey, u64>,
    /// Recorded traces for hot loops (TraceKey → LoopTrace).
    loop_traces: HashMap<TraceKey, LoopTrace>,
    /// If Some(TraceKey), we're currently recording a trace for that loop.
    recording_trace: Option<TraceKey>,
    /// Whether the current hot loop has already been patched.
    loop_patched: HashSet<TraceKey>,
    /// Loops whose trace was recorded but discarded (no back-edge); retry the
    /// recording on the next back-edge.
    pending_rerecord: HashSet<TraceKey>,
    /// Executable memory for compiled loop traces. Kept alive so entry points
    /// remain valid.
    #[cfg(feature = "jit")]
    _compiled_trace_mem: Vec<rune_jit_baseline::assembler::ExecutableMemory>,
    /// Pre-built constructor objects (like `Object`) that expose methods via property access.
    builtin_wrappers: HashMap<String, Value>,
    /// AFPC: cached JIT entry points by function index. When a cache is loaded,
    /// native code blobs are mmap'd and their addresses stored here; MakeFunction
    /// installs them on the newly-created Func objects.
    pub cached_jit_entries: HashMap<usize, *const u8>,
    /// Number of times the JIT entry path was taken (including bailout).
    /// Used by tests to verify JIT actually executed.
    pub jit_entry_count: u64,
    /// Number of JIT bailouts (all reasons). Helps detect wasteful JIT entries
    /// where a function always bails (e.g., always receives non-Smi args).
    pub jit_bailout_count: u64,
    /// Whether inlining is enabled for JIT trace compilation (--inline flag).
    /// When false, InlinePlan is built but emit_inline_call is never reached
    /// because the plan is built as empty.
    pub enable_inlining: bool,
    /// Use stencil-based code emission for JIT compilation (v0.3 copy-and-patch).
    pub stencil_jit: bool,
    /// Pre-allocated string Values for typeof results (indexed by TYPEOF_* constants).
    pub typeof_strings: [Value; 7],
    last_locals: Vec<Value>,
    pub eval_fn: UnsafeCell<Option<EvalFn>>,
    /// Reference to Array.prototype for setting on newly created arrays.
    pub array_prototype: Value,
    pub string_prototype: Value,
    pub string_constructor: Value,
    pub number_constructor: Value,
    pub object_constructor: Value,
    pub promise_constructor: Value,
    pub promise_prototype: Value,
    /// Bytecode bridge program for resolve/reject functions.
    promise_bridge_prog: *const BytecodeProgram,
    pub object_prototype: Value,
    pub function_prototype: Value,
    pub regexp_prototype: Value,
    /// Symbol constructor object (`Symbol` global) with well-known symbol statics.
    pub symbol_ctor: Value,
    pub map_constructor: Value,
    pub array_constructor: Value,
    pub set_constructor: Value,
    pub map_prototype: Value,
    pub set_prototype: Value,
    pub date_constructor: Value,
    pub date_prototype: Value,
    pub regexp_constructor: Value,
    pub array_buffer_constructor: Value,
    pub array_buffer_prototype: Value,
    /// TypedArray ctor wrappers / prototypes / ctor builtin handles, indexed by
    /// `typedarray::TypedArrayKind as usize` (order matches `from_index`).
    pub typed_array_ctors: Vec<Value>,
    pub typed_array_protos: Vec<Value>,
    pub typed_array_ctor_handles: Vec<Value>,
    /// Error-family ctor wrappers / prototypes, indexed by
    /// `builtins::ERROR_TYPE_NAMES` (Error, EvalError, RangeError,
    /// ReferenceError, SyntaxError, TypeError, URIError).
    pub error_ctors: Vec<Value>,
    pub error_protos: Vec<Value>,
    /// Wrapper objects that behave as functions (builtin constructors with
    /// Call/New dispatch). Their [[Prototype]] is %Function.prototype% and
    /// `typeof` reports "function".
    pub callable_wrappers: Vec<Value>,
    /// Symbol.prototype — where Symbol.prototype.toString/[@@toPrimitive] live.
    pub symbol_prototype: Value,
    /// Pending well-known-symbol @@method dispatch (set by String.prototype
    /// match/search/split/replace builtins, consumed by the Return handler).
    pub(crate) pending_symbol_dispatch: Option<PendingSymbolDispatch>,
    pub(crate) pending_symbol_coercion: Option<PendingSymbolCoercion>,
    /// Pending for..of iterator acquisition/iteration (JS @@iterator factory or
    /// JS `next` method called — resumed by the Return handler).
    pub(crate) pending_for_of_init: Option<PendingForOfInit>,
    pub(crate) pending_for_of_next: Option<PendingForOfNext>,
    pub(crate) pending_yield_star_next: Option<PendingYieldStarNext>,
    pub(crate) pending_yield_star_afr: Option<PendingYieldStarAbrupt>,
    pub(crate) pending_yield_star_get: Option<PendingYieldStarGet>,
    /// Pending spread drain (ToArrayFromIterable with JS callbacks).
    pub(crate) pending_iter_drain: Option<PendingIterDrain>,
    pub(crate) pending_collection_ctor: Option<PendingCollectionCtor>,
    pub(crate) pending_collection_foreach: Option<PendingCollectionForEach>,
    /// B1e: live sort/toSorted merge machine (see PendingSortOp).
    pub(crate) pending_sort_op: Option<PendingSortOp>,
    pub(crate) pending_from_op: Option<PendingFromOp>,
    /// B1f-6b: live length-resolution machine (see PendingLengthOp).
    pub(crate) pending_length_op: Option<PendingLengthOp>,
    /// Resolved length for builtin re-entry after a length suspension
    /// (B1f-6b): set synchronously inside the resume immediately before
    /// re-dispatching, consumed first-thing by length_stage. Single-flight
    /// by construction (no user code runs between set and consume).
    pub(crate) length_resume: Option<(u64, f64)>,
    /// B1f-6c: live species-Get machine (see PendingSpeciesOp).
    pub(crate) pending_species_op: Option<PendingSpeciesOp>,
    /// Species passed for builtin re-entry after a species suspension
    /// (B1f-6c): same single-flight contract as length_resume.
    pub(crate) species_passed: bool,
    /// Custom-ctor result for builtin re-entry after a Construct suspension
    /// (B1f-6d): the re-dispatched builtin installs it as its result.
    pub(crate) species_made: Option<Value>,
    /// B1e: func of the most recently pushed callback/getter frame
    /// (machines copy this into their await state at push time).
    pub(crate) last_pushed_callee: Value,
    /// B1e: callee of the most recently popped (returned) frame, for
    /// machine Return arms to verify against their awaited callee.
    pub(crate) last_popped_callee: Value,
    /// Registry id of the hidden symbol used to store iterator state on
    /// iterator objects (not exposed to JS code).
    pub(crate) iter_state_symbol: u32,
    /// Hidden slot symbol on generator instances ("__rune_gen") carrying the
    /// internal Generator id.
    pub(crate) gen_state_symbol: u32,
    /// Pre-allocated property keys used by the iteration protocol ("done",
    /// "value", "next").
    pub(crate) done_key: Value,
    pub(crate) value_key: Value,
    pub(crate) next_key: Value,
    /// Pending exception set by a builtin (checked after builtin dispatch).
    pub pending_exception: Option<Value>,
    /// Pending array operation (filter/map/reduce) with callback state machine.
    /// Set by the builtin, consumed/updated by the Return handler.
    pub(crate) pending_array_op: Option<ArrayOpState>,
    /// Pending Function.prototype.call invocation.
    pub(crate) pending_call: Option<PendingCall>,
    /// Pending Promise constructor call (executor → resolve/reject → return promise).
    pub(crate) pending_promise_ctor: Option<PendingPromiseCtor>,
    /// Microtask queue — Promise callbacks deferred to after sync execution.
    pub(crate) microtask_queue: Vec<Microtask>,
    /// Pending promise reactions: (callback, chained_promise) pairs keyed by promise ptr.
    pub(crate) promise_reactions: HashMap<*mut u8, Vec<(Value, Value)>>,
    /// Async generator tasks: maps gen_id → outer Promise so the Return handler can resolve it.
    pub(crate) async_tasks: Vec<AsyncTask>,
    /// Pending async generator resumption, set by async_continue/async_reject builtins.
    /// Consumed by the Return handler to restore the generator's frame.
    pub(crate) pending_async_gen: Option<PendingAsyncGen>,
    /// Pending Promise.prototype.finally operation.
    pub(crate) pending_finally_op: Option<PendingFinallyOp>,
    /// Pending String.prototype.replace callback state.
    pub(crate) pending_replace_op: Option<PendingReplaceOp>,
    /// Pending String.prototype.replaceAll callback state machine.
    pub(crate) pending_replace_all_op: Option<PendingReplaceAllOp>,
    /// Pending assert.throws callback.
    pub(crate) pending_assert: Option<PendingAssert>,
    /// Pending accessor call (getter/setter).
    pub(crate) pending_accessor_call: Option<PendingAccessorCall>,
    /// Pending primitive conversion (ToPrimitive on object for +, ToString, etc.).
    pub(crate) pending_primitive_conversion: Option<PendingPrimitiveConversion>,
    /// Whether an assert.* function was called during the current execution.
    /// Reset to false at the start of execute(). Used by the test262 runner
    /// to distinguish "test passed" from "test ran without asserting anything."
    pub assert_called: bool,
    /// Counter for allocating unique private name IDs (class fields).
    /// Incremented per PrivateNameScope allocation.
    next_private_name_id: u64,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

/// How a suspended generator is resumed.
#[derive(Copy, Clone)]
pub enum GeneratorResume {
    /// `next(v)`: `v` becomes the suspended yield's value.
    Next(Value),
    /// `throw(e)`: unwind from the suspend point with `e`.
    Throw(Value),
    /// `return(v)`: run finallys, then complete with `v` (via sentinel).
    Return(Value),
}

/// Outcome of a yield* delegation divert.
#[allow(dead_code)]
enum DivertOut {
    /// Fully handled: caller must `continue` the run loop.
    Handled,
    /// Unwinding continues with this exit.
    Throw(Exit),
}

impl Vm {
    pub fn new() -> Self {
        Vm {
            jit_stack: [0; JIT_STACK_SIZE],
            jit_helpers: JitHelpers {
                lexical_helper: rune_jit_lexical_helper as *const () as usize,
                #[cfg(feature = "jit")]
                bailout_helper: rune_jit_bailout_helper as *const () as usize,
                #[cfg(not(feature = "jit"))]
                bailout_helper: 0,
                typeof_helper: rune_jit_typeof_helper as *const () as usize,
                string_helper: rune_jit_string_helper as *const () as usize,
                global_helper: rune_jit_global_helper as *const () as usize,
                float64_add_helper: rune_jit_float64_add_helper as *const () as usize,
                #[cfg(feature = "jit")]
                call_helper: rune_jit_call_helper as *const () as usize,
                #[cfg(not(feature = "jit"))]
                call_helper: 0,
                jit_binop_helper: rune_jit_float64_div_exp_helper as *const () as usize,
            },
            jit_stack_base: 0,
            jit_stack_cursor: 0,
            jit_flag: 0,
            #[cfg(feature = "jit")]
            jit_bailout: JitBailoutState::default(),
            #[cfg(feature = "jit")]
            bailout_tables: std::collections::HashMap::new(),
            stack: Vec::new(),
            frames: Vec::new(),
            try_stack: Vec::new(),
            generators: Vec::new(),
            builtins: Vec::new(),
            globals: HashMap::new(),
            call_ics: Vec::new(),
            module_records: Vec::new(),
            modules: HashMap::new(),
            module_stack: Vec::new(),
            globals_override: None,
            return_frame_floor: 0,
            nested_gen_floor: None,
            generator_return_sentinel: None,
            generator_return_value: None,
            throw_routed_to_catch: false,
            jit_locals_buffer: Vec::new(),
            ics: Vec::new(),
            ic_entries: Vec::new(),
            ic_hit_counts: Vec::new(),
            ic_stats: IcStats::default(),
            string_cache: HashMap::new(),
            loop_counts: HashMap::new(),
            loop_traces: HashMap::new(),
            recording_trace: None,
            loop_patched: HashSet::new(),
            pending_rerecord: HashSet::new(),
            #[cfg(feature = "jit")]
            _compiled_trace_mem: Vec::new(),
            builtin_wrappers: HashMap::new(),
            cached_jit_entries: HashMap::new(),
            jit_entry_count: 0,
            jit_bailout_count: 0,
            enable_inlining: false,
            stencil_jit: false,
            typeof_strings: [Value::undefined(); 7],
            last_locals: Vec::new(),
            eval_fn: UnsafeCell::new(None),
            array_prototype: Value::undefined(),
            string_prototype: Value::undefined(),
            string_constructor: Value::undefined(),
            object_constructor: Value::undefined(),
            number_constructor: Value::undefined(),
            promise_constructor: Value::undefined(),
            promise_prototype: Value::undefined(),
            promise_bridge_prog: std::ptr::null(),
            object_prototype: Value::undefined(),
            function_prototype: Value::undefined(),
            regexp_prototype: Value::undefined(),
            symbol_ctor: Value::undefined(),
            map_constructor: Value::undefined(),
            array_constructor: Value::undefined(),
            set_constructor: Value::undefined(),
            map_prototype: Value::undefined(),
            set_prototype: Value::undefined(),
            date_constructor: Value::undefined(),
            date_prototype: Value::undefined(),
            regexp_constructor: Value::undefined(),
            array_buffer_constructor: Value::undefined(),
            array_buffer_prototype: Value::undefined(),
            typed_array_ctors: Vec::new(),
            typed_array_protos: Vec::new(),
            typed_array_ctor_handles: Vec::new(),
            error_ctors: Vec::new(),
            callable_wrappers: Vec::new(),
            error_protos: Vec::new(),
            symbol_prototype: Value::undefined(),
            pending_symbol_dispatch: None,
            pending_symbol_coercion: None,
            pending_for_of_init: None,
            pending_for_of_next: None,
            pending_yield_star_next: None,
            pending_yield_star_afr: None,
            pending_yield_star_get: None,
            pending_iter_drain: None,
            pending_collection_ctor: None,
            pending_collection_foreach: None,
            pending_sort_op: None,
            pending_from_op: None,
            pending_length_op: None,
            length_resume: None,
            pending_species_op: None,
            species_passed: false,
            species_made: None,
            last_pushed_callee: Value::undefined(),
            last_popped_callee: Value::undefined(),
            iter_state_symbol: rune_core::symbol::symbol_for("__rune_iter_state"),
            gen_state_symbol: rune_core::symbol::symbol_for("__rune_gen"),
            done_key: Value::undefined(),
            value_key: Value::undefined(),
            next_key: Value::undefined(),
            pending_exception: None,
            pending_array_op: None,
            pending_call: None,
            pending_promise_ctor: None,
            microtask_queue: Vec::new(),
            promise_reactions: HashMap::new(),
            async_tasks: Vec::new(),
            pending_async_gen: None,
            pending_finally_op: None,
            pending_replace_op: None,
            pending_replace_all_op: None,
            pending_assert: None,
            pending_accessor_call: None,
            pending_primitive_conversion: None,
            assert_called: false,
            next_private_name_id: 1,
        }
    }

    /// Build pre-wired constructor objects (Object, etc.) in the GC heap.
    /// Must be called after all builtins are registered.
    pub fn init_builtin_wrappers(&mut self, gc: &mut SemiSpace) {
        fn find_handle(builtins: &[Builtin], name: &str) -> Option<Value> {
            builtins
                .iter()
                .position(|b| b.name == name)
                .map(|id| Value::smi(-(id as i32) - 1))
        }
        fn make_object(gc: &mut SemiSpace, pairs: &[(&str, Value)]) -> Value {
            let keys: Vec<(PropertyKey, usize)> = pairs
                .iter()
                .enumerate()
                .map(|(i, (k, _))| (PropertyKey::from_string(k), i))
                .collect();
            let key_names: Vec<String> = pairs.iter().map(|(k, _)| k.to_string()).collect();
            let shape = Shape::intern(keys, key_names);
            let vals: Vec<Value> = pairs.iter().map(|(_, v)| *v).collect();
            let obj_ptr = JSObject::allocate(gc, shape, &vals);
            Value::from_heap_ptr(obj_ptr as *mut u8)
        }

        // Object constructor with .create(), .keys(), .values(), .entries()
        let create_handle = find_handle(&self.builtins, "Object_create");
        let keys_handle = find_handle(&self.builtins, "Object_keys");
        let values_handle = find_handle(&self.builtins, "Object_values");
        let entries_handle = find_handle(&self.builtins, "Object_entries");
        if create_handle.is_some() || keys_handle.is_some() {
            let mut obj_entries: Vec<(&str, Value)> = Vec::new();
            if let Some(h) = create_handle {
                obj_entries.push(("create", h));
            }
            if let Some(h) = keys_handle {
                obj_entries.push(("keys", h));
            }
            if let Some(h) = values_handle {
                obj_entries.push(("values", h));
            }
            if let Some(h) = entries_handle {
                obj_entries.push(("entries", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_assign") {
                obj_entries.push(("assign", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_is") {
                obj_entries.push(("is", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_getOwnPropertyNames") {
                obj_entries.push(("getOwnPropertyNames", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_fromEntries") {
                obj_entries.push(("fromEntries", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_hasOwn") {
                obj_entries.push(("hasOwn", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_setPrototypeOf") {
                obj_entries.push(("setPrototypeOf", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_defineProperty") {
                obj_entries.push(("defineProperty", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_defineProperties") {
                obj_entries.push(("defineProperties", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_getOwnPropertyDescriptor") {
                obj_entries.push(("getOwnPropertyDescriptor", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_preventExtensions") {
                obj_entries.push(("preventExtensions", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_seal") {
                obj_entries.push(("seal", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_freeze") {
                obj_entries.push(("freeze", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_isExtensible") {
                obj_entries.push(("isExtensible", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_isSealed") {
                obj_entries.push(("isSealed", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Object_isFrozen") {
                obj_entries.push(("isFrozen", h));
            }
            let obj_val = make_object(gc, &obj_entries);
            self.object_constructor = obj_val;
            self.builtin_wrappers.insert("Object".to_string(), obj_val);
            // Object.getPrototypeOf — attach later (prototype objects don't
            // exist yet at this point).
        }

        // Array.prototype with push/pop/filter/map/reduce methods
        let push_handle = find_handle(&self.builtins, "Array_prototype_push");
        let pop_handle = find_handle(&self.builtins, "Array_prototype_pop");
        let filter_handle = find_handle(&self.builtins, "Array_prototype_filter");
        let map_handle = find_handle(&self.builtins, "Array_prototype_map");
        let reduce_handle = find_handle(&self.builtins, "Array_prototype_reduce");
        let for_each_handle = find_handle(&self.builtins, "Array_prototype_forEach");
        let slice_handle = find_handle(&self.builtins, "Array_prototype_slice");
        let includes_handle = find_handle(&self.builtins, "Array_prototype_includes");
        let index_of_handle = find_handle(&self.builtins, "Array_prototype_indexOf");
        let find_h = find_handle(&self.builtins, "Array_prototype_find");
        let find_index_h = find_handle(&self.builtins, "Array_prototype_findIndex");
        let some_h = find_handle(&self.builtins, "Array_prototype_some");
        let every_h = find_handle(&self.builtins, "Array_prototype_every");
        let flat_h = find_handle(&self.builtins, "Array_prototype_flat");
        let flat_map_h = find_handle(&self.builtins, "Array_prototype_flatMap");
        let reverse_handle = find_handle(&self.builtins, "Array_prototype_reverse");
        let concat_handle = find_handle(&self.builtins, "Array_prototype_concat");
        let shift_handle = find_handle(&self.builtins, "Array_prototype_shift");
        let unshift_handle = find_handle(&self.builtins, "Array_prototype_unshift");
        let splice_handle = find_handle(&self.builtins, "Array_prototype_splice");
        if let (Some(push), Some(pop)) = (push_handle, pop_handle) {
            let mut proto_entries: Vec<(&str, Value)> = vec![("push", push), ("pop", pop)];
            if let Some(f) = filter_handle {
                proto_entries.push(("filter", f));
            }
            if let Some(m) = map_handle {
                proto_entries.push(("map", m));
            }
            if let Some(r) = reduce_handle {
                proto_entries.push(("reduce", r));
            }
            if let Some(rr) = find_handle(&self.builtins, "Array_prototype_reduceRight") {
                proto_entries.push(("reduceRight", rr));
            }
            if let Some(fe) = for_each_handle {
                proto_entries.push(("forEach", fe));
            }
            if let Some(s) = slice_handle {
                proto_entries.push(("slice", s));
            }
            if let Some(incl) = includes_handle {
                proto_entries.push(("includes", incl));
            }
            if let Some(iof) = index_of_handle {
                proto_entries.push(("indexOf", iof));
            }
            if let Some(lh) = find_handle(&self.builtins, "Array_prototype_lastIndexOf") {
                proto_entries.push(("lastIndexOf", lh));
            }
            if let Some(jh) = find_handle(&self.builtins, "Array_prototype_join") {
                proto_entries.push(("join", jh));
            }
            if let Some(fnd) = find_h {
                proto_entries.push(("find", fnd));
            }
            if let Some(fi) = find_index_h {
                proto_entries.push(("findIndex", fi));
            }
            if let Some(fl) = find_handle(&self.builtins, "Array_prototype_findLast") {
                proto_entries.push(("findLast", fl));
            }
            if let Some(fli) = find_handle(&self.builtins, "Array_prototype_findLastIndex") {
                proto_entries.push(("findLastIndex", fli));
            }
            if let Some(sm) = some_h {
                proto_entries.push(("some", sm));
            }
            if let Some(ev) = every_h {
                proto_entries.push(("every", ev));
            }
            if let Some(fl) = flat_h {
                proto_entries.push(("flat", fl));
            }
            if let Some(fm) = flat_map_h {
                proto_entries.push(("flatMap", fm));
            }
            if let Some(rv) = reverse_handle {
                proto_entries.push(("reverse", rv));
            }
            if let Some(cc) = concat_handle {
                proto_entries.push(("concat", cc));
            }
            if let Some(shf) = shift_handle {
                proto_entries.push(("shift", shf));
            }
            if let Some(ush) = unshift_handle {
                proto_entries.push(("unshift", ush));
            }
            if let Some(sp) = splice_handle {
                proto_entries.push(("splice", sp));
            }
            if let Some(h) = find_handle(&self.builtins, "Array_prototype_copyWithin") {
                proto_entries.push(("copyWithin", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Array_prototype_toSpliced") {
                proto_entries.push(("toSpliced", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Array_prototype_with") {
                proto_entries.push(("with", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Array_prototype_fill") {
                proto_entries.push(("fill", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Array_prototype_toReversed") {
                proto_entries.push(("toReversed", h));
            }
            if let Some(sh) = find_handle(&self.builtins, "Array_prototype_sort") {
                proto_entries.push(("sort", sh));
            }
            if let Some(sh) = find_handle(&self.builtins, "Array_prototype_toSorted") {
                proto_entries.push(("toSorted", sh));
            }
            let arr_proto = make_object(gc, &proto_entries);
            self.builtin_wrappers
                .insert("Array.prototype".to_string(), arr_proto);
            self.array_prototype = arr_proto;
        }

        // String.prototype methods
        let char_at_handle = find_handle(&self.builtins, "String_prototype_charAt");
        let slice_handle = find_handle(&self.builtins, "String_prototype_slice");
        let split_handle = find_handle(&self.builtins, "String_prototype_split");
        let index_of_handle = find_handle(&self.builtins, "String_prototype_indexOf");
        let includes_handle = find_handle(&self.builtins, "String_prototype_includes");
        let starts_with_handle = find_handle(&self.builtins, "String_prototype_startsWith");
        let ends_with_handle = find_handle(&self.builtins, "String_prototype_endsWith");
        let char_code_at_handle = find_handle(&self.builtins, "String_prototype_charCodeAt");
        let code_point_at_handle = find_handle(&self.builtins, "String_prototype_codePointAt");
        let substring_handle = find_handle(&self.builtins, "String_prototype_substring");
        let substr_handle = find_handle(&self.builtins, "String_prototype_substr");
        let trim_handle = find_handle(&self.builtins, "String_prototype_trim");
        let trim_start_handle = find_handle(&self.builtins, "String_prototype_trimStart");
        let trim_end_handle = find_handle(&self.builtins, "String_prototype_trimEnd");
        let to_lower_handle = find_handle(&self.builtins, "String_prototype_toLowerCase");
        let to_upper_handle = find_handle(&self.builtins, "String_prototype_toUpperCase");
        let repeat_handle = find_handle(&self.builtins, "String_prototype_repeat");
        let pad_start_handle = find_handle(&self.builtins, "String_prototype_padStart");
        let pad_end_handle = find_handle(&self.builtins, "String_prototype_padEnd");
        let concat_handle = find_handle(&self.builtins, "String_prototype_concat");
        let to_string_handle = find_handle(&self.builtins, "String_prototype_toString");
        let value_of_handle = find_handle(&self.builtins, "String_prototype_valueOf");
        let replace_handle = find_handle(&self.builtins, "String_prototype_replace");
        let replace_all_handle = find_handle(&self.builtins, "String_prototype_replaceAll");
        let match_handle = find_handle(&self.builtins, "String_prototype_match");
        let search_handle = find_handle(&self.builtins, "String_prototype_search");
        if let (Some(char_at), Some(slice)) = (char_at_handle, slice_handle) {
            let mut str_proto_entries: Vec<(&str, Value)> =
                vec![("charAt", char_at), ("slice", slice)];
            if let Some(split) = split_handle {
                str_proto_entries.push(("split", split));
            }
            if let Some(idx) = index_of_handle {
                str_proto_entries.push(("indexOf", idx));
            }
            if let Some(incl) = includes_handle {
                str_proto_entries.push(("includes", incl));
            }
            if let Some(sw) = starts_with_handle {
                str_proto_entries.push(("startsWith", sw));
            }
            if let Some(ew) = ends_with_handle {
                str_proto_entries.push(("endsWith", ew));
            }
            if let Some(cc) = char_code_at_handle {
                str_proto_entries.push(("charCodeAt", cc));
            }
            if let Some(cp) = code_point_at_handle {
                str_proto_entries.push(("codePointAt", cp));
            }
            if let Some(sb) = substring_handle {
                str_proto_entries.push(("substring", sb));
            }
            if let Some(sr) = substr_handle {
                str_proto_entries.push(("substr", sr));
            }
            if let Some(tr) = trim_handle {
                str_proto_entries.push(("trim", tr));
            }
            if let Some(ts) = trim_start_handle {
                str_proto_entries.push(("trimStart", ts));
            }
            if let Some(te) = trim_end_handle {
                str_proto_entries.push(("trimEnd", te));
            }
            if let Some(tl) = to_lower_handle {
                str_proto_entries.push(("toLowerCase", tl));
            }
            if let Some(tu) = to_upper_handle {
                str_proto_entries.push(("toUpperCase", tu));
            }
            if let Some(rp) = repeat_handle {
                str_proto_entries.push(("repeat", rp));
            }
            if let Some(ps) = pad_start_handle {
                str_proto_entries.push(("padStart", ps));
            }
            if let Some(pe) = pad_end_handle {
                str_proto_entries.push(("padEnd", pe));
            }
            if let Some(cn) = concat_handle {
                str_proto_entries.push(("concat", cn));
            }
            if let Some(ts) = to_string_handle {
                str_proto_entries.push(("toString", ts));
            }
            if let Some(vo) = value_of_handle {
                str_proto_entries.push(("valueOf", vo));
            }
            if let Some(rp) = replace_handle {
                str_proto_entries.push(("replace", rp));
            }
            if let Some(ra) = replace_all_handle {
                str_proto_entries.push(("replaceAll", ra));
            }
            if let Some(mh) = match_handle {
                str_proto_entries.push(("match", mh));
            }
            if let Some(sh) = search_handle {
                str_proto_entries.push(("search", sh));
            }
            let str_proto = make_object(gc, &str_proto_entries);
            self.builtin_wrappers
                .insert("String.prototype".to_string(), str_proto);
            self.string_prototype = str_proto;
        }

        // Array constructor with .isArray() and .prototype
        if let Some(handle) = find_handle(&self.builtins, "Array_isArray") {
            let arr_proto_val = self
                .builtin_wrappers
                .get("Array.prototype")
                .copied()
                .unwrap_or(Value::undefined());
            let len_val = Value::smi(1);
            let name_val = Value::from_heap_ptr(HeapString::allocate(gc, "Array") as *mut u8);
            let mut ctor_entries: Vec<(&str, Value)> =
                vec![("isArray", handle), ("prototype", arr_proto_val)];
            if let Some(h) = find_handle(&self.builtins, "Array_from") {
                ctor_entries.push(("from", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Array_of") {
                ctor_entries.push(("of", h));
            }
            ctor_entries.push(("length", len_val));
            ctor_entries.push(("name", name_val));
            let arr_ctor = make_object(gc, &ctor_entries);
            self.builtin_wrappers.insert("Array".to_string(), arr_ctor);
            self.array_constructor = arr_ctor;
        }

        // String constructor with .fromCharCode() and .prototype
        if let Some(handle) = find_handle(&self.builtins, "String_fromCharCode") {
            let str_proto_val = self
                .builtin_wrappers
                .get("String.prototype")
                .copied()
                .unwrap_or(Value::undefined());
            let str_ctor = make_object(
                gc,
                &[("fromCharCode", handle), ("prototype", str_proto_val)],
            );
            self.string_constructor = str_ctor;
            self.builtin_wrappers.insert("String".to_string(), str_ctor);
        }

        // Number constructor with .prototype + numeric constants (§21.1.2).
        // B1f-1: POSITIVE_INFINITY (and friends) unblock the S15 length
        // families, which assign them to .length and compare against them.
        if find_handle(&self.builtins, "Number").is_some() {
            let num_ctor = make_object(
                gc,
                &[
                    ("prototype", Value::undefined()),
                    ("EPSILON", Value::from_float64(f64::EPSILON)),
                    (
                        "MAX_SAFE_INTEGER",
                        Value::from_float64(9_007_199_254_740_991.0),
                    ),
                    (
                        "MIN_SAFE_INTEGER",
                        Value::from_float64(-9_007_199_254_740_991.0),
                    ),
                    ("MAX_VALUE", Value::from_float64(f64::MAX)),
                    ("MIN_VALUE", Value::from_float64(f64::MIN_POSITIVE)),
                    ("NaN", Value::from_float64(f64::NAN)),
                    ("NEGATIVE_INFINITY", Value::from_float64(f64::NEG_INFINITY)),
                    ("POSITIVE_INFINITY", Value::from_float64(f64::INFINITY)),
                ],
            );
            self.number_constructor = num_ctor;
            self.builtin_wrappers.insert("Number".to_string(), num_ctor);
        }

        // Symbol constructor — well-known symbol statics + Symbol.prototype.
        // `Symbol(x)` is dispatched from Opcode::Call via self.symbol_ctor;
        // `new Symbol(x)` throws a TypeError (see Opcode::New).
        if find_handle(&self.builtins, "Symbol").is_some() {
            let sym_ctor_handle =
                find_handle(&self.builtins, "Symbol").unwrap_or(Value::undefined());
            let mut proto_entries: Vec<(&str, Value)> = Vec::new();
            if let Some(h) = find_handle(&self.builtins, "Symbol_prototype_toString") {
                proto_entries.push(("toString", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Symbol_prototype_valueOf") {
                proto_entries.push(("valueOf", h));
            }
            proto_entries.push(("description", Value::undefined()));
            proto_entries.push(("constructor", sym_ctor_handle));
            let sym_proto = make_object(gc, &proto_entries);
            // Symbol.prototype[@@toPrimitive] and Symbol.prototype[@@toStringTag]
            unsafe {
                let proto_ptr = sym_proto.heap_ptr().unwrap() as *mut JSObject;
                if let Some(h) = find_handle(&self.builtins, "Symbol_prototype_toPrimitive") {
                    JSObject::add_property(
                        proto_ptr,
                        PropertyKey::from_symbol(rune_core::symbol::SYM_TO_PRIMITIVE),
                        "\u{0}".to_string(),
                        h,
                    );
                }
                let tag_str = HeapString::allocate(gc, "Symbol") as *mut u8;
                JSObject::add_property(
                    proto_ptr,
                    PropertyKey::from_symbol(rune_core::symbol::SYM_TO_STRING_TAG),
                    "\u{0}".to_string(),
                    Value::from_heap_ptr(tag_str),
                );
            }
            self.symbol_prototype = sym_proto;
            let mut ctor_entries: Vec<(&str, Value)> = vec![
                ("prototype", sym_proto),
                ("iterator", Value::symbol(rune_core::symbol::SYM_ITERATOR)),
                ("match", Value::symbol(rune_core::symbol::SYM_MATCH)),
                ("replace", Value::symbol(rune_core::symbol::SYM_REPLACE)),
                ("search", Value::symbol(rune_core::symbol::SYM_SEARCH)),
                ("split", Value::symbol(rune_core::symbol::SYM_SPLIT)),
                (
                    "toPrimitive",
                    Value::symbol(rune_core::symbol::SYM_TO_PRIMITIVE),
                ),
                (
                    "hasInstance",
                    Value::symbol(rune_core::symbol::SYM_HAS_INSTANCE),
                ),
                (
                    "toStringTag",
                    Value::symbol(rune_core::symbol::SYM_TO_STRING_TAG),
                ),
                ("species", Value::symbol(rune_core::symbol::SYM_SPECIES)),
                (
                    "isConcatSpreadable",
                    Value::symbol(rune_core::symbol::SYM_IS_CONCAT_SPREADABLE),
                ),
                (
                    "unscopables",
                    Value::symbol(rune_core::symbol::SYM_UNSCOPABLES),
                ),
                ("matchAll", Value::symbol(rune_core::symbol::SYM_MATCH_ALL)),
                (
                    "asyncIterator",
                    Value::symbol(rune_core::symbol::SYM_ASYNC_ITERATOR),
                ),
            ];
            if let Some(h) = find_handle(&self.builtins, "Symbol_for") {
                ctor_entries.push(("for", h));
            }
            if let Some(h) = find_handle(&self.builtins, "Symbol_keyFor") {
                ctor_entries.push(("keyFor", h));
            }
            let sym_ctor = make_object(gc, &ctor_entries);
            self.symbol_ctor = sym_ctor;
            self.builtin_wrappers.insert("Symbol".to_string(), sym_ctor);
        }

        // Map / Set constructors and prototypes
        {
            let mut map_proto_entries: Vec<(&str, Value)> = Vec::new();
            for (name, handle) in [
                ("set", "Map_prototype_set"),
                ("get", "Map_prototype_get"),
                ("has", "Map_prototype_has"),
                ("delete", "Map_prototype_delete"),
                ("clear", "Map_prototype_clear"),
                ("forEach", "Map_prototype_forEach"),
                ("entries", "Map_prototype_entries"),
                ("keys", "Map_prototype_keys"),
                ("values", "Map_prototype_values"),
            ] {
                if let Some(h) = find_handle(&self.builtins, handle) {
                    map_proto_entries.push((name, h));
                }
            }
            let map_proto = make_object(gc, &map_proto_entries);
            unsafe {
                let proto_ptr = map_proto.heap_ptr().unwrap() as *mut JSObject;
                if let Some(h) = find_handle(&self.builtins, "Map_prototype_entries") {
                    JSObject::add_property(
                        proto_ptr,
                        PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR),
                        "\u{0}".to_string(),
                        h,
                    );
                }
                let tag_str = HeapString::allocate(gc, "Map") as *mut u8;
                JSObject::add_property(
                    proto_ptr,
                    PropertyKey::from_symbol(rune_core::symbol::SYM_TO_STRING_TAG),
                    "\u{0}".to_string(),
                    Value::from_heap_ptr(tag_str),
                );
            }
            let map_ctor = make_object(gc, &[("prototype", map_proto)]);
            self.builtin_wrappers.insert("Map".to_string(), map_ctor);
            self.map_constructor = map_ctor;
            self.map_prototype = map_proto;

            let mut set_proto_entries: Vec<(&str, Value)> = Vec::new();
            for (name, handle) in [
                ("add", "Set_prototype_add"),
                ("has", "Set_prototype_has"),
                ("delete", "Set_prototype_delete"),
                ("clear", "Set_prototype_clear"),
                ("forEach", "Set_prototype_forEach"),
                ("entries", "Set_prototype_entries"),
                ("keys", "Set_prototype_keys"),
                ("values", "Set_prototype_values"),
            ] {
                if let Some(h) = find_handle(&self.builtins, handle) {
                    set_proto_entries.push((name, h));
                }
            }
            let set_proto = make_object(gc, &set_proto_entries);
            unsafe {
                let proto_ptr = set_proto.heap_ptr().unwrap() as *mut JSObject;
                if let Some(h) = find_handle(&self.builtins, "Set_prototype_values") {
                    JSObject::add_property(
                        proto_ptr,
                        PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR),
                        "\u{0}".to_string(),
                        h,
                    );
                }
                let tag_str = HeapString::allocate(gc, "Set") as *mut u8;
                JSObject::add_property(
                    proto_ptr,
                    PropertyKey::from_symbol(rune_core::symbol::SYM_TO_STRING_TAG),
                    "\u{0}".to_string(),
                    Value::from_heap_ptr(tag_str),
                );
            }
            let set_ctor = make_object(gc, &[("prototype", set_proto)]);
            self.builtin_wrappers.insert("Set".to_string(), set_ctor);
            self.set_constructor = set_ctor;
            self.set_prototype = set_proto;
        }

        // Date constructor and prototype
        if find_handle(&self.builtins, "Date").is_some() {
            let mut date_proto_entries: Vec<(&str, Value)> = Vec::new();
            for (name, handle) in [
                ("getDate", "Date_prototype_getDate"),
                ("getDay", "Date_prototype_getDay"),
                ("getFullYear", "Date_prototype_getFullYear"),
                ("getHours", "Date_prototype_getHours"),
                ("getMilliseconds", "Date_prototype_getMilliseconds"),
                ("getMinutes", "Date_prototype_getMinutes"),
                ("getMonth", "Date_prototype_getMonth"),
                ("getSeconds", "Date_prototype_getSeconds"),
                ("getTime", "Date_prototype_getTime"),
                ("getTimezoneOffset", "Date_prototype_getTimezoneOffset"),
                ("getUTCDate", "Date_prototype_getUTCDate"),
                ("getUTCDay", "Date_prototype_getUTCDay"),
                ("getUTCFullYear", "Date_prototype_getUTCFullYear"),
                ("getUTCHours", "Date_prototype_getUTCHours"),
                ("getUTCMilliseconds", "Date_prototype_getUTCMilliseconds"),
                ("getUTCMinutes", "Date_prototype_getUTCMinutes"),
                ("getUTCMonth", "Date_prototype_getUTCMonth"),
                ("getUTCSeconds", "Date_prototype_getUTCSeconds"),
                ("setDate", "Date_prototype_setDate"),
                ("setFullYear", "Date_prototype_setFullYear"),
                ("setHours", "Date_prototype_setHours"),
                ("setMilliseconds", "Date_prototype_setMilliseconds"),
                ("setMinutes", "Date_prototype_setMinutes"),
                ("setMonth", "Date_prototype_setMonth"),
                ("setSeconds", "Date_prototype_setSeconds"),
                ("setTime", "Date_prototype_setTime"),
                ("setUTCDate", "Date_prototype_setUTCDate"),
                ("setUTCFullYear", "Date_prototype_setUTCFullYear"),
                ("setUTCHours", "Date_prototype_setUTCHours"),
                ("setUTCMilliseconds", "Date_prototype_setUTCMilliseconds"),
                ("setUTCMinutes", "Date_prototype_setUTCMinutes"),
                ("setUTCMonth", "Date_prototype_setUTCMonth"),
                ("setUTCSeconds", "Date_prototype_setUTCSeconds"),
                ("toDateString", "Date_prototype_toDateString"),
                ("toISOString", "Date_prototype_toISOString"),
                ("toJSON", "Date_prototype_toJSON"),
                ("toLocaleDateString", "Date_prototype_toLocaleDateString"),
                ("toLocaleString", "Date_prototype_toLocaleString"),
                ("toLocaleTimeString", "Date_prototype_toLocaleTimeString"),
                ("toString", "Date_prototype_toString"),
                ("toTimeString", "Date_prototype_toTimeString"),
                ("toUTCString", "Date_prototype_toUTCString"),
                ("valueOf", "Date_prototype_valueOf"),
            ] {
                if let Some(h) = find_handle(&self.builtins, handle) {
                    date_proto_entries.push((name, h));
                }
            }
            let date_ctor_handle =
                find_handle(&self.builtins, "Date").unwrap_or(Value::undefined());
            date_proto_entries.push(("constructor", date_ctor_handle));
            let date_proto = make_object(gc, &date_proto_entries);
            let mut date_ctor_entries: Vec<(&str, Value)> = vec![("prototype", date_proto)];
            for (name, handle) in [
                ("now", "Date_now"),
                ("parse", "Date_parse"),
                ("UTC", "Date_UTC"),
            ] {
                if let Some(h) = find_handle(&self.builtins, handle) {
                    date_ctor_entries.push((name, h));
                }
            }
            let date_ctor = make_object(gc, &date_ctor_entries);
            self.builtin_wrappers.insert("Date".to_string(), date_ctor);
            self.date_constructor = date_ctor;
            self.date_prototype = date_proto;
        }

        // ArrayBuffer constructor and prototype
        if find_handle(&self.builtins, "ArrayBuffer").is_some() {
            let mut ab_proto_entries: Vec<(&str, Value)> = Vec::new();
            if let Some(h) = find_handle(&self.builtins, "ArrayBuffer_prototype_slice") {
                ab_proto_entries.push(("slice", h));
            }
            let tag_str = HeapString::allocate(gc, "ArrayBuffer") as *mut u8;
            let ab_proto = make_object(gc, &ab_proto_entries);
            unsafe {
                let proto_ptr = ab_proto.heap_ptr().unwrap() as *mut JSObject;
                JSObject::add_property(
                    proto_ptr,
                    PropertyKey::from_symbol(rune_core::symbol::SYM_TO_STRING_TAG),
                    "\u{0}".to_string(),
                    Value::from_heap_ptr(tag_str),
                );
            }
            let ab_ctor = make_object(gc, &[("prototype", ab_proto)]);
            self.builtin_wrappers
                .insert("ArrayBuffer".to_string(), ab_ctor);
            self.array_buffer_constructor = ab_ctor;
            self.array_buffer_prototype = ab_proto;
            if let Some(h) = find_handle(&self.builtins, "ArrayBuffer_isView") {
                unsafe {
                    let ctor_ptr = ab_ctor.heap_ptr().unwrap() as *mut JSObject;
                    let shape = JSObject::shape_ptr(ctor_ptr);
                    if let Some(slot) = shape.lookup(&PropertyKey::from_string("isView")) {
                        JSObject::set_slot(ctor_ptr, slot, h);
                    }
                }
            }
        }

        // TypedArray constructors (one per element type) + %TypedArray.prototype%
        if find_handle(&self.builtins, "Uint8Array").is_some() {
            // Shared %TypedArray.prototype% — all methods live here.
            let mut ta_base_entries: Vec<(&str, Value)> = Vec::new();
            for (name, handle) in [
                ("set", "TypedArray_prototype_set"),
                ("subarray", "TypedArray_prototype_subarray"),
                ("fill", "TypedArray_prototype_fill"),
                ("at", "TypedArray_prototype_at"),
                ("indexOf", "TypedArray_prototype_indexOf"),
                ("includes", "TypedArray_prototype_includes"),
                ("slice", "TypedArray_prototype_slice"),
                ("values", "TypedArray_prototype_values"),
                ("keys", "TypedArray_prototype_keys"),
                ("entries", "TypedArray_prototype_entries"),
            ] {
                if let Some(h) = find_handle(&self.builtins, handle) {
                    ta_base_entries.push((name, h));
                }
            }
            let ta_base = make_object(gc, &ta_base_entries);
            unsafe {
                let base_ptr = ta_base.heap_ptr().unwrap() as *mut JSObject;
                if let Some(h) = find_handle(&self.builtins, "TypedArray_prototype_values") {
                    JSObject::add_property(
                        base_ptr,
                        PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR),
                        "\u{0}".to_string(),
                        h,
                    );
                }
                // toString → Array.prototype.toString (join with commas).
                if let Some(h) = find_handle(&self.builtins, "Array_prototype_toString") {
                    JSObject::add_property(
                        base_ptr,
                        PropertyKey::from_string("toString"),
                        "toString".to_string(),
                        h,
                    );
                }
            }
            let base_ptr = ta_base.heap_ptr().unwrap();
            let base_obj = base_ptr as *mut JSObject;
            for i in 0..typedarray::NUM_KINDS {
                let kind = typedarray::TypedArrayKind::from_index(i);
                let ctor_handle = find_handle(&self.builtins, kind.name());
                let Some(ctor_handle) = ctor_handle else {
                    continue;
                };
                // Per-type prototype: BYTES_PER_ELEMENT + constructor + toStringTag.
                let tag_str = HeapString::allocate(gc, kind.name()) as *mut u8;
                let mut proto_entries: Vec<(&str, Value)> =
                    vec![("BYTES_PER_ELEMENT", Value::smi(kind.element_size() as i32))];
                proto_entries.push(("constructor", ctor_handle));
                let proto = make_object(gc, &proto_entries);
                unsafe {
                    let proto_ptr = proto.heap_ptr().unwrap() as *mut JSObject;
                    JSObject::add_property(
                        proto_ptr,
                        PropertyKey::from_symbol(rune_core::symbol::SYM_TO_STRING_TAG),
                        "\u{0}".to_string(),
                        Value::from_heap_ptr(tag_str),
                    );
                    JSObject::set_prototype(proto_ptr, base_ptr);
                }
                let ctor_obj = make_object(
                    gc,
                    &[
                        ("prototype", proto),
                        ("BYTES_PER_ELEMENT", Value::smi(kind.element_size() as i32)),
                    ],
                );
                self.builtin_wrappers
                    .insert(kind.name().to_string(), ctor_obj);
                self.typed_array_ctors.push(ctor_obj);
                self.typed_array_protos.push(proto);
                self.typed_array_ctor_handles.push(ctor_handle);
            }
            let _ = base_obj;
        }

        // Iteration protocol — Array.prototype.values/keys/entries/[Symbol.iterator]
        // and String.prototype[Symbol.iterator]. Iterator state is stored under a
        // hidden symbol (excluded from enumeration) on each iterator object.
        {
            self.iter_state_symbol = rune_core::symbol::symbol_for("__rune_iter_state");
            self.gen_state_symbol = rune_core::symbol::symbol_for("__rune_gen");
            self.done_key = Value::from_heap_ptr(HeapString::allocate(gc, "done") as *mut u8);
            self.value_key = Value::from_heap_ptr(HeapString::allocate(gc, "value") as *mut u8);
            self.next_key = Value::from_heap_ptr(HeapString::allocate(gc, "next") as *mut u8);
            if self.array_prototype.is_heap_object() {
                unsafe {
                    let proto_ptr = self.array_prototype.heap_ptr().unwrap() as *mut JSObject;
                    if let Some(h) = find_handle(&self.builtins, "Array_prototype_values") {
                        JSObject::add_property(
                            proto_ptr,
                            PropertyKey::from_string("values"),
                            "values".to_string(),
                            h,
                        );
                    }
                    if let Some(h) = find_handle(&self.builtins, "Array_prototype_keys") {
                        JSObject::add_property(
                            proto_ptr,
                            PropertyKey::from_string("keys"),
                            "keys".to_string(),
                            h,
                        );
                    }
                    if let Some(h) = find_handle(&self.builtins, "Array_prototype_entries") {
                        JSObject::add_property(
                            proto_ptr,
                            PropertyKey::from_string("entries"),
                            "entries".to_string(),
                            h,
                        );
                    }
                    if let Some(h) = find_handle(&self.builtins, "Array_prototype_iterator") {
                        JSObject::add_property(
                            proto_ptr,
                            PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR),
                            "\u{0}".to_string(),
                            h,
                        );
                    }
                }
            }
            if self.string_prototype.is_heap_object() {
                unsafe {
                    let proto_ptr = self.string_prototype.heap_ptr().unwrap() as *mut JSObject;
                    if let Some(h) = find_handle(&self.builtins, "String_prototype_iterator") {
                        JSObject::add_property(
                            proto_ptr,
                            PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR),
                            "\u{0}".to_string(),
                            h,
                        );
                    }
                }
            }
        }

        // Promise constructor — resolve/reject bridge program (lazy init)
        {
            let bridge_inner = BytecodeProgram::new(
                vec![
                    Instruction::new(Opcode::LoadCaptured, vec![0, 0]),
                    Instruction::new(Opcode::LoadCaptured, vec![0, 1]),
                    Instruction::new(Opcode::LoadLocal, vec![0]),
                    Instruction::new(Opcode::Call, vec![1]),
                    Instruction::new(Opcode::Return, vec![]),
                ],
                vec![],
                vec![],
            );
            let bridge_prog = Box::new(BytecodeProgram::new(vec![], vec![], vec![bridge_inner]));
            self.promise_bridge_prog = Box::leak(bridge_prog) as *const BytecodeProgram;
        }
        if find_handle(&self.builtins, "Promise").is_some() {
            let tf = find_handle(&self.builtins, "Promise_prototype_then");
            let cf = find_handle(&self.builtins, "Promise_prototype_catch");
            let mut proto_entries: Vec<(&str, Value)> = Vec::new();
            if let Some(then_h) = tf {
                proto_entries.push(("then", then_h));
            }
            if let Some(catch_h) = cf {
                proto_entries.push(("catch", catch_h));
            }
            if let Some(fin_h) = find_handle(&self.builtins, "Promise_prototype_finally") {
                proto_entries.push(("finally", fin_h));
            }
            let proto_obj = make_object(gc, &proto_entries);
            self.promise_prototype = proto_obj;
            let mut ctor_entries: Vec<(&str, Value)> = vec![("prototype", proto_obj)];
            if let Some(r) = find_handle(&self.builtins, "Promise_resolve") {
                ctor_entries.push(("resolve", r));
            }
            if let Some(r) = find_handle(&self.builtins, "Promise_reject") {
                ctor_entries.push(("reject", r));
            }
            if let Some(r) = find_handle(&self.builtins, "Promise_all") {
                ctor_entries.push(("all", r));
            }
            if let Some(r) = find_handle(&self.builtins, "Promise_race") {
                ctor_entries.push(("race", r));
            }
            let prom_ctor = make_object(gc, &ctor_entries);
            self.promise_constructor = prom_ctor;
            self.builtin_wrappers
                .insert("Promise".to_string(), prom_ctor);
        }

        // RegExp namespace — prototype with exec/test/source/flags/lastIndex
        {
            let exec_h = find_handle(&self.builtins, "RegExp_prototype_exec");
            let test_h = find_handle(&self.builtins, "RegExp_prototype_test");
            let source_h = find_handle(&self.builtins, "RegExp_prototype_source");
            let flags_h = find_handle(&self.builtins, "RegExp_prototype_flags");
            let li_h = find_handle(&self.builtins, "RegExp_prototype_lastIndex");
            let mut proto_entries: Vec<(&str, Value)> = Vec::new();
            if let Some(h) = exec_h {
                proto_entries.push(("exec", h));
            }
            if let Some(h) = test_h {
                proto_entries.push(("test", h));
            }
            if let Some(h) = source_h {
                proto_entries.push(("source", h));
            }
            if let Some(h) = flags_h {
                proto_entries.push(("flags", h));
            }
            if let Some(h) = li_h {
                proto_entries.push(("lastIndex", h));
            }
            let re_ctor_handle =
                find_handle(&self.builtins, "RegExp").unwrap_or(Value::undefined());
            proto_entries.push(("constructor", re_ctor_handle));
            let re_proto = make_object(gc, &proto_entries);
            self.regexp_prototype = re_proto;
            let re_ctor = make_object(gc, &[("prototype", re_proto)]);
            self.builtin_wrappers.insert("RegExp".to_string(), re_ctor);
            self.regexp_constructor = re_ctor;
        }

        // Math namespace with all methods + constants
        let pi_val = Value::from_float64(std::f64::consts::PI);
        let e_val = Value::from_float64(std::f64::consts::E);
        let math_entries: Vec<(&str, Value)> = [
            ("floor", find_handle(&self.builtins, "Math_floor")),
            ("ceil", find_handle(&self.builtins, "Math_ceil")),
            ("abs", find_handle(&self.builtins, "Math_abs")),
            ("min", find_handle(&self.builtins, "Math_min")),
            ("max", find_handle(&self.builtins, "Math_max")),
            ("pow", find_handle(&self.builtins, "Math_pow")),
            ("sqrt", find_handle(&self.builtins, "Math_sqrt")),
            ("round", find_handle(&self.builtins, "Math_round")),
            ("trunc", find_handle(&self.builtins, "Math_trunc")),
            ("sign", find_handle(&self.builtins, "Math_sign")),
            ("hypot", find_handle(&self.builtins, "Math_hypot")),
            ("clz32", find_handle(&self.builtins, "Math_clz32")),
            ("imul", find_handle(&self.builtins, "Math_imul")),
            ("cbrt", find_handle(&self.builtins, "Math_cbrt")),
            ("log", find_handle(&self.builtins, "Math_log")),
            ("log2", find_handle(&self.builtins, "Math_log2")),
            ("log10", find_handle(&self.builtins, "Math_log10")),
            ("exp", find_handle(&self.builtins, "Math_exp")),
            ("sin", find_handle(&self.builtins, "Math_sin")),
            ("cos", find_handle(&self.builtins, "Math_cos")),
            ("tan", find_handle(&self.builtins, "Math_tan")),
            ("asin", find_handle(&self.builtins, "Math_asin")),
            ("acos", find_handle(&self.builtins, "Math_acos")),
            ("atan", find_handle(&self.builtins, "Math_atan")),
            ("atan2", find_handle(&self.builtins, "Math_atan2")),
            // §21.3.1.1-8 constants
            ("PI", Some(pi_val)),
            ("E", Some(e_val)),
            ("LN2", Some(Value::from_float64(std::f64::consts::LN_2))),
            ("LN10", Some(Value::from_float64(std::f64::consts::LN_10))),
            ("LOG2E", Some(Value::from_float64(std::f64::consts::LOG2_E))),
            (
                "LOG10E",
                Some(Value::from_float64(1.0 / std::f64::consts::LN_10)),
            ),
            ("SQRT2", Some(Value::from_float64(std::f64::consts::SQRT_2))),
            (
                "SQRT1_2",
                Some(Value::from_float64(std::f64::consts::FRAC_1_SQRT_2)),
            ),
        ]
        .iter()
        .filter_map(|(name, val)| val.map(|v| (*name, v)))
        .collect();
        if !math_entries.is_empty() {
            let math_obj = make_object(gc, &math_entries);
            self.builtin_wrappers.insert("Math".to_string(), math_obj);
        }

        // JSON namespace with .parse() and .stringify()
        if let Some(parse_handle) = find_handle(&self.builtins, "JSON_parse") {
            let mut json_entries: Vec<(&str, Value)> = vec![("parse", parse_handle)];
            if let Some(stringify_handle) = find_handle(&self.builtins, "JSON_stringify") {
                json_entries.push(("stringify", stringify_handle));
            }
            let json_obj = make_object(gc, &json_entries);
            self.builtin_wrappers.insert("JSON".to_string(), json_obj);
        }

        // Function.prototype with .call() method; its constructor is the
        // %Function% wrapper so `ctor.constructor` chains resolve (spec:
        // %Error%.[[Prototype]] is %Function.prototype%).
        if let Some(call_handle) = find_handle(&self.builtins, "Function_prototype_call") {
            let mut proto_entries_f: Vec<(&str, Value)> = vec![("call", call_handle)];
            if let Some(apply_handle) = find_handle(&self.builtins, "Function_prototype_apply") {
                proto_entries_f.push(("apply", apply_handle));
            }
            let func_proto = make_object(gc, &proto_entries_f);
            self.function_prototype = func_proto;
            // %Function% wrapper
            let fn_name = HeapString::allocate(gc, "Function") as *mut u8;
            let fn_ctor = make_object(
                gc,
                &[
                    ("prototype", func_proto),
                    ("length", Value::smi(1)),
                    ("name", Value::from_heap_ptr(fn_name)),
                ],
            );
            unsafe {
                JSObject::add_property(
                    func_proto.heap_ptr().unwrap() as *mut JSObject,
                    PropertyKey::from_string("constructor"),
                    "constructor".to_string(),
                    fn_ctor,
                );
            }
            self.builtin_wrappers
                .insert("Function".to_string(), fn_ctor);
        }

        // Object.prototype — an empty object that serves as default [[Prototype]]
        let obj_proto_shape = Shape::empty();
        let obj_proto_ptr = JSObject::allocate(gc, obj_proto_shape, &[]);
        self.object_prototype = Value::from_heap_ptr(obj_proto_ptr as *mut u8);
        // Object.prototype methods: toString / hasOwnProperty / isPrototypeOf
        {
            let to_string_handle = find_handle(&self.builtins, "Object_prototype_toString");
            let has_own_handle = find_handle(&self.builtins, "Object_prototype_hasOwnProperty");
            let is_proto_handle = find_handle(&self.builtins, "Object_prototype_isPrototypeOf");
            let obj_proto_ptr = self.object_prototype.heap_ptr().unwrap() as *mut JSObject;
            if let Some(h) = to_string_handle {
                unsafe {
                    JSObject::add_property(
                        obj_proto_ptr,
                        PropertyKey::from_string("toString"),
                        "toString".to_string(),
                        h,
                    );
                }
            }
            if let Some(h) = has_own_handle {
                unsafe {
                    JSObject::add_property(
                        obj_proto_ptr,
                        PropertyKey::from_string("hasOwnProperty"),
                        "hasOwnProperty".to_string(),
                        h,
                    );
                }
            }
            if let Some(h) = is_proto_handle {
                unsafe {
                    JSObject::add_property(
                        obj_proto_ptr,
                        PropertyKey::from_string("isPrototypeOf"),
                        "isPrototypeOf".to_string(),
                        h,
                    );
                }
            }
            if let Some(h) = find_handle(&self.builtins, "Object_prototype_propertyIsEnumerable") {
                unsafe {
                    JSObject::add_property(
                        obj_proto_ptr,
                        PropertyKey::from_string("propertyIsEnumerable"),
                        "propertyIsEnumerable".to_string(),
                        h,
                    );
                }
            }
            if let Some(h) = find_handle(&self.builtins, "Object_prototype_valueOf") {
                unsafe {
                    JSObject::add_property(
                        obj_proto_ptr,
                        PropertyKey::from_string("valueOf"),
                        "valueOf".to_string(),
                        h,
                    );
                }
            }
        }

        // B1e: Array.prototype's [[Prototype]] is Object.prototype (§23.1.3).
        // Without this, arrays cannot reach hasOwnProperty/toString/valueOf
        // (observable in sort's precise-prototype tests and everywhere else).
        if self.array_prototype.is_heap_object() {
            unsafe {
                JSObject::set_prototype(
                    self.array_prototype.heap_ptr().unwrap() as *mut JSObject,
                    self.object_prototype.heap_ptr().unwrap(),
                );
            }
        }

        // Error family — Error, EvalError, RangeError, ReferenceError,
        // SyntaxError, TypeError, URIError. Each ctor wrapper exposes
        // .prototype/.length/.name; each prototype chains to Error.prototype,
        // which chains to Object.prototype. Instances get their [[Prototype]]
        // from the wrapper's "prototype" property via the New/Call arms.
        {
            let names = crate::builtins::ERROR_TYPE_NAMES;
            let to_string_handle = find_handle(&self.builtins, "Error_prototype_toString");
            // Error.prototype
            let mut err_proto_pairs: Vec<(&str, Value)> = vec![
                (
                    "name",
                    Value::from_heap_ptr(HeapString::allocate(gc, "Error") as *mut u8),
                ),
                (
                    "message",
                    Value::from_heap_ptr(HeapString::allocate(gc, "") as *mut u8),
                ),
            ];
            if let Some(h) = to_string_handle {
                err_proto_pairs.push(("toString", h));
            }
            let err_proto = make_object(gc, &err_proto_pairs);
            unsafe {
                JSObject::set_prototype(
                    err_proto.heap_ptr().unwrap() as *mut JSObject,
                    self.object_prototype.heap_ptr().unwrap(),
                );
            }
            self.error_protos.push(err_proto);
            // Error wrapper
            let err_name = Value::from_heap_ptr(HeapString::allocate(gc, "Error") as *mut u8);
            let err_ctor = make_object(
                gc,
                &[
                    ("prototype", err_proto),
                    ("length", Value::smi(1)),
                    ("name", err_name),
                ],
            );
            self.error_ctors.push(err_ctor);
            self.builtin_wrappers.insert("Error".to_string(), err_ctor);
            unsafe {
                JSObject::add_property(
                    err_proto.heap_ptr().unwrap() as *mut JSObject,
                    PropertyKey::from_string("constructor"),
                    "constructor".to_string(),
                    err_ctor,
                );
            }
            if let Some(h) = find_handle(&self.builtins, "isError") {
                unsafe {
                    JSObject::add_property(
                        err_ctor.heap_ptr().unwrap() as *mut JSObject,
                        PropertyKey::from_string("isError"),
                        "isError".to_string(),
                        h,
                    );
                }
            }
            // Native error types (skip Error at index 0)
            for (i, name) in names.iter().enumerate().skip(1) {
                let proto_pairs: Vec<(&str, Value)> = vec![
                    (
                        "name",
                        Value::from_heap_ptr(HeapString::allocate(gc, name) as *mut u8),
                    ),
                    (
                        "message",
                        Value::from_heap_ptr(HeapString::allocate(gc, "") as *mut u8),
                    ),
                ];
                let proto = make_object(gc, &proto_pairs);
                unsafe {
                    JSObject::set_prototype(
                        proto.heap_ptr().unwrap() as *mut JSObject,
                        err_proto.heap_ptr().unwrap(),
                    );
                }
                self.error_protos.push(proto);
                let ctor_name = Value::from_heap_ptr(HeapString::allocate(gc, name) as *mut u8);
                let ctor = make_object(
                    gc,
                    &[
                        ("prototype", proto),
                        ("length", Value::smi(1)),
                        ("name", ctor_name),
                    ],
                );
                self.error_ctors.push(ctor);
                self.builtin_wrappers.insert(name.to_string(), ctor);
                unsafe {
                    JSObject::set_prototype(
                        ctor.heap_ptr().unwrap() as *mut JSObject,
                        err_ctor.heap_ptr().unwrap(),
                    );
                }
                unsafe {
                    JSObject::add_property(
                        proto.heap_ptr().unwrap() as *mut JSObject,
                        PropertyKey::from_string("constructor"),
                        "constructor".to_string(),
                        ctor,
                    );
                }
                let _ = i;
            }
        }

        // Wire the callable wrapper set: every builtin constructor wrapper
        // gets [[Prototype]] = %Function.prototype% and joins
        // `callable_wrappers` (drives `typeof` → "function" and
        // `Object.prototype.toString` → "[object Function]").
        if let Some(fp) = self.function_prototype.heap_ptr() {
            // %Object% needs a "prototype" property (created earlier without
            // one — object_prototype didn't exist yet).
            if let Some(obj_wrapper) = self.builtin_wrappers.get("Object").copied() {
                if let Some(ptr) = obj_wrapper.heap_ptr() {
                    unsafe {
                        let shape = JSObject::shape_ptr(ptr as *mut JSObject);
                        if shape.lookup(&PROTOTYPE_KEY).is_none() {
                            JSObject::add_property(
                                ptr as *mut JSObject,
                                *PROTOTYPE_KEY,
                                "prototype".to_string(),
                                self.object_prototype,
                            );
                        }
                        // Object.getPrototypeOf (constructor static).
                        if let Some(h) = find_handle(&self.builtins, "Object_getPrototypeOf") {
                            if shape
                                .lookup(&PropertyKey::from_string("getPrototypeOf"))
                                .is_none()
                            {
                                JSObject::add_property(
                                    ptr as *mut JSObject,
                                    PropertyKey::from_string("getPrototypeOf"),
                                    "getPrototypeOf".to_string(),
                                    h,
                                );
                            }
                        }
                    }
                }
            }
            // %Function.prototype%.[[Prototype]] = %Object.prototype%, and
            // %Function%.[[Prototype]] = %Function.prototype% (self-chain).
            unsafe {
                JSObject::set_prototype(
                    fp as *mut JSObject,
                    self.object_prototype
                        .heap_ptr()
                        .unwrap_or(std::ptr::null_mut()),
                );
            }
            if let Some(fn_ctor) = self.builtin_wrappers.get("Function").copied() {
                if let Some(ptr) = fn_ctor.heap_ptr() {
                    unsafe {
                        JSObject::set_prototype(ptr as *mut JSObject, fp);
                    }
                }
            }
            let mut wrappers: Vec<Value> = Vec::new();
            wrappers.extend(self.error_ctors.iter().copied());
            if let Some(v) = self.string_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.number_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.symbol_ctor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.promise_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.map_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.set_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.date_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.array_buffer_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            if let Some(v) = self.regexp_constructor.heap_ptr() {
                wrappers.push(Value::from_heap_ptr(v));
            }
            wrappers.extend(self.typed_array_ctors.iter().copied());
            for w in &wrappers {
                if let Some(ptr) = w.heap_ptr() {
                    // Native error constructors inherit from %Error%, not
                    // %Function.prototype% (their proto was set in the error
                    // family block above).
                    if self
                        .error_ctors
                        .iter()
                        .skip(1)
                        .any(|e| e.heap_ptr() == Some(ptr))
                    {
                        continue;
                    }
                    unsafe {
                        JSObject::set_prototype(ptr as *mut JSObject, fp);
                    }
                }
            }
            self.callable_wrappers = wrappers;
        }

        // Global constants: NaN, Infinity, undefined
        let nan_val = Value::from_float64(f64::NAN);
        self.globals.insert("NaN".to_string(), nan_val);
        let inf_val = Value::from_float64(f64::INFINITY);
        self.globals.insert("Infinity".to_string(), inf_val);
        self.globals
            .insert("undefined".to_string(), Value::undefined());

        // assert wrapper object for Test262: assert.sameValue, assert.notSameValue, assert.throws
        let assert_same = find_handle(&self.builtins, "assert_sameValue");
        let assert_not_same = find_handle(&self.builtins, "assert_notSameValue");
        let assert_throws = find_handle(&self.builtins, "assert_throws");
        let assert_cmp = find_handle(&self.builtins, "assert_compareArray");
        if let (Some(same), Some(not_same), Some(th)) =
            (assert_same, assert_not_same, assert_throws)
        {
            let mut pairs: Vec<(&str, Value)> = vec![
                ("sameValue", same),
                ("notSameValue", not_same),
                ("throws", th),
            ];
            if let Some(ca) = assert_cmp {
                pairs.push(("compareArray", ca));
            }
            let assert_obj = make_object(gc, &pairs);
            self.builtin_wrappers
                .insert("assert".to_string(), assert_obj);
        }
    }

    /// Register a built-in function and return its handle (negative Smi).
    pub fn register_builtin(&mut self, name: &'static str, func: BuiltinFn) -> Value {
        let id = self.builtins.len();
        self.builtins.push(Builtin {
            name,
            length: 1,
            func,
        });
        Value::smi(-(id as i32) - 1)
    }

    /// Look up a builtin handle by name.
    pub fn get_builtin(&self, name: &str) -> Option<Value> {
        self.builtins
            .iter()
            .position(|b| b.name == name)
            .map(|id| Value::smi(-(id as i32) - 1))
    }

    /// Create a resolve/reject bridge function that closes over a promise
    /// and a builtin handle. Returns a callable TAG_FUNC Value.
    /// Find a builtin handle by name, returning a negative Smi handle or undefined.
    pub fn find_builtin_handle(&self, name: &str) -> Value {
        self.builtins
            .iter()
            .position(|b| b.name == name)
            .map(|id| Value::smi(-(id as i32) - 1))
            .unwrap_or(Value::undefined())
    }

    /// Create a bridge function for async generator resume/reject.
    /// The bridge calls `builtin(this=gen_id_smi, args=[value])` via the bridge_inner program.
    pub fn create_async_bridge(
        &mut self,
        gc: &mut SemiSpace,
        gen_id: usize,
        handle: Value,
    ) -> Value {
        let env = EnvObject::allocate(gc, 2, std::ptr::null_mut()) as *mut u8;
        unsafe {
            let resolved_env = if (*(env as *const GcHeader)).is_forwarded() {
                (*(env as *const GcHeader)).forwarding_addr() as *mut EnvObject
            } else {
                env as *mut EnvObject
            };
            EnvObject::set_slot(resolved_env, 0, Value::smi(gen_id as i32));
            EnvObject::set_slot(resolved_env, 1, handle);
            let func = Func::allocate(
                gc,
                0,
                self.promise_bridge_prog as *const u8,
                false,
                resolved_env as *mut u8,
            );
            let resolved_func = if (*(func as *const GcHeader)).is_forwarded() {
                (*(func as *const GcHeader)).forwarding_addr() as *mut Func
            } else {
                func
            };
            Value::from_heap_ptr(resolved_func as *mut u8)
        }
    }

    pub fn create_promise_bridge(
        &self,
        gc: &mut SemiSpace,
        promise: Value,
        builtin_handle: Value,
    ) -> Value {
        unsafe {
            let env = EnvObject::allocate(gc, 2, std::ptr::null_mut());
            let func = Func::allocate(
                gc,
                0,
                self.promise_bridge_prog as *const u8,
                false,
                env as *mut u8,
            );
            let env_ptr = if (*(env as *const GcHeader)).is_forwarded() {
                (*(env as *const GcHeader)).forwarding_addr() as *mut EnvObject
            } else {
                env
            };
            EnvObject::set_slot(env_ptr, 0, promise);
            EnvObject::set_slot(env_ptr, 1, builtin_handle);
            let func_ptr = if (*(func as *const GcHeader)).is_forwarded() {
                (*(func as *const GcHeader)).forwarding_addr() as *mut Func
            } else {
                func
            };
            Func::set_env_ptr(func_ptr, env_ptr as *mut u8);
            Value::from_heap_ptr(func_ptr as *mut u8)
        }
    }

    /// Check if all values in the slice are Smi (tag bit 0 = 1).
    #[allow(dead_code)]
    fn all_smi(values: &[Value]) -> bool {
        values.iter().all(|v| v.is_smi())
    }

    /// Set a pending exception (used by builtins that cannot return Exit).
    pub fn set_pending_exception(&mut self, val: Value) {
        self.pending_exception = Some(val);
    }

    /// Enqueue a microtask to be executed after the current synchronous task.
    pub(crate) fn enqueue_microtask(
        &mut self,
        callback: Value,
        args: Vec<Value>,
        ppc: Option<PendingPromiseCtor>,
    ) {
        self.microtask_queue.push(Microtask {
            callback,
            args,
            promise_ctor: ppc,
        });
    }

    /// Drain all enqueued microtasks. Each microtask is executed synchronously
    /// via push_callback_call. New microtasks enqueued during draining are
    /// processed in the current batch.
    pub(crate) fn drain_microtask_queue(&mut self, gc: &mut SemiSpace) {
        while !self.microtask_queue.is_empty() {
            let tasks: Vec<Microtask> = std::mem::take(&mut self.microtask_queue);
            for task in tasks {
                self.pending_promise_ctor = task.promise_ctor;
                self.push_callback_call(gc, task.callback, Value::undefined(), task.args);
                self.register_roots(gc);
                let _ = self.run_loop(gc);
            }
        }
    }

    /// Throw a ReferenceError from the run loop (A2: real error object via
    /// the factory, routed through handle_throw so in-frame try/catch +
    /// assert.throws observe it). Returns `None` when a handler took over
    /// (caller must `continue`), `Some(Exit)` when it propagates out.
    fn throw_reference_error(&mut self, gc: &mut SemiSpace, msg: &str) -> Option<Exit> {
        let val = crate::errors::error_object(
            gc,
            &self.error_protos,
            crate::errors::ErrorKind::ReferenceError,
            msg,
        );
        self.handle_throw(gc, val)
    }

    /// Throw a TypeError from the run loop (A2: same contract as above).
    fn throw_type_error(&mut self, gc: &mut SemiSpace, msg: &str) -> Option<Exit> {
        let val = crate::errors::error_object(
            gc,
            &self.error_protos,
            crate::errors::ErrorKind::TypeError,
            msg,
        );
        self.handle_throw(gc, val)
    }

    /// Build a TypeError and route it through `handle_throw` so in-frame
    /// try/catch/finally handlers run (notably inside resumed generators,
    /// whose try frames are banked and swapped in on resume). Returns `None`
    /// when a handler took over (caller must `continue`), `Some(Exit)` when
    /// the error propagates out. Outcome is identical to `throw_type_error`
    /// when no handler matches.
    fn throw_routed(&mut self, gc: &mut SemiSpace, msg: &str) -> Option<Exit> {
        let val = crate::errors::error_object(
            gc,
            &self.error_protos,
            crate::errors::ErrorKind::TypeError,
            msg,
        );
        self.handle_throw(gc, val)
    }

    /// Whether the thrown value satisfies an `assert.throws(expected, fn)`
    /// expectation. String expectations compare against the error name;
    /// builtin constructor handles compare against the builtin's name.
    /// Unknown constructor shapes are accepted (no way to verify them yet).
    fn assert_error_matches(&self, expected: Value, thrown: Value) -> bool {
        if let Some(ptr) = expected.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_STRING {
                let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                return crate::builtins::read_error_name(thrown).as_deref() == Some(s.as_str());
            }
            if tag == TAG_OBJECT {
                // Error-family constructor wrapper (TypeError, RangeError, …):
                // the thrown value matches if its prototype chain contains the
                // corresponding error prototype (instanceof semantics), or its
                // name reads back to the same type name.
                if let Some(ti) = self
                    .error_ctors
                    .iter()
                    .position(|c| c.heap_ptr() == Some(ptr))
                {
                    let proto = self.error_protos.get(ti).and_then(|v| v.heap_ptr());
                    if let Some(pp) = proto {
                        if ordinary_has_instance(thrown, pp) {
                            return true;
                        }
                    }
                    return crate::builtins::read_error_name(thrown)
                        .as_deref()
                        .is_some_and(|n| n == self.error_ctor_name(ptr).as_deref().unwrap_or(""));
                }
                return true;
            }
            return true;
        }
        if let Some(smi) = expected.as_smi() {
            if smi < 0 {
                let id = (-smi - 1) as usize;
                if let Some(b) = self.builtins.get(id) {
                    return crate::builtins::read_error_name(thrown).as_deref() == Some(b.name);
                }
            }
        }
        true
    }

    /// Type name of an Error-family constructor wrapper ("TypeError", …),
    /// read from its prototype's `name` property. None for non-error objects.
    fn error_ctor_name(&self, ctor_ptr: *mut u8) -> Option<String> {
        let ti = self
            .error_ctors
            .iter()
            .position(|c| c.heap_ptr() == Some(ctor_ptr))?;
        let proto = self.error_protos.get(ti)?.heap_ptr()?;
        let shape = unsafe { JSObject::shape_ptr(proto as *mut JSObject) };
        let slot = shape.lookup(&PropertyKey::from_string("name"))?;
        let name_val = unsafe { JSObject::get_slot(proto as *mut JSObject, slot) };
        let name_ptr = name_val.heap_ptr()?;
        if unsafe { (*(name_ptr as *const GcHeader)).tag() } != TAG_STRING {
            return None;
        }
        Some(unsafe { HeapString::to_string(name_ptr as *mut HeapString) })
    }

    /// Human-readable name for an `assert.throws` expectation value.
    fn describe_expected_error(&self, expected: Value) -> String {
        if let Some(ptr) = expected.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_STRING {
                return unsafe { HeapString::to_string(ptr as *mut HeapString) };
            }
            if tag == TAG_OBJECT {
                if let Some(name) = self.error_ctor_name(ptr) {
                    return name;
                }
            }
        }
        if let Some(smi) = expected.as_smi() {
            if smi < 0 {
                let id = (-smi - 1) as usize;
                if let Some(b) = self.builtins.get(id) {
                    return b.name.to_string();
                }
            }
        }
        "an error".to_string()
    }

    /// Yield* delegation divert: if the frame being unwound past is a
    /// delegate callback (next/throw/return/getter invoked on a suspended
    /// outer generator's behalf — i.e. a live yield* pending state points
    /// at it), resume the outer generator in Throw mode with the in-flight
    /// error instead of unwinding past it.
    /// Returns `Some(outcome)` when diverted, `None` to unwind normally.
    fn divert_delegate_throw(&mut self, gc: &mut SemiSpace, val: Value) -> Option<DivertOut> {
        let top = self.frames.len().wrapping_sub(1);
        let afr_hit = self
            .pending_yield_star_afr
            .as_ref()
            .is_some_and(|p| p.source_frame_depth == top);
        let nxt_hit = self
            .pending_yield_star_next
            .as_ref()
            .is_some_and(|p| p.source_frame_depth == top && p.gen_id.is_some());
        let get_hit = self
            .pending_yield_star_get
            .as_ref()
            .is_some_and(|p| p.source_frame_depth == top);
        if !(afr_hit || nxt_hit || get_hit) {
            return None;
        }
        // Consume the matched state (only one is ever live).
        let gen_id = if afr_hit {
            let pa = self.pending_yield_star_afr.take().unwrap();
            pa.gen_id
        } else if nxt_hit {
            self.pending_yield_star_next.take().unwrap().gen_id.unwrap()
        } else {
            self.pending_yield_star_get.take().unwrap().gen_id
        };
        // Pop the callback frame (mirror the standard pop bookkeeping).
        let popped_frame = self.frames.len() - 1;
        self.last_locals = self.frames[popped_frame].locals.clone();
        self.frames.pop();
        self.try_stack
            .retain(|tf| tf.frame_depth != popped_frame + 1);
        match self.resume_generator_full(gc, gen_id, GeneratorResume::Throw(val)) {
            Ok((v, done)) => {
                if done {
                    self.generators[gen_id].in_delegate = false;
                }
                let r = crate::builtins::iter_result_with_proto(gc, self, v, done);
                self.push(r);
                let n = self.frames.len();
                self.frames[n - 1].pc += 1;
                Some(DivertOut::Handled)
            }
            Err(e2) => match self.handle_throw(gc, e2) {
                Some(exit) => Some(DivertOut::Throw(exit)),
                None => Some(DivertOut::Handled),
            },
        }
    }

    /// B1f-4: divert a throw escaping a From mapper frame into IteratorClose.
    /// Spec requires calling the iterator's return() before propagating the
    /// abrupt (IfAbruptCloseIterator). Pops the mapper frame, runs the close,
    /// and either waits on a JS return() or rethrows via handle_throw.
    /// Next/factory/done/value/close-frame throws are NOT diverted (raw
    /// propagation); array-like feeds never close.
    fn divert_from_throw(&mut self, gc: &mut SemiSpace, val: Value) -> Option<DivertOut> {
        let top = self.frames.len().wrapping_sub(1);
        let hit = self.pending_from_op.as_ref().is_some_and(|op| {
            op.source_frame_depth == top
                && matches!(op.await_kind, FromAwait::Map)
                && matches!(op.feed, FromFeed::Iter { .. })
        });
        if !hit {
            return None;
        }
        let mut op = self.pending_from_op.take().unwrap();
        // Pop the mapper frame (mirror the standard pop bookkeeping; do NOT
        // drop_dead_from_op — the op stays live in our hands below).
        let popped_frame = self.frames.len() - 1;
        let callee_base = self.frames.last().unwrap().stack_base;
        self.frames.pop();
        self.try_stack
            .retain(|tf| tf.frame_depth != popped_frame + 1);
        self.drop_dead_array_op(popped_frame);
        self.drop_dead_sort_op(popped_frame);
        self.stack.truncate(callee_base);
        match crate::builtins::from_close(self, gc, &mut op, val) {
            crate::builtins::FromStepOut::Wait => {
                self.pending_from_op = Some(op);
                self.rebase_pending_depths();
                Some(DivertOut::Handled)
            }
            crate::builtins::FromStepOut::Raise(e) => match self.handle_throw(gc, e) {
                Some(exit) => Some(DivertOut::Throw(exit)),
                None => Some(DivertOut::Handled),
            },
            crate::builtins::FromStepOut::Progress | crate::builtins::FromStepOut::Done(_) => {
                unreachable!("from_close never drives or completes")
            }
        }
    }

    /// B1b: drop a live array-iteration machine whose callback/getter
    /// frame is being unwound past (its depth would otherwise point beyond
    /// the stack and misfire on depth coincidence — proven via the Call
    /// skip-list poisoning after getter throws). Called at every frame-pop
    /// site in the unwind paths below.
    fn drop_dead_array_op(&mut self, popped_frame: usize) {
        if self
            .pending_array_op
            .as_ref()
            .is_some_and(|op| op.source_frame_depth == popped_frame)
        {
            self.pending_array_op = None;
        }
    }

    /// B1e: same firewall for the sort machine (drops the merge state
    /// when its callback/getter frame is unwound past).
    fn drop_dead_sort_op(&mut self, popped_frame: usize) {
        if self
            .pending_sort_op
            .as_ref()
            .is_some_and(|op| op.source_frame_depth == popped_frame)
        {
            self.pending_sort_op = None;
        }
    }

    /// B1f-6c: same firewall for the species machine.
    fn drop_dead_species_op(&mut self, popped_frame: usize) {
        if self
            .pending_species_op
            .as_ref()
            .is_some_and(|op| op.source_frame_depth == popped_frame)
        {
            self.pending_species_op = None;
        }
    }

    /// B1f-6b: same firewall for the length machine.
    fn drop_dead_length_op(&mut self, popped_frame: usize) {
        if self
            .pending_length_op
            .as_ref()
            .is_some_and(|op| op.source_frame_depth == popped_frame)
        {
            self.pending_length_op = None;
        }
    }

    /// B1f-4: same firewall for the from machine.
    fn drop_dead_from_op(&mut self, popped_frame: usize) {
        if self
            .pending_from_op
            .as_ref()
            .is_some_and(|op| op.source_frame_depth == popped_frame)
        {
            self.pending_from_op = None;
        }
    }

    /// Unwind stack for a thrown value, routing to try/catch/finally handlers.
    /// This implements the same logic as the Opcode::Throw handler so that
    /// builtins can route exceptions through the JS try/catch mechanism
    /// instead of returning Exit::Throw directly.
    ///
    /// Returns `None` if the exception was handled (caught, finally, or
    /// assert.throws consumed it). Returns `Some(Exit)` if the exception
    /// must propagate up (no handler anywhere).
    pub(crate) fn handle_throw(&mut self, gc: &mut SemiSpace, val: Value) -> Option<Exit> {
        // Outer unwind loop: route into the top frame's handlers, else pop
        // it and repeat for the caller — until handled or frames run out.
        // (Previously the pop-caller tail ran once and then resumed the
        // caller with garbage + exited, so any throw nested 2+ frames deep
        // escaped try/catch and assert.throws.)
        'unwind: loop {
            // Find in-frame handler. Loops: an entry may decline (generator
            // return-sentinel skipping its catch with no finally of its own) —
            // discard it and keep scanning outer entries in this frame.
            loop {
                let handler_idx = self
                    .try_stack
                    .iter()
                    .rposition(|tf| tf.frame_depth == self.frames.len());
                let Some(idx) = handler_idx else {
                    break;
                };
                let (catch_pc, finally_pc, stack_depth, in_catch) = {
                    let tf = &self.try_stack[idx];
                    (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                };
                if in_catch && finally_pc != 0 {
                    self.try_stack[idx].saved_exception = Some(val);
                    self.stack.truncate(stack_depth);
                    let fi = self.frames.len() - 1;
                    self.frames[fi].pc = finally_pc;
                    return None;
                }
                // Generator return-completion sentinel: invisible to user
                // catch (spec: return runs finallys, not catches). Fall through
                // to the finally handling below.
                let is_return_sentinel = self
                    .generator_return_sentinel
                    .is_some_and(|s| s == val.raw());
                if catch_pc != 0 && !in_catch && !is_return_sentinel {
                    self.throw_routed_to_catch = true;
                    if finally_pc != 0 {
                        self.try_stack[idx].in_catch = true;
                    } else {
                        self.try_stack.remove(idx);
                    }
                    self.stack.truncate(stack_depth);
                    self.push(val);
                    let fi = self.frames.len() - 1;
                    self.frames[fi].pc = catch_pc;
                    return None;
                }
                if finally_pc != 0 {
                    self.try_stack[idx].saved_exception = Some(val);
                    self.stack.truncate(stack_depth);
                    let fi = self.frames.len() - 1;
                    self.frames[fi].pc = finally_pc;
                    return None;
                }
                // Declined: no catch taken (or skipped) and no finally here.
                // Discard and keep scanning this frame's outer entries.
                self.try_stack.remove(idx);
            }
            // No handler — pop frame and check caller
            // Respect nested-run_loop floors (generator resume, module eval):
            // the floor frame belongs to the outer loop — unwind to the owner
            // instead of popping it (matches the old single-pop behavior of
            // checking the floor frame's handlers exactly once, below).
            if self.frames.len() <= self.return_frame_floor
                || self.nested_gen_floor == Some(self.frames.len())
            {
                return Some(Exit::Throw(val));
            }
            let popped_frame = self.frames.len() - 1;
            self.last_locals = self.frames[popped_frame].locals.clone();
            // Truncation target for the assert-consume paths below: the
            // popped (thunk/callback) frame's base — drops exactly its temps
            // while preserving live state below (e.g. an enclosing for-of's
            // [iter, next] pair in the caller frame).
            let callee_base = self.frames.last().unwrap().stack_base;
            // B1f-4: a throw escaping a From mapper frame diverts into
            // IteratorClose BEFORE assert.throws can consume it (rebase drags
            // assert's depth onto nested frames; the close must run first per
            // IfAbruptCloseIterator, then the abrupt rethrows into assert).
            // Narrow to Map-awaits on iterable feeds — everything else falls
            // through to the historical order below.
            if let Some(outcome) = self.divert_from_throw(gc, val) {
                match outcome {
                    DivertOut::Handled => return None,
                    DivertOut::Throw(exit) => return Some(exit),
                }
            }
            // Check for pending assert.throws before popping frame. B1f-4: the
            // unwind check uses the PINNED thunk depth (never rebased), so a
            // throw surfacing from nested frames (after inner machines divert
            // or die) still routes into assert at the thunk. The Return path
            // keeps using the dragged depth above.
            let assert_depth = self.pending_assert.as_ref().map(|pa| {
                if pa.pinned_depth == usize::MAX {
                    pa.source_frame_depth
                } else {
                    pa.pinned_depth
                }
            });
            if let Some(source_depth) = assert_depth {
                if self.frames.len() - 1 == source_depth {
                    let pa = self.pending_assert.take().unwrap();
                    if !self.assert_error_matches(pa.expected_error, val) {
                        // Wrong error type — report a mismatch and propagate it.
                        let expected = self.describe_expected_error(pa.expected_error);
                        let actual = crate::builtins::read_error_name(val)
                            .unwrap_or_else(|| "a non-error value".to_string());
                        let detail = crate::builtins::read_error_message(val);
                        let msg = format!(
                            "assert.throws: expected {} but got {}: {}",
                            expected,
                            actual,
                            detail.unwrap_or_default()
                        );
                        let err = crate::builtins::make_error(gc, &self.error_protos, &msg);
                        self.frames.pop();
                        self.try_stack
                            .retain(|tf| tf.frame_depth != popped_frame + 1);
                        self.drop_dead_array_op(popped_frame);
                        self.drop_dead_sort_op(popped_frame);
                        self.drop_dead_from_op(popped_frame);
                        self.drop_dead_length_op(popped_frame);
                        self.drop_dead_species_op(popped_frame);
                        self.stack.truncate(callee_base);
                        return self.handle_throw(gc, err);
                    }
                    self.frames.pop();
                    self.try_stack
                        .retain(|tf| tf.frame_depth != popped_frame + 1);
                    self.drop_dead_array_op(popped_frame);
                    self.drop_dead_sort_op(popped_frame);
                    self.drop_dead_from_op(popped_frame);
                    self.drop_dead_length_op(popped_frame);
                    self.drop_dead_species_op(popped_frame);
                    self.stack.truncate(callee_base);
                    self.push(Value::undefined());
                    let new_fi = self.frames.len() - 1;
                    self.frames[new_fi].pc += 1;
                    return None;
                }
            }
            // Yield* delegation: if the frame being popped is a delegate
            // callback invoked on a suspended outer generator's behalf,
            // resume the outer generator in Throw mode instead of unwinding
            // past it.
            if let Some(outcome) = self.divert_delegate_throw(gc, val) {
                match outcome {
                    DivertOut::Handled => return None,
                    DivertOut::Throw(exit) => return Some(exit),
                }
            }
            self.frames.pop();
            self.try_stack
                .retain(|tf| tf.frame_depth != popped_frame + 1);
            // B1b: drop array machines unwound past (see helper docs).
            self.drop_dead_array_op(popped_frame);
            self.drop_dead_sort_op(popped_frame);
            self.drop_dead_from_op(popped_frame);
            self.drop_dead_length_op(popped_frame);
            self.drop_dead_species_op(popped_frame);
            if self.frames.is_empty() {
                self.stack.clear();
                return Some(Exit::Throw(val));
            }
            // Check for try-catch-finally in the caller frame
            let new_fi = self.frames.len() - 1;
            let caller_idx = self
                .try_stack
                .iter()
                .rposition(|tf| tf.frame_depth == self.frames.len());
            if let Some(idx) = caller_idx {
                let (catch_pc, finally_pc, stack_depth, in_catch) = {
                    let tf = &self.try_stack[idx];
                    (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                };
                if in_catch && finally_pc != 0 {
                    self.try_stack[idx].saved_exception = Some(val);
                    self.stack.truncate(stack_depth);
                    self.frames[new_fi].pc = finally_pc;
                    return None;
                }
                if catch_pc != 0 && !in_catch {
                    self.throw_routed_to_catch = true;
                    if finally_pc != 0 {
                        self.try_stack[idx].in_catch = true;
                    } else {
                        self.try_stack.remove(idx);
                    }
                    self.stack.truncate(stack_depth);
                    self.push(val);
                    self.frames[new_fi].pc = catch_pc;
                    return None;
                }
                if finally_pc != 0 {
                    self.try_stack[idx].saved_exception = Some(val);
                    self.stack.truncate(stack_depth);
                    self.frames[new_fi].pc = finally_pc;
                    return None;
                }
                // No handler in the caller either — keep unwinding outward.
                continue 'unwind;
            }
        }
    }

    /// Register all GC root slots (stack, locals, try_stack saved values).
    /// Must be called after any change to stack/frames/try_stack before GC can run.
    pub fn register_roots(&mut self, gc: &mut SemiSpace) {
        gc.clear_roots();
        for val in &self.stack {
            gc.push_root(val as *const Value as *mut u64);
        }
        for frame in &self.frames {
            for local in &frame.locals {
                gc.push_root(local as *const Value as *mut u64);
            }
            for slot in &frame.lexical_slots {
                gc.push_root(slot as *const Value as *mut u64);
            }
            // Root the frame's captured environment pointer (a valid GC heap pointer)
            if !frame.env.is_null() {
                gc.push_root(&frame.env as *const *mut u8 as *mut u64);
            }
        }
        for tf in &self.try_stack {
            if let Some(ref val) = tf.saved_exception {
                gc.push_root(val as *const Value as *mut u64);
            }
        }
        for val in &self.last_locals {
            gc.push_root(val as *const Value as *mut u64);
        }
        for g in &self.generators {
            for local in &g.locals {
                gc.push_root(local as *const Value as *mut u64);
            }
            for slot in &g.lexical_slots {
                gc.push_root(slot as *const Value as *mut u64);
            }
            for val in &g.stack {
                gc.push_root(val as *const Value as *mut u64);
            }
        }
        // Root builtin prototype objects that are stored as Vm fields
        // (these are not on the stack but are used after GC cycles)
        gc.push_root(&self.object_prototype as *const Value as *mut u64);
        gc.push_root(&self.array_prototype as *const Value as *mut u64);
        gc.push_root(&self.string_prototype as *const Value as *mut u64);
        gc.push_root(&self.string_constructor as *const Value as *mut u64);
        gc.push_root(&self.object_constructor as *const Value as *mut u64);
        gc.push_root(&self.number_constructor as *const Value as *mut u64);
        gc.push_root(&self.promise_constructor as *const Value as *mut u64);
        gc.push_root(&self.map_constructor as *const Value as *mut u64);
        gc.push_root(&self.array_constructor as *const Value as *mut u64);
        gc.push_root(&self.set_constructor as *const Value as *mut u64);
        gc.push_root(&self.map_prototype as *const Value as *mut u64);
        gc.push_root(&self.set_prototype as *const Value as *mut u64);
        gc.push_root(&self.date_constructor as *const Value as *mut u64);
        gc.push_root(&self.regexp_constructor as *const Value as *mut u64);
        gc.push_root(&self.date_prototype as *const Value as *mut u64);
        gc.push_root(&self.array_buffer_constructor as *const Value as *mut u64);
        gc.push_root(&self.array_buffer_prototype as *const Value as *mut u64);
        for v in self.typed_array_ctors.iter_mut() {
            gc.push_root(v as *mut Value as *mut u64);
        }
        for v in self.typed_array_protos.iter_mut() {
            gc.push_root(v as *mut Value as *mut u64);
        }
        for v in self.typed_array_ctor_handles.iter_mut() {
            gc.push_root(v as *mut Value as *mut u64);
        }
        for v in self.error_ctors.iter_mut() {
            gc.push_root(v as *mut Value as *mut u64);
        }
        for v in self.error_protos.iter_mut() {
            gc.push_root(v as *mut Value as *mut u64);
        }
        for v in self.callable_wrappers.iter_mut() {
            gc.push_root(v as *mut Value as *mut u64);
        }
        // Root builtin wrapper objects (assert, Object, Array, String,
        // Function, Error, …) held in the builtin_wrappers map — they are
        // read by LoadGlobal and the Call/New arms, so they must stay
        // forwarded in place after a GC.
        for val in self.builtin_wrappers.values() {
            gc.push_root(val as *const Value as *mut u64);
        }
        gc.push_root(&self.promise_prototype as *const Value as *mut u64);
        gc.push_root(&self.function_prototype as *const Value as *mut u64);
        gc.push_root(&self.regexp_prototype as *const Value as *mut u64);
        gc.push_root(&self.symbol_ctor as *const Value as *mut u64);
        gc.push_root(&self.symbol_prototype as *const Value as *mut u64);
        // Root pre-allocated iteration protocol keys ("done"/"value"/"next")
        gc.push_root(&self.done_key as *const Value as *mut u64);
        gc.push_root(&self.value_key as *const Value as *mut u64);
        gc.push_root(&self.next_key as *const Value as *mut u64);
        // Root pre-allocated typeof result strings (JIT typeof_helper reads these)
        for v in &self.typeof_strings {
            gc.push_root(v as *const Value as *mut u64);
        }
        // Root cached string constant handles (LoadStringConst cache)
        for handles in self.string_cache.values() {
            for v in handles {
                gc.push_root(v as *const Value as *mut u64);
            }
        }
        // Root global variables (StoreGlobal pushes heap Values here)
        for val in self.globals.values() {
            gc.push_root(val as *const Value as *mut u64);
        }
        // Root ESM module environments (exported values, namespaces)
        for rec in &self.module_records {
            for val in rec.env.values() {
                gc.push_root(val as *const Value as *mut u64);
            }
            if let Some(ref ns) = rec.namespace {
                gc.push_root(ns as *const Value as *mut u64);
            }
        }
        // Root JIT call helper's locals buffer (holds callee + args during JIT calls)
        for val in &self.jit_locals_buffer {
            gc.push_root(val as *const Value as *mut u64);
        }
        // Root pending exception (holds thrown Value between throw and catch)
        if let Some(ref val) = self.pending_exception {
            gc.push_root(val as *const Value as *mut u64);
        }
        // F4: every pending machine's roots live in root_pending_values.
        self.root_pending_values(gc);
    }

    /// F4: single place that roots every live pending machine's GC-managed
    /// values. Called at the end of `register_roots`. Rules: root every
    /// `Value` field (callbacks, receivers, iterators, collections, abrupt
    /// args, stashes, accumulators, promise handles) and every `*mut u8`
    /// slot that points into the GC heap (result arrays, snapshots); plain
    /// data (`usize`, `bool`, `String`, state enums) needs nothing. When
    /// adding a machine, add its fields here — nowhere else.
    fn root_pending_values(&self, gc: &mut SemiSpace) {
        if let Some(ref op) = self.pending_array_op {
            gc.push_root(&op.callback as *const Value as *mut u64);
            gc.push_root(&op.this_val as *const Value as *mut u64);
            gc.push_root(&op.source_val as *const Value as *mut u64);
            gc.push_root(&op.source as *const *mut u8 as *mut u64);
            gc.push_root(&op.result as *const *mut u8 as *mut u64);
            if let Some(ref acc) = op.accumulator {
                gc.push_root(acc as *const Value as *mut u64);
            }
            if let Some(ref fb) = op.species_fallback {
                gc.push_root(fb as *const Value as *mut u64);
            }
        }
        if let Some(ref pa) = self.pending_assert {
            gc.push_root(&pa.expected_error as *const Value as *mut u64);
        }
        if let Some(ref ppc) = self.pending_promise_ctor {
            gc.push_root(&ppc.promise as *const Value as *mut u64);
            gc.push_root(&ppc.resolve_handle as *const Value as *mut u64);
            gc.push_root(&ppc.reject_handle as *const Value as *mut u64);
        }
        if let Some(ref ppc) = self.pending_primitive_conversion {
            gc.push_root(&ppc.other_operand as *const Value as *mut u64);
        }
        if let Some(ref pfo) = self.pending_finally_op {
            gc.push_root(&pfo.promise as *const Value as *mut u64);
            gc.push_root(&pfo.orig_value as *const Value as *mut u64);
        }
        if let Some(ref pag) = self.pending_async_gen {
            gc.push_root(&pag.arg as *const Value as *mut u64);
        }
        if let Some(ref pa) = self.pending_yield_star_afr {
            gc.push_root(&pa.abrupt_arg as *const Value as *mut u64);
            gc.push_root(&pa.iter as *const Value as *mut u64);
        }
        if let Some(ref pg) = self.pending_yield_star_get {
            gc.push_root(&pg.abrupt_arg as *const Value as *mut u64);
            gc.push_root(&pg.iter as *const Value as *mut u64);
            gc.push_root(&pg.stash as *const Value as *mut u64);
        }
        if let Some(ref pid) = self.pending_iter_drain {
            gc.push_root(&pid.iter as *const Value as *mut u64);
            gc.push_root(&pid.next as *const Value as *mut u64);
            gc.push_root(&pid.receiver as *const Value as *mut u64);
            gc.push_root(&pid.result as *const *mut u8 as *mut u64);
        }
        if let Some(ref pcc) = self.pending_collection_ctor {
            gc.push_root(&pcc.collection as *const Value as *mut u64);
            gc.push_root(&pcc.iter as *const Value as *mut u64);
            gc.push_root(&pcc.next as *const Value as *mut u64);
        }
        if let Some(ref pfe) = self.pending_collection_foreach {
            gc.push_root(&pfe.snapshot as *const *mut u8 as *mut u64);
            gc.push_root(&pfe.callback as *const Value as *mut u64);
            gc.push_root(&pfe.this_arg as *const Value as *mut u64);
            gc.push_root(&pfe.collection as *const Value as *mut u64);
        }
        gc.push_root(&self.last_pushed_callee as *const Value as *mut u64);
        gc.push_root(&self.last_popped_callee as *const Value as *mut u64);
        if let Some(ref pso) = self.pending_sort_op {
            gc.push_root(&pso.source_val as *const Value as *mut u64);
            gc.push_root(&pso.write_val as *const Value as *mut u64);
            gc.push_root(&pso.comparator as *const Value as *mut u64);
            gc.push_root(&pso.prim_value as *const Value as *mut u64);
            for item in pso.items.iter().chain(pso.aux.iter()) {
                gc.push_root(&item.value as *const Value as *mut u64);
            }
            gc.push_root(&pso.await_callee as *const Value as *mut u64);
        }
        if let Some(ref pf) = self.pending_from_op {
            gc.push_root(&pf.result as *const Value as *mut u64);
            gc.push_root(&pf.mapper as *const Value as *mut u64);
            gc.push_root(&pf.map_this as *const Value as *mut u64);
            gc.push_root(&pf.await_callee as *const Value as *mut u64);
            gc.push_root(&pf.staged_value as *const Value as *mut u64);
            gc.push_root(&pf.done_key as *const Value as *mut u64);
            gc.push_root(&pf.value_key as *const Value as *mut u64);
            if let Some(ref a) = pf.closing_abrupt {
                gc.push_root(a as *const Value as *mut u64);
            }
            match &pf.feed {
                FromFeed::ArrayLike { obj, .. } => {
                    gc.push_root(obj as *const Value as *mut u64);
                }
                FromFeed::Iter { iter, next } => {
                    gc.push_root(iter as *const Value as *mut u64);
                    gc.push_root(next as *const Value as *mut u64);
                }
            }
        }
        if let Some(ref pl) = self.pending_length_op {
            gc.push_root(&pl.stash_this as *const Value as *mut u64);
            gc.push_root(&pl.stash_this as *const Value as *mut u64);
            gc.push_root(&pl.staged as *const Value as *mut u64);
            gc.push_root(&pl.await_callee as *const Value as *mut u64);
            for a in pl.stash_args.iter() {
                gc.push_root(a as *const Value as *mut u64);
            }
        }
        if let Some(ref ps) = self.pending_species_op {
            gc.push_root(&ps.stash_this as *const Value as *mut u64);
            gc.push_root(&ps.ctor as *const Value as *mut u64);
            gc.push_root(&ps.constructed_this as *const Value as *mut u64);
            gc.push_root(&ps.await_callee as *const Value as *mut u64);
            for a in ps.stash_args.iter() {
                gc.push_root(a as *const Value as *mut u64);
            }
        }
        if let Some(ref sm) = self.species_made {
            gc.push_root(sm as *const Value as *mut u64);
        }
        if let Some(ref pra) = self.pending_replace_all_op {
            gc.push_root(&pra.fn_val as *const Value as *mut u64);
        }
        if let Some(ref ppc) = self.pending_call {
            if let PendingCallCont::ToPrimString { value, .. } = ppc.cont {
                gc.push_root(&value as *const Value as *mut u64);
            }
        }
    }

    /// Push a frame for a JS function call (used by array method callbacks).
    /// Reads the function program from `callee` (must be TAG_FUNC) and sets
    /// up locals with the given args. Updates `source_frame_depth` in the
    /// pending array op state to the frame count before pushing.
    pub fn frame_depth(&self) -> usize {
        self.frames.len()
    }

    pub fn push_callback_call(
        &mut self,
        _gc: &mut SemiSpace,
        callee: Value,
        this: Value,
        args: Vec<Value>,
    ) {
        let ptr = callee.heap_ptr().expect("callback must be heap object");
        let func_ptr = ptr as *mut Func;
        let func_env = unsafe { Func::env_ptr(func_ptr) };
        let func_idx = unsafe { Func::func_index(func_ptr) } as usize;
        let creator_prog = unsafe { &*(Func::prog_ptr(func_ptr) as *const BytecodeProgram) };
        let func_prog = &creator_prog.functions[func_idx];
        let passed_argc = args.len();
        // B1e: see last_popped_callee.
        self.last_pushed_callee = callee;
        let mut locals: Vec<Value> = if func_prog.named_function {
            vec![callee]
        } else {
            vec![]
        };
        locals.extend(args);
        self.frames.push(Frame {
            locals,
            lexical_slots: Vec::new(),
            lexical_tdz: Vec::new(),
            lexical_const: Vec::new(),
            scope_boundaries: Vec::new(),
            passed_argc,
            pc: 0,
            stack_base: self.stack.len(),
            prog: func_prog as *const BytecodeProgram,
            generator_id: None,
            this,
            is_constructor_call: false,
            constructed_object: Value::undefined(),
            env: func_env,
            func_ptr: func_ptr as *mut u8,
            private_name_ids: std::ptr::null_mut(),
        });
        // F4: every live machine tracks the newest callback frame (single
        // place — historically new machines forgot this and fired on the
        // wrong Return, cf. v0.8.1 skip-gate batch).
        self.rebase_pending_depths();
    }

    /// Construct(ctor, args) for builtins (B1f-6d species/of/from): mirrors
    /// the Opcode::New arms without touching the operand stack (caller holds
    /// ctor/args in its op). TAG_FUNC allocates this (proto from
    /// ctor.prototype) + pushes a constructor frame; builtin ctors run
    /// inline; arrows/non-ctors Fail. Other builtin exotics (Map/Set/… as a
    /// species) are a documented micro-gap (no test262 coverage). Like
    /// push_callback_call, rebases live machines.
    pub(crate) fn push_construct_frame(
        &mut self,
        gc: &mut SemiSpace,
        ctor: Value,
        args: Vec<Value>,
    ) -> ConstructOut {
        // Array [[Construct]] inline (single-arg length form et al).
        if ctor == self.array_constructor {
            let result = crate::builtins::array_constructor(gc, Value::undefined(), &args, self);
            if let Some(exc) = self.pending_exception.take() {
                return ConstructOut::Fail(exc);
            }
            return ConstructOut::Done(result);
        }
        // Object [[Construct]]: fresh empty object (args ignored).
        if ctor == self.object_constructor {
            let shape = Shape::empty();
            let obj = JSObject::allocate(gc, shape, &[]);
            if let Some(pp) = self.object_prototype.heap_ptr() {
                unsafe {
                    JSObject::set_prototype(obj, pp);
                }
            }
            return ConstructOut::Done(Value::from_heap_ptr(obj as *mut u8));
        }
        // Smi builtins: only Test262Error constructs (New-arm rule).
        if let Some(smi_val) = ctor.as_smi() {
            if smi_val < 0 {
                let id = ((-smi_val) as usize) - 1;
                if id < self.builtins.len() {
                    if self.builtins[id].name != "Test262Error" {
                        let exc = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            &format!("{} is not a constructor", self.builtins[id].name),
                        );
                        return ConstructOut::Fail(exc);
                    }
                    let shape = Shape::empty();
                    let obj = JSObject::allocate(gc, shape, &[]);
                    let obj_val = Value::from_heap_ptr(obj as *mut u8);
                    let result = (self.builtins[id].func)(gc, obj_val, &args, &mut *self);
                    if let Some(exc) = self.pending_exception.take() {
                        return ConstructOut::Fail(exc);
                    }
                    if result.is_heap_object() {
                        return ConstructOut::Done(result);
                    }
                    return ConstructOut::Done(obj_val);
                }
            }
            return ConstructOut::Fail(crate::errors::error_object(
                gc,
                &self.error_protos,
                crate::errors::ErrorKind::TypeError,
                "value is not a constructor",
            ));
        }
        // User functions: fresh this + constructor frame (New-arm shape).
        if let Some(ptr) = ctor.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_FUNC {
                if unsafe { Func::is_arrow(ptr as *mut Func) } {
                    return ConstructOut::Fail(crate::errors::error_object(
                        gc,
                        &self.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        "Arrow function is not a constructor",
                    ));
                }
                let shape = Shape::empty();
                let obj = JSObject::allocate(gc, shape, &[]);
                let obj_val = Value::from_heap_ptr(obj as *mut u8);
                if let Some(p) = ctor.heap_ptr() {
                    let tag = unsafe { (*(p as *const GcHeader)).tag() };
                    if tag == TAG_OBJECT {
                        let shape = unsafe { JSObject::shape_ptr(p as *mut JSObject) };
                        if let Some(slot) = shape.lookup(&PROTOTYPE_KEY) {
                            let proto_val = unsafe { JSObject::get_slot(p as *mut JSObject, slot) };
                            if proto_val.is_heap_object() {
                                if let Some(proto_ptr) = proto_val.heap_ptr() {
                                    unsafe {
                                        JSObject::set_prototype(obj, proto_ptr);
                                    }
                                }
                            }
                        }
                    } else if tag == TAG_FUNC {
                        let proto_ptr = unsafe { Func::prototype(p as *mut Func) };
                        if !proto_ptr.is_null() {
                            unsafe {
                                JSObject::set_prototype(obj, proto_ptr);
                            }
                        }
                    }
                }
                let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                let creator_prog =
                    unsafe { &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram) };
                if func_idx < creator_prog.functions.len() {
                    let func_prog = &creator_prog.functions[func_idx];
                    let func_ptr = ptr as *mut Func;
                    let func_env = unsafe { Func::env_ptr(func_ptr) };
                    // B1e: callee identity for machine Return arms.
                    self.last_pushed_callee = ctor;
                    let mut locals: Vec<Value> = if func_prog.named_function {
                        vec![ctor]
                    } else {
                        vec![]
                    };
                    let passed_argc = args.len();
                    locals.extend(args);
                    self.frames.push(Frame {
                        locals,
                        lexical_slots: Vec::new(),
                        lexical_tdz: Vec::new(),
                        lexical_const: Vec::new(),
                        scope_boundaries: Vec::new(),
                        passed_argc,
                        pc: 0,
                        stack_base: self.stack.len(),
                        prog: func_prog as *const BytecodeProgram,
                        generator_id: None,
                        this: obj_val,
                        is_constructor_call: true,
                        constructed_object: obj_val,
                        env: func_env,
                        func_ptr: func_ptr as *mut u8,
                        private_name_ids: std::ptr::null_mut(),
                    });
                    self.rebase_pending_depths();
                    return ConstructOut::Pushed(obj_val);
                }
                return ConstructOut::Fail(crate::errors::error_object(
                    gc,
                    &self.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "value is not a constructor",
                ));
            }
        }
        ConstructOut::Fail(crate::errors::error_object(
            gc,
            &self.error_protos,
            crate::errors::ErrorKind::TypeError,
            "value is not a constructor",
        ))
    }

    /// F4: overwrite every live pending machine's `source_frame_depth` with
    /// the current callback frame index. Called by `push_callback_call`
    /// after pushing a callback frame. When adding a machine, add one arm
    /// here — nowhere else. Also used after manual getter-frame pushes
    /// (B1a array elements), which bypass push_callback_call.
    pub(crate) fn rebase_pending_depths(&mut self) {
        let depth = self.frames.len() - 1;
        if let Some(ref mut state) = self.pending_array_op {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_call {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_assert {
            state.source_frame_depth = depth;
            // B1f-4: pin the thunk frame on first push only — later pushes
            // (nested callbacks, machine frames) must not drag the unwind
            // check (see PendingAssert::pinned_depth).
            if state.pinned_depth == usize::MAX {
                state.pinned_depth = depth;
            }
        }
        if let Some(ref mut state) = self.pending_promise_ctor {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_finally_op {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_replace_op {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_replace_all_op {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_symbol_dispatch {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_symbol_coercion {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_for_of_init {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_for_of_next {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_yield_star_next {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_yield_star_afr {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_yield_star_get {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_iter_drain {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_collection_ctor {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_collection_foreach {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_sort_op {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_from_op {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_length_op {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_species_op {
            state.source_frame_depth = depth;
        }
        // F4 additions: accessor_call and primitive_conversion never
        // rebased (stale depth if a nested callback pushed between set and
        // return). PendingAsyncGen has no depth field (bridge-driven, not
        // Return-driven) — nothing to rebase.
        if let Some(ref mut state) = self.pending_accessor_call {
            state.source_frame_depth = depth;
        }
        if let Some(ref mut state) = self.pending_primitive_conversion {
            state.source_frame_depth = depth;
        }
    }

    /// F4: whether a builtin `Call` at frame `fi` must skip its result push
    /// and pc advance because a pending machine owns the continuation (the
    /// callback-setup builtin pushed the callback frame itself; the Return
    /// handler's state machine owns the pc). Membership is exactly the
    /// historical 12 plus the length/species re-dispatch machines (whose
    /// builtins return junk that must not leak onto the caller stack); other
    /// machines (for_of_*, yield_star_next,
    /// iter_drain, accessor_call, …) complete through different paths and
    /// must NOT skip. When adding a machine, decide membership here.
    fn pending_owns_call(&self, fi: usize) -> bool {
        self.pending_array_op
            .as_ref()
            .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_call
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_assert
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_promise_ctor
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_finally_op
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_replace_op
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_replace_all_op
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_symbol_dispatch
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_collection_foreach
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_sort_op
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_from_op
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_length_op
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_species_op
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_collection_ctor
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_yield_star_afr
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
            || self
                .pending_yield_star_get
                .as_ref()
                .is_some_and(|p| fi < p.source_frame_depth)
    }

    /// Execute a bytecode program and return its result.
    /// Check if a value is an AccessorPair. If so, extract the getter and push a
    /// frame to call it, setting `pending_accessor_call` so the Return handler
    /// can route the getter's return value back to the LoadProperty caller.
    /// Returns the original value if not an accessor, or undefined if getter undefined.
    /// When a getter frame is pushed, the caller MUST `continue` the VM loop.
    fn resolve_accessor_for_read(
        &mut self,
        val: Value,
        this: Value,
        _gc: &mut SemiSpace,
    ) -> (Value, bool) {
        if let Some(ptr) = val.heap_ptr() {
            if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_ACCESSOR {
                let getter = unsafe { AccessorPair::getter(ptr) };
                if !getter.is_undefined() {
                    if let Some(gptr) = getter.heap_ptr() {
                        if unsafe { (*(gptr as *const GcHeader)).tag() } == TAG_FUNC {
                            let func_ptr = gptr;
                            let func_idx =
                                unsafe { Func::func_index(func_ptr as *mut Func) } as usize;
                            let creator_prog = unsafe {
                                &*(Func::prog_ptr(func_ptr as *mut Func) as *const BytecodeProgram)
                            };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];
                                let func_env = unsafe { Func::env_ptr(func_ptr as *mut Func) };
                                let locals = if func_prog.named_function {
                                    vec![getter]
                                } else {
                                    vec![]
                                };
                                self.pending_accessor_call = Some(PendingAccessorCall {
                                    source_frame_depth: self.frames.len(),
                                    is_getter: true,
                                });
                                self.frames.push(Frame {
                                    locals,
                                    lexical_slots: Vec::new(),
                                    lexical_tdz: Vec::new(),
                                    lexical_const: Vec::new(),
                                    scope_boundaries: Vec::new(),
                                    passed_argc: 0,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this,
                                    is_constructor_call: false,
                                    constructed_object: Value::undefined(),
                                    env: func_env,
                                    func_ptr,
                                    private_name_ids: std::ptr::null_mut(),
                                });
                                return (Value::undefined(), true);
                            }
                        }
                    }
                }
                return (Value::undefined(), false);
            }
        }
        (val, false)
    }

    /// Load `key` from `obj` with accessor dispatch for the yield*
    /// machinery. Plain values (and missing/undefined getters) resolve
    /// synchronously to Ready. A JS-function getter pushes its frame, records
    /// `pend`, and returns Wait (caller must bail without advancing; resume
    /// via the PendingYieldStarGet Return arm). A builtin getter is invoked
    /// synchronously; its abrupt paths surface as a raised pending exception
    /// for builtin contexts to observe (return undefined, rely on skip-list)
    /// or as an Exit for handler contexts — reported via Bail so each caller
    /// translates for its own context.
    pub(crate) fn yieldstar_get(
        &mut self,
        gc: &mut SemiSpace,
        obj: Value,
        key: Value,
        pend: PendingYieldStarGet,
    ) -> YsGetOut {
        let raw = load_property_recursive(obj, key, None, gc);
        let Some(ptr) = raw.heap_ptr() else {
            return YsGetOut::Ready(raw);
        };
        if unsafe { (*(ptr as *const GcHeader)).tag() } != TAG_ACCESSOR {
            return YsGetOut::Ready(raw);
        }
        let getter = unsafe { AccessorPair::getter(ptr) };
        if getter.is_undefined() {
            return YsGetOut::Ready(Value::undefined());
        }
        if getter.as_smi().is_some_and(|s| s < 0) {
            return match call_builtin_sync(self, gc, getter, obj, &[]) {
                Ok(v) => YsGetOut::Ready(v),
                Err(Some(exit)) => YsGetOut::Bail(Some(exit)),
                Err(None) => YsGetOut::Bail(None),
            };
        }
        let Some(gptr) = getter.heap_ptr() else {
            return YsGetOut::Ready(Value::undefined());
        };
        if unsafe { (*(gptr as *const GcHeader)).tag() } != TAG_FUNC {
            return YsGetOut::Ready(Value::undefined());
        }
        let func_ptr = gptr;
        let func_idx = unsafe { Func::func_index(func_ptr as *mut Func) } as usize;
        let creator_prog =
            unsafe { &*(Func::prog_ptr(func_ptr as *mut Func) as *const BytecodeProgram) };
        if func_idx >= creator_prog.functions.len() {
            return YsGetOut::Ready(Value::undefined());
        }
        let func_prog = &creator_prog.functions[func_idx];
        let func_env = unsafe { Func::env_ptr(func_ptr as *mut Func) };
        let locals = if func_prog.named_function {
            vec![getter]
        } else {
            vec![]
        };
        self.pending_yield_star_get = Some(pend);
        self.frames.push(Frame {
            locals,
            lexical_slots: Vec::new(),
            lexical_tdz: Vec::new(),
            lexical_const: Vec::new(),
            scope_boundaries: Vec::new(),
            passed_argc: 0,
            pc: 0,
            stack_base: self.stack.len(),
            prog: func_prog as *const BytecodeProgram,
            generator_id: None,
            this: obj,
            is_constructor_call: false,
            constructed_object: Value::undefined(),
            env: func_env,
            func_ptr,
            private_name_ids: std::ptr::null_mut(),
        });
        YsGetOut::Wait
    }

    pub fn execute(
        &mut self,
        gc: &mut SemiSpace,
        program: &BytecodeProgram,
    ) -> Result<Value, Value> {
        self.frames.clear();
        self.stack.clear();
        self.try_stack.clear();
        self.assert_called = false;

        // Initialize top-level locals from persisted globals
        // (module programs never go through execute(), but guard anyway —
        // module locals are seeded by ImportModule, not globals)
        let locals: Vec<Value> = if program.is_module {
            vec![Value::undefined(); program.local_names.len()]
        } else {
            program
                .local_names
                .iter()
                .map(|name| {
                    self.globals
                        .get(name)
                        .copied()
                        .unwrap_or(Value::undefined())
                })
                .collect()
        };

        self.frames.push(Frame {
            locals,
            lexical_slots: Vec::new(),
            lexical_tdz: Vec::new(),
            lexical_const: Vec::new(),
            scope_boundaries: Vec::new(),
            passed_argc: 0,
            pc: 0,
            stack_base: 0,
            prog: program as *const BytecodeProgram,
            generator_id: None,
            this: Value::undefined(),
            is_constructor_call: false,
            constructed_object: Value::undefined(),
            env: std::ptr::null_mut(),
            func_ptr: std::ptr::null_mut(),
            private_name_ids: std::ptr::null_mut(),
        });

        self.register_roots(gc);

        // Enable automatic root refresh before each GC cycle
        gc.root_provider = Some(self as *mut dyn RootProvider);

        let result = match self.run_loop(gc) {
            Exit::Return(v) => Ok(v),
            Exit::Yield(_) => Ok(Value::undefined()),
            Exit::Throw(v) => Err(v),
        };

        // Drain microtask queue after the synchronous task completes
        self.drain_microtask_queue(gc);

        // Disable root provider until next execute
        gc.root_provider = None;

        // Sync locals back to globals for persistence
        for (i, name) in program.local_names.iter().enumerate() {
            if i < self.last_locals.len() && !program.is_module {
                self.globals.insert(name.clone(), self.last_locals[i]);
            }
        }

        result
    }

    /// Evaluate an ESM module (and, transitively, all of its dependencies) and
    /// return the module's evaluation outcome. Modules must be pre-registered
    /// via `modules`/`module_records` before calling (the embedding Context
    /// compiles the whole import graph first).
    pub fn evaluate_module(&mut self, gc: &mut SemiSpace, specifier: &str) -> Result<(), Value> {
        let idx = match self.modules.get(specifier) {
            Some(&i) => i,
            None => return Ok(()),
        };
        self.eval_module_rec(gc, idx)
    }

    /// The module record in effect for the current frame: the owning module
    /// of the executing function (functions created during module evaluation
    /// resolve imported bindings against their own module), falling back to
    /// the top of the module evaluation stack (top-level module code).
    fn current_module_mi(&self, fi: usize) -> Option<usize> {
        self.module_mi_of_frame(fi)
            .or_else(|| self.module_stack.last().copied())
    }

    /// The module record index owning the executing function of frame `fi`
    /// (None for script functions and the module top-level program itself,
    /// which resolves via `globals_override` instead).
    fn module_mi_of_frame(&self, fi: usize) -> Option<usize> {
        let fp = self.frames.get(fi)?.func_ptr;
        if fp.is_null() {
            return None;
        }
        let mi = unsafe { Func::module_mi(fp as *mut Func) };
        if mi < 0 { None } else { Some(mi as usize) }
    }

    /// Resolve a global-name read against the owning module of the executing
    /// function: the module env (locals, namespace seeds, rename syncs) first,
    /// then live imported bindings, then None (caller falls back to globals).
    fn load_global_from_module_frame(
        &mut self,
        gc: &mut SemiSpace,
        fi: usize,
        name: &str,
    ) -> Option<Value> {
        let mi = self.module_mi_of_frame(fi)?;
        let env = &self.module_records[mi].env;
        if let Some(v) = env.get(name) {
            if v == &Value::empty_sentinel() {
                self.pending_exception = Some(self.tdz_error(gc, name));
                return None;
            }
            return Some(*v);
        }
        let info = unsafe { (*self.module_records[mi].program).module.as_ref() }?;
        for imp in &info.imports {
            if imp.imported == "*ns*" || imp.local != name {
                continue;
            }
            let dep = self.modules.get(&imp.specifier).copied()?;
            return self.resolve_export_value(gc, dep, &imp.imported, &mut Vec::new());
        }
        None
    }

    /// Evaluate one module record (DFS, cycle-safe via `status`).
    ///
    /// Runs the module's program in its own frame. LoadGlobal/StoreGlobal are
    /// redirected into the module env (`globals_override`) while it runs, and
    /// nested module evaluation re-enters `run_loop` with a `return_frame_floor`
    /// so the nested loop exits when the module frame returns.
    fn eval_module_rec(&mut self, gc: &mut SemiSpace, idx: usize) -> Result<(), Value> {
        let status = self.module_records[idx].status;
        if status != 0 {
            // Cycle or already evaluated — bindings are available (section 1
            // of an evaluating module has already run for cycles).
            return Ok(());
        }
        self.module_records[idx].status = 1;
        let program = self.module_records[idx].program;
        self.module_stack.push(idx);
        let saved_override = self.globals_override.replace(idx);
        let saved_floor = self.return_frame_floor;
        self.return_frame_floor = self.frames.len();
        let base = self.stack.len();
        self.frames.push(Frame {
            locals: vec![Value::undefined(); unsafe { (*program).local_names.len() }],
            lexical_slots: Vec::new(),
            lexical_tdz: Vec::new(),
            lexical_const: Vec::new(),
            scope_boundaries: Vec::new(),
            passed_argc: 0,
            pc: 0,
            stack_base: base,
            prog: program,
            generator_id: None,
            this: Value::undefined(),
            is_constructor_call: false,
            constructed_object: Value::undefined(),
            env: std::ptr::null_mut(),
            func_ptr: std::ptr::null_mut(),
            private_name_ids: std::ptr::null_mut(),
        });
        let exit = self.run_loop(gc);
        // The exception unwinder may have already popped frames above this
        // module's frame (uncaught throw) — truncate defensively.
        let saved_len = self.return_frame_floor;
        self.return_frame_floor = saved_floor;
        self.globals_override = saved_override;
        self.module_stack.pop();
        self.frames.truncate(saved_len);
        self.stack.truncate(base);
        self.module_records[idx].status = 2;
        match exit {
            Exit::Return(_) => Ok(()),
            Exit::Throw(v) => Err(v),
            Exit::Yield(_) => Ok(()),
        }
    }

    /// A catchable ReferenceError for a TDZ module-binding read.
    fn tdz_error(&self, gc: &mut SemiSpace, name: &str) -> Value {
        let msg = format!("Cannot access '{name}' before initialization");
        crate::errors::error_object(
            gc,
            &self.error_protos,
            crate::errors::ErrorKind::ReferenceError,
            &msg,
        )
    }

    /// Resolve an exported name of a module to its current value, following
    /// local bindings, re-exports (`export {a} from`), namespace exports, and
    /// star exports. `visited` guards against star-export cycles.
    pub fn resolve_export_value(
        &mut self,
        gc: &mut SemiSpace,
        mi: usize,
        export_name: &str,
        visited: &mut Vec<usize>,
    ) -> Option<Value> {
        if visited.contains(&mi) {
            return None;
        }
        visited.push(mi);
        let info = unsafe {
            let rec = &self.module_records[mi];
            (*rec.program).module.as_ref()?
        };
        // 1. Local exports (export_name → local binding).
        for (exported, local) in &info.local_exports {
            if exported == export_name {
                // The local may itself be an imported binding re-exported.
                for imp in &info.imports {
                    if imp.imported != "*ns*" && &imp.local == local {
                        if let Some(&dep) = self.modules.get(&imp.specifier) {
                            return self.resolve_export_value(gc, dep, &imp.imported, visited);
                        }
                        return None;
                    }
                }
                return self.module_records[mi].env.get(local).copied();
            }
        }
        // 2. Indirect exports.
        for (exported, spec, imported) in &info.indirect_exports {
            if exported == export_name {
                if let Some(&dep) = self.modules.get(spec) {
                    return self.resolve_export_value(gc, dep, imported, visited);
                }
                return None;
            }
        }
        // 3. Namespace exports (`export * as ns from`).
        for (ns, spec) in &info.namespace_exports {
            if ns == export_name {
                if let Some(&dep) = self.modules.get(spec) {
                    return Some(self.make_module_namespace(gc, dep));
                }
                return None;
            }
        }
        // 4. Star exports (first hit wins; conflicts resolve to the first).
        for spec in &info.star_exports {
            if let Some(&dep) = self.modules.get(spec) {
                if let Some(v) = self.resolve_export_value(gc, dep, export_name, visited) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Enumerate the export names of a module (local + indirect + namespace
    /// + star-merged, minus star conflicts and `default`).
    fn module_export_names(&self, mi: usize) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let Some(info) = (unsafe { (*self.module_records[mi].program).module.as_ref() }) else {
            return names;
        };
        for (exported, _) in &info.local_exports {
            if !names.contains(exported) {
                names.push(exported.clone());
            }
        }
        for (exported, _, _) in &info.indirect_exports {
            if !names.contains(exported) {
                names.push(exported.clone());
            }
        }
        for (ns, _) in &info.namespace_exports {
            if !names.contains(ns) {
                names.push(ns.clone());
            }
        }
        for spec in &info.star_exports {
            if let Some(&dep) = self.modules.get(spec) {
                for name in self.module_export_names(dep) {
                    if name == "default" || names.contains(&name) {
                        continue;
                    }
                    names.push(name);
                }
            }
        }
        names
    }

    /// Create (or return the cached) module namespace object for a module —
    /// a plain JSObject snapshot of its exports (§16.2.1.2 CreateNamespace).
    /// Values are snapshotted at creation time (not live).
    fn make_module_namespace(&mut self, gc: &mut SemiSpace, mi: usize) -> Value {
        if let Some(ns) = self.module_records[mi].namespace {
            return ns;
        }
        let names = self.module_export_names(mi);
        let mut entries: Vec<(PropertyKey, usize)> = Vec::with_capacity(names.len());
        let mut key_names: Vec<String> = Vec::with_capacity(names.len());
        let mut values: Vec<Value> = Vec::with_capacity(names.len());
        for name in &names {
            entries.push((PropertyKey::from_string(name), values.len()));
            key_names.push(name.clone());
            let v = self
                .resolve_export_value(gc, mi, name, &mut Vec::new())
                .unwrap_or(Value::undefined());
            values.push(v);
        }
        let shape = Shape::intern(entries, key_names);
        let obj = JSObject::allocate(gc, shape, &values);
        if self.object_prototype.is_heap_object() {
            unsafe { JSObject::set_prototype(obj, self.object_prototype.heap_ptr().unwrap()) };
        }
        let ns = Value::from_heap_ptr(obj as *mut u8);
        self.module_records[mi].namespace = Some(ns);
        ns
    }

    /// Resume a suspended generator with `arg` as the yield result value.
    /// Returns the next yielded (or returned) value.
    pub fn resume_generator(
        &mut self,
        gc: &mut SemiSpace,
        gen_id: usize,
        arg: Value,
    ) -> Result<Value, Value> {
        // Legacy semantics: Yield and Return both flatten to the value
        // (used by Context::resume and the async machinery).
        self.resume_generator_inner(gc, gen_id, GeneratorResume::Next(arg))
            .map(|(v, _done)| v)
    }

    /// Resume a generator for `next(arg)` — returns `(value, done)`.
    pub(crate) fn resume_generator_full(
        &mut self,
        gc: &mut SemiSpace,
        gen_id: usize,
        resume: GeneratorResume,
    ) -> Result<(Value, bool), Value> {
        self.resume_generator_inner(gc, gen_id, resume)
    }

    fn resume_generator_inner(
        &mut self,
        gc: &mut SemiSpace,
        gen_id: usize,
        resume: GeneratorResume,
    ) -> Result<(Value, bool), Value> {
        if self.generators[gen_id].done {
            return Ok((Value::undefined(), true));
        }
        if self.generators[gen_id].executing {
            return Err(crate::errors::error_object(
                gc,
                &self.error_protos,
                crate::errors::ErrorKind::TypeError,
                "generator already running",
            ));
        }
        // A suspended abrupt (Throw/Return resume that yielded again,
        // e.g. inside a `finally`) takes precedence over the new request.
        let resume = self.generators[gen_id].abrupt.take().unwrap_or(resume);
        let is_abrupt = !matches!(resume, GeneratorResume::Next(_));
        self.generators[gen_id].executing = true;
        // Swap in the generator's own try/catch/finally frames; the caller's
        // stack is restored when the nested run_loop returns. (Yield banks
        // the live stack back into the generator on suspension.)
        let caller_try = std::mem::take(&mut self.try_stack);
        self.try_stack = std::mem::take(&mut self.generators[gen_id].try_frames);

        let (
            locals,
            lexical_slots,
            lexical_tdz,
            lexical_const,
            scope_boundaries,
            pc,
            prog,
            started,
            saved_this,
            saved_env,
            saved_stack,
        ) = {
            let g = &mut self.generators[gen_id];
            (
                g.locals.clone(),
                g.lexical_slots.clone(),
                g.lexical_tdz.clone(),
                g.lexical_const.clone(),
                g.scope_boundaries.clone(),
                g.pc,
                g.prog,
                g.started,
                g.this,
                g.env,
                std::mem::take(&mut g.stack),
            )
        };

        // Scope the nested run_loop to THIS frame: without a floor, once the
        // generator Returns the loop would continue executing the PARENT
        // (caller) frame inside the nested call and only stop when the whole
        // script ran out of frames.
        let floor_base = self.frames.len();
        let saved_floor = self.return_frame_floor;
        let saved_gen_floor = self.nested_gen_floor;
        self.return_frame_floor = floor_base;
        self.nested_gen_floor = Some(floor_base);
        self.frames.push(Frame {
            locals,
            lexical_slots,
            lexical_tdz,
            lexical_const,
            scope_boundaries,
            passed_argc: 0,
            pc,
            stack_base: self.stack.len(),
            prog,
            generator_id: Some(gen_id),
            this: saved_this,
            is_constructor_call: false,
            constructed_object: Value::undefined(),
            env: saved_env,
            func_ptr: std::ptr::null_mut(),
            private_name_ids: std::ptr::null_mut(),
        });

        // Rebase banked try frames onto the fresh frame: TryBegin records
        // frame_depth as frames.len() at push time, so it must be the
        // post-push length; stack depths rebase relative to suspension base.
        {
            let new_fi = self.frames.len();
            let new_base = self.stack.len();
            let old_base = self.generators[gen_id].stack_base_saved;
            for tf in self.try_stack.iter_mut() {
                tf.frame_depth = new_fi;
                let rel = tf.stack_depth.saturating_sub(old_base);
                tf.stack_depth = new_base + rel;
            }
        }

        // Restore live operand temps saved at suspension, then push the
        // resume argument on top (the post-Yield continuation consumes it).
        for v in saved_stack {
            self.push(v);
        }
        // Only Next carries a sent value; Throw/Return inject their
        // completion below instead of pushing an operand.
        let sent = match resume {
            GeneratorResume::Next(v) => Some(v),
            _ => None,
        };
        if started {
            if let Some(v) = sent {
                self.push(v);
            }
        }
        self.generators[gen_id].started = true;

        // Throw/Return modes inject an abrupt completion at the suspend
        // point instead of a sent value. handle_throw unwinds through the
        // generator's own (swapped-in) try frames, so in-generator
        // catch/finally blocks run naturally; if nothing handles it, the
        // Throw exit propagates out.
        //
        // If suspended INSIDE a finally region already (suspend pc strictly
        // past that entry's finally_pc), the new abrupt preempts the rest of
        // that finally — drop such entries first so the kick doesn't
        // restart them from the top (which re-yields forever or duplicates
        // effects). Being inside a catch region is already marked by
        // in_catch and handled by the normal branches.
        if !matches!(resume, GeneratorResume::Next(_)) {
            while let Some(tf) = self.try_stack.last() {
                if !tf.in_catch && tf.finally_pc != 0 && pc > tf.finally_pc {
                    self.try_stack.pop();
                } else {
                    break;
                }
            }
        }
        self.throw_routed_to_catch = false;
        let injected = match resume {
            GeneratorResume::Next(_) => None,
            GeneratorResume::Throw(e) => self.handle_throw(gc, e),
            GeneratorResume::Return(_) => {
                let sentinel =
                    Value::from_heap_ptr(crate::vm::heap_string(gc, "\0__rune_gen_return__\0"));
                self.generator_return_sentinel = Some(sentinel.raw());
                self.generator_return_value = Some(match resume {
                    GeneratorResume::Return(v) => v,
                    _ => unreachable!(),
                });
                self.handle_throw(gc, sentinel)
            }
        };
        let exit = match injected {
            Some(exit) => exit,
            None => self.run_loop(gc),
        };
        self.return_frame_floor = saved_floor;
        self.nested_gen_floor = saved_gen_floor;
        // Discard any leftover generator-side try frames (Yield banked the
        // live ones already) and restore the caller's stack.
        let _ = std::mem::take(&mut self.try_stack);
        self.try_stack = caller_try;
        self.generators[gen_id].executing = false;
        match exit {
            Exit::Return(v) => {
                self.generators[gen_id].done = true;
                Ok((v, true))
            }
            Exit::Yield(v) => {
                if is_abrupt && !self.throw_routed_to_catch {
                    if !matches!(resume, GeneratorResume::Next(_)) {
                        self.generators[gen_id].abrupt = Some(resume);
                    }
                }
                self.throw_routed_to_catch = false;
                Ok((v, false))
            }
            // Abrupt completion (uncaught throw) completes the generator —
            // otherwise the stale suspended pc would re-execute the throwing
            // call on the next resume.
            Exit::Throw(v) => {
                self.generators[gen_id].done = true;
                // A fully-unwound return sentinel converts to the stashed
                // return completion (finally blocks already ran).
                if self.generator_return_sentinel.is_some_and(|s| s == v.raw()) {
                    self.generator_return_sentinel = None;
                    let rv = self.generator_return_value.take().unwrap_or(v);
                    Ok((rv, true))
                } else {
                    Err(v)
                }
            }
        }
    }

    /// A3: sloppy `.caller` value — the executing function object of the
    /// caller frame, or null at top level / when unavailable. Strictness of
    /// the caller is checked by the caller (poison rule).
    fn stack_caller(&self) -> Value {
        if self.frames.len() < 2 {
            return Value::null();
        }
        let caller_fp = self.frames[self.frames.len() - 2].func_ptr;
        if caller_fp.is_null() {
            return Value::null();
        }
        Value::from_heap_ptr(caller_fp)
    }

    /// Whether the executing caller frame runs strict code (A3 poison rule:
    /// reading sloppy `.caller` throws when the caller is strict). Unknown
    /// / non-function callers count as sloppy.
    fn caller_is_strict(&self) -> bool {
        if self.frames.len() < 2 {
            return false;
        }
        let caller_fp = self.frames[self.frames.len() - 2].func_ptr;
        if caller_fp.is_null() {
            return false;
        }
        unsafe {
            (*(caller_fp as *const GcHeader)).tag() == TAG_FUNC
                && Func::is_strict(caller_fp as *mut Func)
        }
    }

    /// F3: single choke point for interpreter property READS (GetValue).
    /// `Opcode::LoadProperty` and the `LoadPropertyIC` miss path both funnel
    /// here (the IC fast guard stays inline in the arm for speed). Moved
    /// verbatim from the LoadProperty arm — behavior identical.
    ///
    /// A1: the RequireObjectCoercible null/undefined check below is the
    /// single place all read paths throw from (B7 Proxy trap goes above it).
    ///
    /// Returns `Ready(v)` (the arm pushes `v` and advances), `Wait` (getter
    /// pushed — arm must `continue`), or `Bail` (abrupt — propagate/continue
    /// per the payload).
    pub(crate) fn vm_get_property(
        &mut self,
        gc: &mut SemiSpace,
        obj: Value,
        raw_key: Value,
        instr: &Instruction,
    ) -> PropGetOut {
        // A1: RequireObjectCoercible (§7.2.1) — property reads on null /
        // undefined throw a catchable TypeError (routed through handle_throw
        // so in-frame try/catch + assert.throws observe it).
        if obj.is_null() || obj.is_undefined() {
            let msg = null_prop_message(false, obj.is_null(), raw_key);
            let err = crate::errors::error_object(
                gc,
                &self.error_protos,
                crate::errors::ErrorKind::TypeError,
                &msg,
            );
            return match self.handle_throw(gc, err) {
                Some(exit) => PropGetOut::Bail(Some(exit)),
                None => PropGetOut::Bail(None),
            };
        }
        // A3: strict-function poison pills + sloppy .caller (§15.3.5.4).
        // Own data props shadow (a sloppy assignment created them) — only
        // consulted when absent. Strict functions always throw; sloppy
        // .caller resolves to the executing caller function (or null at top
        // level); sloppy .arguments keeps legacy behavior (undefined — mapped
        // arguments objects are future work).
        if let Some(fptr) = obj.heap_ptr() {
            if unsafe { (*(fptr as *const GcHeader)).tag() } == TAG_FUNC
                && (prop_key_is(raw_key, "caller") || prop_key_is(raw_key, "arguments"))
            {
                let is_caller = prop_key_is(raw_key, "caller");
                let has_own = unsafe {
                    let ep = Func::extra_props(fptr as *mut Func);
                    !ep.is_null()
                        && JSObject::shape_ptr(ep as *mut JSObject)
                            .lookup(&PropertyKey::from_string(if is_caller {
                                "caller"
                            } else {
                                "arguments"
                            }))
                            .is_some()
                };
                let strict = unsafe { Func::is_strict(fptr as *mut Func) };
                // Strict function itself → always throws (unless shadowed by
                // an own data prop, checked above).
                // Sloppy .caller → throws when the CALLER is strict
                // (§15.3.5.4 step 3), else resolves to the caller or null.
                let caller_strict = !strict && is_caller && self.caller_is_strict();
                if (strict || caller_strict) && !has_own {
                    let msg = if strict {
                        format!(
                            "Cannot access '{}' of a strict function",
                            if is_caller { "caller" } else { "arguments" }
                        )
                    } else {
                        "Cannot access caller of a strict function".to_string()
                    };
                    let err = crate::errors::error_object(
                        gc,
                        &self.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        &msg,
                    );
                    return match self.handle_throw(gc, err) {
                        Some(exit) => PropGetOut::Bail(Some(exit)),
                        None => PropGetOut::Bail(None),
                    };
                }
                if is_caller && !has_own && !strict {
                    return PropGetOut::Ready(self.stack_caller());
                }
            }
        }
        let result = if obj.is_heap_object() {
            let tag = {
                let ptr = obj.heap_ptr().unwrap();
                unsafe { (*(ptr as *const GcHeader)).tag() }
            };
            if tag == TAG_STRING || tag == TAG_STRING_OBJ {
                let string_ptr = if tag == TAG_STRING {
                    obj.heap_ptr().unwrap()
                } else {
                    unsafe {
                        StringObject::string_ptr(obj.heap_ptr().unwrap() as *mut StringObject)
                    }
                };
                // String property access (both primitive and wrapper)
                if let Some(index) = value_to_array_index(raw_key) {
                    // Numeric index: return character at index
                    let s = unsafe { HeapString::to_string(string_ptr as *mut HeapString) };
                    let ch = s.chars().nth(index);
                    match ch {
                        Some(c) => {
                            let result_s = HeapString::allocate(gc, &c.to_string());
                            Value::from_heap_ptr(result_s as *mut u8)
                        }
                        None => Value::undefined(),
                    }
                } else if let Some(ptr) = raw_key.heap_ptr() {
                    let key_tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                    if key_tag == TAG_STRING {
                        let key_str = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                        if key_str == "length" {
                            // String length
                            let s = unsafe { HeapString::to_string(string_ptr as *mut HeapString) };
                            let len = s.encode_utf16().count();
                            Value::smi(len as i32)
                        } else if self.string_prototype.is_heap_object() {
                            // Look up from String.prototype
                            if let Some(proto_ptr) = self.string_prototype.heap_ptr() {
                                let proto_key = PropertyKey::from_string(&key_str);
                                let shape =
                                    unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
                                if let Some(slot) = shape.lookup(&proto_key) {
                                    unsafe { JSObject::get_slot(proto_ptr as *mut JSObject, slot) }
                                } else {
                                    Value::undefined()
                                }
                            } else {
                                Value::undefined()
                            }
                        } else {
                            Value::undefined()
                        }
                    } else {
                        Value::undefined()
                    }
                } else if let Some(sym_id) = raw_key.as_symbol_id() {
                    // Symbol key on a string receiver → String.prototype
                    // (e.g. `'abc'[Symbol.iterator]`).
                    if self.string_prototype.is_heap_object() {
                        if let Some(proto_ptr) = self.string_prototype.heap_ptr() {
                            let proto_key = PropertyKey::from_symbol(sym_id);
                            let shape = unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
                            if let Some(slot) = shape.lookup(&proto_key) {
                                unsafe { JSObject::get_slot(proto_ptr as *mut JSObject, slot) }
                            } else {
                                Value::undefined()
                            }
                        } else {
                            Value::undefined()
                        }
                    } else {
                        Value::undefined()
                    }
                } else {
                    Value::undefined()
                }
            } else if tag == TAG_ARRAY
                || tag == TAG_OBJECT
                || tag == TAG_FUNC
                || tag == TAG_REGEXP
                || tag == TAG_PROMISE
                || tag == TAG_STRING_OBJ
                || tag == TAG_MAP
                || tag == TAG_SET
                || tag == TAG_DATE
                || tag == TAG_TYPED_ARRAY
                || tag == TAG_ARRAY_BUFFER
            {
                if instr.ic_index >= 0 {
                    self.ic_stats.lookups += 1;
                    let hits_before = self.ic_stats.hits;
                    let result = load_property_recursive_ic(
                        gc,
                        &mut self.ics,
                        &mut self.ic_entries,
                        &mut self.ic_hit_counts,
                        &mut self.ic_stats,
                        instr,
                        obj,
                        raw_key,
                        Some(self.function_prototype),
                    );
                    if self.ic_stats.hits == hits_before {
                        self.ic_stats.misses += 1;
                    }
                    result
                } else {
                    load_property_recursive(obj, raw_key, Some(self.function_prototype), gc)
                }
            } else {
                Value::undefined()
            }
        } else if let Some(smi) = obj.as_smi() {
            if smi < 0 {
                // Negative Smi = builtin handle — expose name/length
                // metadata (Function.prototype fallback for the rest).
                let id = ((-smi) as usize) - 1;
                if id < self.builtins.len() {
                    let key_str = match raw_key.heap_ptr() {
                        Some(ptr) => {
                            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                            if tag == TAG_STRING {
                                Some(unsafe { HeapString::to_string(ptr as *mut HeapString) })
                            } else {
                                None
                            }
                        }
                        None => None,
                    };
                    if let Some(k) = key_str {
                        if k == "name" {
                            let hs = HeapString::allocate(gc, self.builtins[id].name) as *mut u8;
                            return PropGetOut::Ready(Value::from_heap_ptr(hs));
                        } else if k == "length" {
                            return PropGetOut::Ready(Value::smi(self.builtins[id].length as i32));
                        }
                    }
                }
                // Negative Smi = builtin handle — check Function.prototype
                if self.function_prototype.is_heap_object() {
                    load_property_recursive(
                        self.function_prototype,
                        raw_key,
                        Some(self.function_prototype),
                        gc,
                    )
                } else {
                    Value::undefined()
                }
            } else {
                Value::undefined()
            }
        } else if obj.is_symbol() {
            // Symbol property access — Symbol.prototype plus the
            // per-symbol `description` (computed from the registry).
            if let Some(ptr) = raw_key.heap_ptr() {
                let key_tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                if key_tag == TAG_STRING {
                    let key_str = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                    if key_str == "description" {
                        match obj.as_symbol_id().and_then(symbol_description) {
                            Some(d) => {
                                let hs = HeapString::allocate(gc, &d) as *mut u8;
                                Value::from_heap_ptr(hs)
                            }
                            None => Value::undefined(),
                        }
                    } else if self.symbol_prototype.is_heap_object() {
                        if let Some(proto_ptr) = self.symbol_prototype.heap_ptr() {
                            let proto_key = PropertyKey::from_string(&key_str);
                            let shape = unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
                            if let Some(slot) = shape.lookup(&proto_key) {
                                unsafe { JSObject::get_slot(proto_ptr as *mut JSObject, slot) }
                            } else {
                                Value::undefined()
                            }
                        } else {
                            Value::undefined()
                        }
                    } else {
                        Value::undefined()
                    }
                } else {
                    Value::undefined()
                }
            } else {
                Value::undefined()
            }
        } else if obj.is_boolean() {
            // Boolean primitive property access — boxed semantics:
            // the value is an object with [[Prototype]] =
            // %Object.prototype% (no %Boolean.prototype% in the
            // engine), so Object.prototype methods resolve.
            load_property_recursive(
                self.object_prototype,
                raw_key,
                Some(self.function_prototype),
                gc,
            )
        } else if obj.as_float64().is_some() {
            // Number primitive — same boxed-object semantics
            // (Number.prototype is not implemented, so only
            // Object.prototype methods resolve).
            load_property_recursive(
                self.object_prototype,
                raw_key,
                Some(self.function_prototype),
                gc,
            )
        } else {
            Value::undefined()
        };
        let (result, pushed_getter) = self.resolve_accessor_for_read(result, obj, gc);
        if pushed_getter {
            // Getter frame pushed; the Return handler resumes this
            // opcode with the getter's result.
            return PropGetOut::Wait;
        }
        PropGetOut::Ready(result)
    }

    /// F3: single choke point for interpreter property WRITES (SetValue).
    /// `Opcode::StoreProperty` and the `StorePropertyIC` miss path both
    /// funnel here (the IC fast guard stays inline in the arm for speed).
    /// Moved verbatim from the StoreProperty arm — behavior identical,
    /// including the setter scan, the 8-hit IC-patch counting, and the
    /// getter-only skip path.
    ///
    /// Insertion points (one each, by design):
    /// - A1 (RequireObjectCoercible): null/undefined receiver check below is
    ///   the single place all write paths throw from. DONE.
    /// - B7 (Proxy): the exotic-set trap dispatches at the top (same heap-tag
    ///   rule as reads — Proxies never match TAG_OBJECT shape guards).
    ///
    /// Returns `Ready(v)` (the arm pushes `v` and advances), `Wait` (setter
    /// pushed — arm must `continue 'run`), or `Bail` (abrupt — propagate /
    /// continue per the payload).
    pub(crate) fn vm_set_property(
        &mut self,
        gc: &mut SemiSpace,
        obj: Value,
        raw_key: Value,
        value: Value,
        site: PropSetSite,
    ) -> PropSetOut {
        // A1: RequireObjectCoercible (§7.2.1) — property writes on null /
        // undefined throw a catchable TypeError (same routing as reads).
        if obj.is_null() || obj.is_undefined() {
            let msg = null_prop_message(true, obj.is_null(), raw_key);
            let err = crate::errors::error_object(
                gc,
                &self.error_protos,
                crate::errors::ErrorKind::TypeError,
                &msg,
            );
            return match self.handle_throw(gc, err) {
                Some(exit) => PropSetOut::Bail(Some(exit)),
                None => PropSetOut::Bail(None),
            };
        }
        // A3: strict-function poison pills on write (sloppy writes fall
        // through to the normal store, creating shadowing own props).
        if let Some(fptr) = obj.heap_ptr() {
            if unsafe { (*(fptr as *const GcHeader)).tag() } == TAG_FUNC
                && (prop_key_is(raw_key, "caller") || prop_key_is(raw_key, "arguments"))
                && unsafe { Func::is_strict(fptr as *mut Func) }
            {
                let err = crate::errors::error_object(
                    gc,
                    &self.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "Cannot assign to 'caller'/'arguments' of a strict function",
                );
                return match self.handle_throw(gc, err) {
                    Some(exit) => PropSetOut::Bail(Some(exit)),
                    None => PropSetOut::Bail(None),
                };
            }
        }
        // Check for accessor setter on own or prototype chain.
        // B1e: dense arrays participate — the own slot is the accessor
        // overlay for numeric indices (named extra_props otherwise), then
        // the prototype chain is walked exactly like objects.
        if let Some(ptr) = obj.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_OBJECT || tag == TAG_ARRAY {
                let mut search_ptr = ptr;
                loop {
                    let search_tag = unsafe { (*(search_ptr as *const GcHeader)).tag() };
                    // B1e: only objects and dense arrays participate (anything
                    // else on the chain ends the scan — reading shapes out of
                    // exotic layouts is unsound).
                    let found: Option<Value> = if search_tag == TAG_ARRAY {
                        if let Some(index) = value_to_array_index(raw_key) {
                            array_overlay_accessor(search_ptr as *mut RuneArray, index)
                        } else if let Some(key) = value_to_prop_key(raw_key) {
                            let extra =
                                unsafe { RuneArray::extra_props(search_ptr as *mut RuneArray) };
                            if extra.is_null() {
                                None
                            } else {
                                let eshape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                                eshape.lookup(&key).map(|slot| unsafe {
                                    JSObject::get_slot(extra as *mut JSObject, slot)
                                })
                            }
                        } else {
                            None
                        }
                    } else if search_tag == TAG_OBJECT {
                        value_to_prop_key(raw_key).and_then(|key| {
                            let search_shape =
                                unsafe { JSObject::shape_ptr(search_ptr as *mut JSObject) };
                            search_shape.lookup(&key).map(|slot| unsafe {
                                JSObject::get_slot(search_ptr as *mut JSObject, slot)
                            })
                        })
                    } else {
                        None
                    };
                    if let Some(val) = found {
                        if val.is_heap_object() {
                            if let Some(vptr) = val.heap_ptr() {
                                if unsafe { (*(vptr as *const GcHeader)).tag() } == TAG_ACCESSOR {
                                    let setter = unsafe { AccessorPair::setter(vptr) };
                                    if !setter.is_undefined() {
                                        if let Some(sptr) = setter.heap_ptr() {
                                            if unsafe { (*(sptr as *const GcHeader)).tag() }
                                                == TAG_FUNC
                                            {
                                                self.pending_accessor_call =
                                                    Some(PendingAccessorCall {
                                                        source_frame_depth: self.frames.len(),
                                                        is_getter: false,
                                                    });
                                                let func_ptr = sptr;
                                                let func_idx = unsafe {
                                                    Func::func_index(func_ptr as *mut Func)
                                                }
                                                    as usize;
                                                let creator_prog = unsafe {
                                                    &*(Func::prog_ptr(func_ptr as *mut Func)
                                                        as *const BytecodeProgram)
                                                };
                                                if func_idx < creator_prog.functions.len() {
                                                    let func_prog =
                                                        &creator_prog.functions[func_idx];
                                                    let func_env = unsafe {
                                                        Func::env_ptr(func_ptr as *mut Func)
                                                    };
                                                    let mut locals = if func_prog.named_function {
                                                        vec![setter]
                                                    } else {
                                                        vec![]
                                                    };
                                                    locals.push(value);
                                                    self.frames.push(Frame {
                                                        locals,
                                                        lexical_slots: Vec::new(),
                                                        lexical_tdz: Vec::new(),
                                                        lexical_const: Vec::new(),
                                                        scope_boundaries: Vec::new(),
                                                        passed_argc: 1,
                                                        pc: 0,
                                                        stack_base: self.stack.len(),
                                                        prog: func_prog as *const BytecodeProgram,
                                                        generator_id: None,
                                                        this: obj,
                                                        is_constructor_call: false,
                                                        constructed_object: Value::undefined(),
                                                        env: func_env,
                                                        func_ptr,
                                                        private_name_ids: std::ptr::null_mut(),
                                                    });
                                                    return PropSetOut::Wait;
                                                }
                                            }
                                        }
                                    }
                                    // Accessor with no setter (getter-only): skip store (spec: return false)
                                    return PropSetOut::Ready(Value::undefined());
                                }
                            }
                        }
                    }
                    // Walk to prototype
                    let proto = unsafe { JSObject::prototype(search_ptr as *mut JSObject) };
                    if proto.is_null() {
                        break;
                    }
                    search_ptr = proto;
                }
            }
        }
        // IC hit counting: track successful own-property writes for patching
        if let Some(ptr) = obj.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_OBJECT && !is_proto_key(raw_key) {
                if let Some(key) = value_to_prop_key(raw_key) {
                    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                    if let Some(slot) = shape.lookup(&key) {
                        let ic_idx = site.ic_index as usize;
                        if ic_idx < self.ic_hit_counts.len() && self.ic_hit_counts[ic_idx] < 8 {
                            self.ic_hit_counts[ic_idx] += 1;
                            if self.ic_hit_counts[ic_idx] == 8 {
                                let instr_mut = unsafe {
                                    let instrs_ptr =
                                        (*site.prog_ptr).instructions.as_ptr() as *mut Instruction;
                                    &mut *instrs_ptr.add(site.pc)
                                };
                                instr_mut.opcode = Opcode::StorePropertyIC;
                                instr_mut.operands.clear();
                                instr_mut.operands.extend_from_slice(&[
                                    shape.id as i64,
                                    slot as i64,
                                    0,
                                ]);
                            }
                        }
                    }
                }
            }
        }
        if do_store_property(obj, raw_key, value, gc, self) {
            PropSetOut::Ready(value)
        } else if let Some(exc) = self.pending_exception.take() {
            // B1f-5: do_store stashed a precise error (array-length
            // RangeError) — prefer it over the generic strict TypeError below.
            // Only the array-"length" path stashes (all other do_store
            // callers use non-"length" keys), so no misattribution.
            match self.handle_throw(gc, exc) {
                Some(exit) => PropSetOut::Bail(Some(exit)),
                None => PropSetOut::Bail(None),
            }
        } else if site.is_strict {
            // Strict-mode [[Set]] failure throws (§10.1.8.1);
            // sloppy failures silently keep the RHS value.
            let err = crate::errors::error_object(
                gc,
                &self.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Cannot assign to read-only property",
            );
            match self.handle_throw(gc, err) {
                Some(exit) => PropSetOut::Bail(Some(exit)),
                None => PropSetOut::Bail(None),
            }
        } else {
            PropSetOut::Ready(value)
        }
    }

    /// Complete a ToPrimitive-string resume: stringify a primitive result.
    /// Symbol results throw (ToString abrupt). Returns Some(exit) to propagate.
    fn complete_toprim_string(
        &mut self,
        gc: &mut SemiSpace,
        result: Value,
        callee_base: usize,
    ) -> Option<Exit> {
        if result.is_symbol() {
            let err = crate::errors::error_object(
                gc,
                &self.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Cannot convert a Symbol value to a string",
            );
            return self.handle_throw(gc, err);
        }
        let s = crate::builtins::value_to_js_string(result);
        let ptr = HeapString::allocate(gc, &s);
        self.stack.truncate(callee_base);
        self.push(Value::from_heap_ptr(ptr as *mut u8));
        let caller_idx = self.frames.len() - 1;
        self.frames[caller_idx].pc += 1;
        None
    }

    /// Ensure a JSObject has room for one more property, growing (with root
    /// updates + forwarding) when full. Returns the live pointer; callers
    /// must refresh any locals holding the old address (same discipline as
    /// array grow). Resolves GC-forwarded pointers on entry.
    pub(crate) fn ensure_object_capacity(
        &mut self,
        gc: &mut SemiSpace,
        obj: *mut JSObject,
    ) -> *mut JSObject {
        unsafe {
            let live = {
                let h = obj as *const GcHeader;
                if (*h).is_forwarded() {
                    (*h).forwarding_addr() as *mut JSObject
                } else {
                    obj
                }
            };
            if JSObject::slot_count(live) < JSObject::capacity(live) {
                return live;
            }
            let (old, new_obj) = JSObject::grow(gc, live);
            if old != new_obj as *mut u8 {
                self.update_heap_reference(old, new_obj as *mut u8);
            }
            new_obj
        }
    }

    /// Whether the currently executing function is strict (A4: strict-mode
    /// [[Set]] failures throw instead of silently ignoring). Unknown callers
    /// (null func_ptr: top level) count as sloppy.
    fn executing_is_strict(&self, fi: usize) -> bool {
        let fp = self.frames[fi].func_ptr;
        !fp.is_null()
            && unsafe { (*(fp as *const GcHeader)).tag() } == TAG_FUNC
            && unsafe { Func::is_strict(fp as *mut Func) }
    }
    pub fn run_loop(&mut self, gc: &mut SemiSpace) -> Exit {
        'run: loop {
            let fi = self.frames.len() - 1;
            let pc = self.frames[fi].pc;
            let prog_ptr = self.frames[fi].prog;
            let prog = unsafe { &*prog_ptr };

            if pc >= prog.instructions.len() {
                break;
            }

            let instr = prog.instructions[pc].clone();

            // Trace recording: capture opcodes while recording a hot loop
            if let Some(key @ (rec_prog, target_pc)) = self.recording_trace {
                // Cross-program guard: if this instruction belongs to a
                // different program than the loop being recorded (a Call
                // descended into a callee, or a Return popped back to the
                // caller), the trace would mix opcodes from two programs
                // whose pcs collide.  Discard the partial trace.
                if prog_ptr as usize != rec_prog {
                    self.recording_trace = None;
                    self.loop_traces.remove(&key);
                    self.frames[fi].pc = pc;
                    continue;
                }
                if let Some(trace) = self.loop_traces.get_mut(&key) {
                    // Cross-loop guard: if this instruction is a Jump whose target
                    // is a different loop (present in loop_counts), the trace would
                    // cross loop boundaries.  The subsequent compile_trace_native
                    // cannot remap the inner-loop Jump target correctly — it would
                    // either exit the trace prematurely or jump to a wrong index.
                    // Stop recording and discard the partial trace.
                    if matches!(
                        instr.opcode,
                        Opcode::Jump | Opcode::JumpIfTrue | Opcode::JumpIfFalse
                    ) {
                        let jump_target = instr.operands.first().copied().unwrap_or(0) as usize;
                        if jump_target != target_pc
                            && self.loop_counts.contains_key(&(rec_prog, jump_target))
                        {
                            self.recording_trace = None;
                            self.loop_traces.remove(&key);
                            self.frames[fi].pc = pc;
                            continue;
                        }
                    }
                    if trace.ops.len() < 200 {
                        trace.ops.push(TraceOp {
                            opcode: instr.opcode as u8,
                            operands: instr.operands.clone(),
                            original_pc: pc,
                            shape_id: 0,
                            cost: 1,
                            ic_index: instr.ic_index,
                        });
                    }
                    // Stop recording when we've looped back to the target
                    if pc == target_pc && trace.ops.len() > 1 {
                        self.recording_trace = None;
                        #[cfg(all(feature = "jit", target_arch = "aarch64"))]
                        {
                            // Check if trace contains callee ops (Call followed by
                            // callee-body ops ending in Return).  If so, compiling
                            // the trace would produce JIT code where the callee's
                            // Return opcode exits the trace early.  Remove the
                            // trace to prevent the bug.
                            let mut has_callee_return = false;
                            let mut in_callee = false;
                            for op in &trace.ops {
                                if op.opcode == Opcode::Call as u8
                                    || op.opcode == Opcode::CallFromArray as u8
                                {
                                    in_callee = true;
                                } else if in_callee && op.opcode == Opcode::Return as u8 {
                                    has_callee_return = true;
                                    break;
                                }
                            }
                            if has_callee_return {
                                // Trace crosses a function-call boundary; inlining
                                // not yet supported — prevent compilation of buggy trace.
                                self.loop_traces.remove(&key);
                                self.loop_counts.remove(&key);
                            } else {
                                // Validate the trace actually contains the loop's
                                // back-edge Jump (target == target_pc). If recording
                                // started on the loop's FINAL iteration, the trace
                                // holds only the exit path + prologue of the next
                                // call and no back-edge — compiling it produces a
                                // straight-line trace that exits immediately and
                                // returns a premature result. Discard and re-record
                                // on the next back-edge instead.
                                let mut has_back_edge = false;
                                for op in &trace.ops {
                                    if op.opcode == Opcode::Jump as u8 {
                                        let t = op.operands.first().copied().unwrap_or(0) as usize;
                                        if t == target_pc {
                                            has_back_edge = true;
                                            break;
                                        }
                                    }
                                }
                                if has_back_edge {
                                    self.compile_trace_native(prog_ptr, target_pc);
                                } else {
                                    self.loop_traces.remove(&key);
                                    self.pending_rerecord.insert(key);
                                }
                            }
                        }
                    }
                }
            }

            match instr.opcode {
                // ---- Literals ----
                Opcode::LoadSmi => {
                    let val = instr.operands[0] as i32;
                    self.push(Value::smi(val));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadUndefined => {
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadNull => {
                    self.push(Value::null());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadBoolean => {
                    let val = instr.operands[0] != 0;
                    self.push(Value::boolean(val));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadString => {
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadStringConst => {
                    let idx = instr.operands[0] as usize;
                    let cache_key = prog_ptr as usize;
                    // Look up or allocate cached string handle
                    let val = if let Some(handles) = self.string_cache.get_mut(&cache_key) {
                        if let Some(v) = handles.get(idx) {
                            if v.is_undefined() {
                                let s = prog.string_pool.get(idx).map(|s| s.as_str()).unwrap_or("");
                                let ptr = HeapString::allocate(gc, s);
                                let new_val = Value::from_heap_ptr(ptr as *mut u8);
                                handles[idx] = new_val;
                                new_val
                            } else {
                                *v
                            }
                        } else {
                            let s = prog.string_pool.get(idx).map(|s| s.as_str()).unwrap_or("");
                            let ptr = HeapString::allocate(gc, s);
                            Value::from_heap_ptr(ptr as *mut u8)
                        }
                    } else {
                        let mut handles = vec![Value::undefined(); prog.string_pool.len()];
                        let s = prog.string_pool.get(idx).map(|s| s.as_str()).unwrap_or("");
                        let ptr = HeapString::allocate(gc, s);
                        let new_val = Value::from_heap_ptr(ptr as *mut u8);
                        handles[idx] = new_val;
                        self.string_cache.insert(cache_key, handles);
                        new_val
                    };
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadFloat64 => {
                    let idx = instr.operands[0] as usize;
                    let val = prog.float_pool.get(idx).copied().unwrap_or(0.0);
                    let is_int = val.fract() == 0.0 && val.is_finite();
                    if is_int {
                        let i = val as i64;
                        if i >= -(1 << 30) as i64 && i < (1 << 30) as i64 {
                            self.push(Value::smi(val as i32));
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    self.push(Value::from_float64(val));
                    self.frames[fi].pc = pc + 1;
                }

                Opcode::LoadRegExp => {
                    let idx = instr.operands[0] as usize;
                    let (pattern, flags) = prog.regex_pool.get(idx).cloned().unwrap_or_default();
                    let pattern_ptr = HeapString::allocate(gc, &pattern) as *mut u8;
                    let mut flag_bits = 0u32;
                    for c in flags.chars() {
                        match c {
                            'g' => flag_bits |= 1,
                            'i' => flag_bits |= 2,
                            'm' => flag_bits |= 4,
                            's' => flag_bits |= 8,
                            'u' => flag_bits |= 16,
                            'y' => flag_bits |= 32,
                            'd' => flag_bits |= 64,
                            _ => {}
                        }
                    }
                    let ptr = rune_core::regexp::RegExp::allocate(gc, pattern_ptr, flag_bits);
                    if let Some(proto_ptr) = self.regexp_prototype.heap_ptr() {
                        unsafe {
                            rune_core::regexp::RegExp::set_prototype(ptr, proto_ptr);
                        }
                    }
                    self.register_roots(gc);
                    self.push(Value::from_heap_ptr(ptr));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- `this` binding ----
                Opcode::LoadThis => {
                    self.push(self.frames[fi].this);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Locals ----
                Opcode::LoadLocal => {
                    let idx = instr.operands[0] as usize;
                    let val = if idx < self.frames[fi].locals.len() {
                        self.frames[fi].locals[idx]
                    } else {
                        Value::undefined()
                    };
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreLocal => {
                    let idx = instr.operands[0] as usize;
                    let val = self.pop();
                    if idx >= self.frames[fi].locals.len() {
                        self.frames[fi].locals.resize(idx + 1, Value::undefined());
                    }
                    self.frames[fi].locals[idx] = val;
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Stack ----
                Opcode::Pop => {
                    // Only pop if the stack is above this frame's base, so we
                    // don't steal an item belonging to a parent frame (this
                    // matters after StoreCaptured already consumed the value).
                    let stack_base = self.frames[fi].stack_base;
                    if self.stack.len() > stack_base {
                        self.stack.pop();
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Dup => {
                    let val = self.peek();
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Dup2 => {
                    let n = self.stack.len();
                    if n >= 2 {
                        let a = self.stack[n - 2];
                        let b = self.stack[n - 1];
                        self.push(a);
                        self.push(b);
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Swap => {
                    let a = self.pop();
                    let b = self.pop();
                    self.push(a);
                    self.push(b);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Unary ----
                Opcode::UnaryPlus => {
                    let a = self.pop();
                    // §13.5.3: Return ToNumber(UnaryExpression)
                    if a.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let n = to_number(a);
                    let result = number_result(gc, n);
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Neg => {
                    let a = self.pop();
                    let result = if let Some(v) = a.as_smi() {
                        if v == 0 {
                            // Preserve -0.0 per spec (§13.5.5)
                            Value::from_float64(-0.0f64)
                        } else if v == -(1 << 30) {
                            // Overflow: -(-2^30) = 2^30 doesn't fit in Smi
                            Value::from_float64(-(v as f64))
                        } else {
                            Value::smi(-v)
                        }
                    } else if let Some(v) = a.as_float64() {
                        Value::from_float64(-v)
                    } else {
                        if a.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let n = to_number(a);
                        Value::from_float64(-n)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Not => {
                    let a = self.pop();
                    self.push(if a.to_bool() {
                        Value::boolean(false)
                    } else {
                        Value::boolean(true)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitNot => {
                    let a = self.pop();
                    if a.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let n = to_int32(a);
                    // !n always fits in i32; use number_result for i31 safety
                    let result = number_result(gc, (!n) as f64);
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Void => {
                    self.pop();
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Binary ----
                Opcode::Add => {
                    let b = self.pop();
                    let a = self.pop();
                    // §12.8.5: ToPrimitive both operands (no hint → "default").
                    // §7.1.1: OrdinaryToPrimitive with "default" hint tries valueOf then toString.
                    // We use to_primitive_string with a string bias (acceptable simplification:
                    // for objects, toString is the common path. valueOf is tried if toString returns an object).
                    let a = match try_convert_object_to_string(a, gc, self) {
                        Ok(v) => v,
                        Err(()) => {
                            self.pending_primitive_conversion = Some(PendingPrimitiveConversion {
                                source_frame_depth: self.frame_depth() - 1,
                                other_operand: b,
                            });
                            continue;
                        }
                    };
                    let b = match try_convert_object_to_string(b, gc, self) {
                        Ok(v) => v,
                        Err(()) => {
                            self.pending_primitive_conversion = Some(PendingPrimitiveConversion {
                                source_frame_depth: self.frame_depth() - 1,
                                other_operand: a,
                            });
                            continue;
                        }
                    };
                    let a_is_str = value_is_string(a);
                    let b_is_str = value_is_string(b);
                    let result = if a_is_str || b_is_str {
                        // §7.1.12.1 ToString(Symbol) throws TypeError
                        if a.is_symbol() || b.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a string",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let sa = value_to_debug_string(a);
                        let sb = value_to_debug_string(b);
                        let combined = sa + &sb;
                        let ptr = HeapString::allocate(gc, &combined);
                        Value::from_heap_ptr(ptr as *mut u8)
                    } else {
                        if a.is_symbol() || b.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av + bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Sub => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if let Some(r) = av.checked_sub(bv) {
                            if (-(1 << 30)..(1 << 30)).contains(&r) {
                                Value::smi(r)
                            } else {
                                number_result(gc, av as f64 - bv as f64)
                            }
                        } else {
                            number_result(gc, av as f64 - bv as f64)
                        }
                    } else {
                        if a.is_symbol() || b.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av - bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Mul => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if let Some(r) = av.checked_mul(bv) {
                            if (-(1 << 30)..(1 << 30)).contains(&r) {
                                Value::smi(r)
                            } else {
                                number_result(gc, av as f64 * bv as f64)
                            }
                        } else {
                            number_result(gc, av as f64 * bv as f64)
                        }
                    } else {
                        if a.is_symbol() || b.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av * bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Div => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_symbol() || b.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let av = to_number(a);
                    let bv = to_number(b);
                    let result = number_result(gc, av / bv);
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Mod => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if bv == 0 {
                            number_result(gc, f64::NAN)
                        } else {
                            let r = av % bv;
                            if (-(1 << 30)..(1 << 30)).contains(&r) {
                                Value::smi(r)
                            } else {
                                number_result(gc, r as f64)
                            }
                        }
                    } else {
                        if a.is_symbol() || b.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av % bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Exp => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if bv < 0 {
                            number_result(gc, (av as f64).powf(bv as f64))
                        } else if let Some(r64) = (av as i64).checked_pow(bv as u32) {
                            // B1e: wrapping_pow destroyed overflow evidence
                            // (2**32 wrapped to 0, which passed the Smi-range
                            // check). checked i64 pow keeps it; out-of-Smi
                            // (or out-of-i64, via None) takes the float path.
                            if (-(1i64 << 30)..(1i64 << 30)).contains(&r64) {
                                Value::smi(r64 as i32)
                            } else {
                                number_result(gc, (av as f64).powf(bv as f64))
                            }
                        } else {
                            number_result(gc, (av as f64).powf(bv as f64))
                        }
                    } else {
                        if a.is_symbol() || b.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av.powf(bv))
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Bitwise ----
                Opcode::Shl => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_symbol() || b.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    let r = av.wrapping_shl(bv as u32);
                    if (-(1 << 30)..(1 << 30)).contains(&r) {
                        self.push(Value::smi(r));
                    } else {
                        self.push(number_result(gc, r as f64));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Shr => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_symbol() || b.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi(av.wrapping_shr(bv as u32)));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ShrU => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_symbol() || b.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    let r = (av as u32).wrapping_shr(bv as u32);
                    if r < (1 << 30) as u32 {
                        self.push(Value::smi(r as i32));
                    } else {
                        self.push(number_result(gc, r as f64));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitOr => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_symbol() || b.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    let r = av | bv;
                    if (-(1 << 30)..(1 << 30)).contains(&r) {
                        self.push(Value::smi(r));
                    } else {
                        self.push(number_result(gc, r as f64));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitXor => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_symbol() || b.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    let r = av ^ bv;
                    if (-(1 << 30)..(1 << 30)).contains(&r) {
                        self.push(Value::smi(r));
                    } else {
                        self.push(number_result(gc, r as f64));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitAnd => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_symbol() || b.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi(av & bv));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Logical ----
                // ---- Comparisons ----
                Opcode::Eq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if values_loosely_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StrictEq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if values_strictly_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Ne => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if !values_loosely_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StrictNe => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if !values_strictly_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Lt => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av < bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(a, b) {
                                Value::boolean(v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::undefined()
                                } else {
                                    Value::boolean(av < bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Gt => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av > bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(b, a) {
                                Value::boolean(v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::undefined()
                                } else {
                                    Value::boolean(av > bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Le => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av <= bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(a, b) {
                                Value::boolean(v)
                            } else if let Some(v) = compare_strings_lt(b, a) {
                                // Both are strings: if b < a then a <= b is false, else equal → true
                                Value::boolean(!v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::boolean(false)
                                } else {
                                    Value::boolean(av <= bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Ge => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av >= bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(b, a) {
                                Value::boolean(v)
                            } else if let Some(v) = compare_strings_lt(a, b) {
                                Value::boolean(!v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::boolean(false)
                                } else {
                                    Value::boolean(av >= bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::In => {
                    let obj = self.pop();
                    let key = self.pop();
                    let found = has_property(obj, key, Some(self.function_prototype));
                    self.push(Value::boolean(found));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Instanceof => {
                    // TODO: §13.10.3 — check rhs[Symbol.hasInstance] first when Symbol lands.
                    // If rhs has @@hasInstance, call it instead of OrdinaryHasInstance.
                    let rhs = self.pop();
                    let lhs = self.pop();
                    // §13.10.1: If Type(rhs) is not Object → TypeError
                    if !rhs.is_heap_object() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "invalid 'instanceof' operand (RHS is not an object)",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let rhs_ptr = rhs.heap_ptr().unwrap();
                    let rhs_tag = unsafe { (*(rhs_ptr as *const GcHeader)).tag() };
                    // §13.10.2 OrdinaryHasInstance: get rhs.prototype
                    let rhs_proto_ptr: *mut u8 = if rhs_tag == TAG_FUNC {
                        unsafe { Func::prototype(rhs_ptr as *mut Func) }
                    } else if rhs_tag == TAG_OBJECT {
                        // Builtin constructor wrappers (Array, String, Promise, etc.)
                        // are stored as TAG_OBJECT with a "prototype" property
                        let shape = unsafe { JSObject::shape_ptr(rhs_ptr as *mut JSObject) };
                        if let Some(slot) = shape.lookup(&PROTOTYPE_KEY) {
                            let proto_val =
                                unsafe { JSObject::get_slot(rhs_ptr as *mut JSObject, slot) };
                            proto_val.heap_ptr().unwrap_or(std::ptr::null_mut())
                        } else {
                            std::ptr::null_mut()
                        }
                    } else {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "RHS of 'instanceof' is not callable",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    };
                    if rhs_proto_ptr.is_null() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "function 'prototype' is not an object",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    // Walk lhs prototype chain
                    let result = ordinary_has_instance(lhs, rhs_proto_ptr);
                    self.push(Value::boolean(result));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Objects ----
                Opcode::NewObject => {
                    let count = instr.operands[0] as usize;
                    let mut values: Vec<Value> = (0..count).map(|_| self.pop()).collect();
                    values.reverse();
                    let mut entries: Vec<(PropertyKey, usize)> = Vec::with_capacity(count);
                    let mut key_names: Vec<String> = Vec::with_capacity(count);
                    for i in 0..count {
                        let key_idx = instr.operands[1 + i] as usize;
                        let key_str = self.frames[fi].prog_str(key_idx).unwrap_or_default();
                        entries.push((PropertyKey::from_string(&key_str), i));
                        key_names.push(key_str);
                    }
                    let shape = Shape::intern(entries, key_names);
                    let obj = JSObject::allocate(gc, shape, &values);
                    if self.object_prototype.is_heap_object() {
                        if let Some(proto_ptr) = self.object_prototype.heap_ptr() {
                            unsafe {
                                JSObject::set_prototype(obj, proto_ptr);
                            }
                        }
                    }
                    self.push(Value::from_heap_ptr(obj as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::NewArray => {
                    let elem_count = instr.operands[0] as usize;
                    let mut elems: Vec<Value> = (0..elem_count).map(|_| self.pop()).collect();
                    elems.reverse();
                    let arr = RuneArray::allocate(gc, &elems);
                    // Set the DENSE_ARRAY_SHAPE and Array.prototype on the newly allocated array
                    unsafe {
                        let ptr = arr as *mut u8;
                        let shape_ptr = ptr.add(8) as *mut *const Shape;
                        *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                        let proto_ptr = ptr.add(24) as *mut *mut u8;
                        if self.array_prototype.is_heap_object() {
                            if let Some(proto) = self.array_prototype.heap_ptr() {
                                *proto_ptr = proto;
                            }
                        }
                    }
                    self.push(Value::from_heap_ptr(arr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ArrayPush => {
                    let val = self.pop();
                    let arr_val = self.pop();
                    if let Some(heap) = arr_val.heap_ptr() {
                        let arr_ptr = heap as *mut RuneArray;
                        unsafe {
                            let new_arr = RuneArray::push(gc, arr_ptr, val);
                            self.push(Value::from_heap_ptr(new_arr as *mut u8));
                        }
                    } else {
                        self.push(crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "ArrayPush on non-array",
                        ));
                        return Exit::Throw(self.pop());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ArrayPushHole => {
                    // B1e: elision — push the empty sentinel (a real hole).
                    let arr_val = self.pop();
                    if let Some(heap) = arr_val.heap_ptr() {
                        let arr_ptr = heap as *mut RuneArray;
                        unsafe {
                            let new_arr = RuneArray::push(gc, arr_ptr, Value::empty_sentinel());
                            self.push(Value::from_heap_ptr(new_arr as *mut u8));
                        }
                    } else {
                        self.push(crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "ArrayPushHole on non-array",
                        ));
                        return Exit::Throw(self.pop());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ArrayExtend => {
                    let src_val = self.pop();
                    let tgt_val = self.pop();
                    if let (Some(src_heap), Some(tgt_heap)) =
                        (src_val.heap_ptr(), tgt_val.heap_ptr())
                    {
                        let src_arr = src_heap as *mut RuneArray;
                        let mut tgt_arr = tgt_heap as *mut RuneArray;
                        let src_len = unsafe { RuneArray::length(src_arr) };
                        for i in 0..src_len {
                            let elem = unsafe { RuneArray::get_element(src_arr, i as usize) };
                            // B1e: holes spread as undefined (Get semantics).
                            let elem = if elem == Value::empty_sentinel() {
                                Value::undefined()
                            } else {
                                elem
                            };
                            unsafe {
                                tgt_arr = RuneArray::push(gc, tgt_arr, elem);
                            }
                        }
                        self.push(Value::from_heap_ptr(tgt_arr as *mut u8));
                    } else {
                        self.push(crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "ArrayExtend on non-array",
                        ));
                        return Exit::Throw(self.pop());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ArraySlice => {
                    let start_idx = self.pop();
                    let arr_val = self.pop();
                    let start = start_idx.as_smi().unwrap_or(0) as usize;
                    if let Some(heap) = arr_val.heap_ptr() {
                        let tag = unsafe { (*(heap as *const GcHeader)).tag() };
                        if tag == TAG_ARRAY {
                            let arr_ptr = heap as *mut RuneArray;
                            let len = unsafe { RuneArray::length(arr_ptr) } as usize;
                            let slice_len = len.saturating_sub(start);
                            let mut elems: Vec<Value> = Vec::with_capacity(slice_len);
                            for i in start..len {
                                let v = unsafe { RuneArray::get_element(arr_ptr, i) };
                                elems.push(v);
                            }
                            let new_arr = RuneArray::allocate(gc, &elems);
                            // Set shape and prototype
                            unsafe {
                                let ptr = new_arr as *mut u8;
                                let shape_ptr = ptr.add(8) as *mut *const Shape;
                                *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                                let proto_ptr = ptr.add(24) as *mut *mut u8;
                                if self.array_prototype.is_heap_object() {
                                    if let Some(proto) = self.array_prototype.heap_ptr() {
                                        *proto_ptr = proto;
                                    }
                                }
                            }
                            self.push(Value::from_heap_ptr(new_arr as *mut u8));
                        } else {
                            self.push(Value::from_heap_ptr(heap));
                        }
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ToString => {
                    let val = self.pop();
                    let val = match try_convert_object_to_string(val, gc, self) {
                        Ok(v) => v,
                        Err(()) => {
                            self.pending_primitive_conversion = Some(PendingPrimitiveConversion {
                                source_frame_depth: self.frame_depth() - 1,
                                other_operand: Value::undefined(),
                            });
                            continue;
                        }
                    };
                    let s = value_to_js_string(val);
                    let ptr = HeapString::allocate(gc, &s);
                    self.push(Value::from_heap_ptr(ptr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StringConcat => {
                    let rhs = self.pop();
                    let lhs = self.pop();
                    let lhs = match try_convert_object_to_string(lhs, gc, self) {
                        Ok(v) => v,
                        Err(()) => {
                            self.pending_primitive_conversion = Some(PendingPrimitiveConversion {
                                source_frame_depth: self.frame_depth() - 1,
                                other_operand: rhs,
                            });
                            continue;
                        }
                    };
                    let rhs = match try_convert_object_to_string(rhs, gc, self) {
                        Ok(v) => v,
                        Err(()) => {
                            self.pending_primitive_conversion = Some(PendingPrimitiveConversion {
                                source_frame_depth: self.frame_depth() - 1,
                                other_operand: lhs,
                            });
                            continue;
                        }
                    };
                    let lhs_s = value_to_js_string(lhs);
                    let rhs_s = value_to_js_string(rhs);
                    let combined = lhs_s + &rhs_s;
                    let ptr = HeapString::allocate(gc, &combined);
                    self.push(Value::from_heap_ptr(ptr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ForInInit => {
                    let obj = self.pop();
                    if obj.is_null() || obj.is_undefined() {
                        self.push(Value::smi(0));
                    } else {
                        self.push(obj);
                        self.push(Value::smi(0));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ForInNext => {
                    let end_target = instr.operands[0] as usize;
                    let index_val = self.pop();
                    let index = index_val.as_smi().unwrap_or(0) as usize;
                    let obj = self.peek();
                    let done = if let Some(ptr) = obj.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        match tag {
                            TAG_ARRAY => {
                                let len =
                                    unsafe { RuneArray::length(ptr as *mut RuneArray) } as usize;
                                if index < len {
                                    let key_str = index.to_string();
                                    let key = HeapString::allocate(gc, &key_str);
                                    self.push(Value::smi((index + 1) as i32));
                                    self.push(Value::from_heap_ptr(key as *mut u8));
                                    false
                                } else {
                                    true
                                }
                            }
                            TAG_OBJECT => {
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                // §14.7.5.9: symbol-keyed properties are excluded
                                // from for-in enumeration. A4: so are
                                // non-enumerable own properties.
                                let mut idx = index;
                                while idx < shape.property_count
                                    && (shape.entries[idx].0.is_symbol()
                                        || shape.attr_at(idx) & rune_core::shape::ATTR_ENUMERABLE
                                            == 0)
                                {
                                    idx += 1;
                                }
                                if idx < shape.property_count {
                                    let key_name = shape.key_name_at(idx).unwrap_or("");
                                    let key = HeapString::allocate(gc, key_name);
                                    self.push(Value::smi((idx + 1) as i32));
                                    self.push(Value::from_heap_ptr(key as *mut u8));
                                    false
                                } else {
                                    true
                                }
                            }
                            _ => true,
                        }
                    } else {
                        true
                    };
                    if done {
                        self.pop(); // pop obj
                        self.frames[fi].pc = end_target;
                    } else {
                        self.frames[fi].pc = pc + 1;
                    }
                }
                Opcode::ForOfInit => {
                    // pop iterable → push [iterator, nextMethod]

                    let expr_val = self.pop();
                    match get_iter_method(self, gc, expr_val) {
                        SymbolMethodResult::NotFound => {
                            if let Some(exit) = self.throw_routed(gc, "value is not iterable") {
                                return exit;
                            }
                            continue;
                        }
                        SymbolMethodResult::NotCallable => {
                            if let Some(exit) =
                                self.throw_routed(gc, "value[Symbol.iterator] is not a function")
                            {
                                return exit;
                            }
                            continue;
                        }
                        SymbolMethodResult::Found(method) => {
                            if method.as_smi().is_some_and(|s| s < 0) {
                                let result =
                                    match call_builtin_sync(self, gc, method, expr_val, &[]) {
                                        Ok(v) => v,
                                        Err(Some(exit)) => return exit,
                                        Err(None) => continue,
                                    };
                                match complete_for_of_init(self, gc, result) {
                                    Ok(()) => {
                                        self.frames[fi].pc = pc + 1;
                                    }
                                    Err(exit) => return exit,
                                }
                            } else if method.is_heap_object() {
                                // User-defined @@iterator — call it via a callback.
                                self.pending_for_of_init = Some(PendingForOfInit {
                                    source_frame_depth: self.frames.len() - 1,
                                });
                                self.push_callback_call(gc, method, expr_val, vec![]);
                                // pc NOT advanced — the Return handler resumes.
                            } else {
                                if let Some(exit) = self
                                    .throw_routed(gc, "value[Symbol.iterator] is not a function")
                                {
                                    return exit;
                                }
                                continue;
                            }
                        }
                    }
                }
                Opcode::ForOfNext => {
                    let end_target = instr.operands[0] as usize;
                    let prefix = instr.operands[1] as usize;
                    let len = self.stack.len();
                    let next = self.stack[len - 1 - prefix];
                    let iter = self.stack[len - 2 - prefix];

                    if next.as_smi().is_some_and(|s| s < 0) {
                        let result = match call_builtin_sync(self, gc, next, iter, &[]) {
                            Ok(v) => v,
                            Err(Some(exit)) => return exit,
                            Err(None) => continue,
                        };
                        match process_for_of_next_result(self, gc, result, end_target) {
                            Ok(()) => continue,
                            Err(exit) => return exit,
                        }
                    } else if next.is_heap_object()
                        && unsafe { (*(next.heap_ptr().unwrap() as *const GcHeader)).tag() }
                            == TAG_FUNC
                    {
                        self.pending_for_of_next = Some(PendingForOfNext {
                            source_frame_depth: self.frames.len() - 1,
                            end_target,
                        });
                        self.push_callback_call(gc, next, iter, vec![]);
                        // pc NOT advanced — the Return handler resumes.
                    } else {
                        if let Some(exit) = self.throw_routed(gc, "iterator.next is not a function")
                        {
                            return exit;
                        }
                        continue;
                    }
                }
                Opcode::ForOfNextStar => {
                    // yield* delegation step: stack [..., iter, next, sent].
                    let end_target = instr.operands[0] as usize;
                    let sent = self.pop();
                    let len = self.stack.len();
                    let next = self.stack[len - 1];
                    let iter = self.stack[len - 2];
                    if next.as_smi().is_some_and(|s| s < 0) {
                        let result = match call_builtin_sync(self, gc, next, iter, &[sent]) {
                            Ok(v) => v,
                            Err(Some(exit)) => return exit,
                            Err(None) => continue,
                        };
                        match process_yieldstar_next_result(self, gc, result, end_target) {
                            Ok(()) => continue,
                            Err(exit) => return exit,
                        }
                    } else if next.is_heap_object()
                        && unsafe { (*(next.heap_ptr().unwrap() as *const GcHeader)).tag() }
                            == TAG_FUNC
                    {
                        self.pending_yield_star_next = Some(PendingYieldStarNext {
                            source_frame_depth: self.frames.len() - 1,
                            end_target,
                            gen_id: self.frames[fi].generator_id,
                        });
                        self.push_callback_call(gc, next, iter, vec![sent]);
                        // pc NOT advanced — the Return handler resumes.
                    } else {
                        if let Some(exit) = self.throw_routed(gc, "iterator.next is not a function")
                        {
                            return exit;
                        }
                        continue;
                    }
                }
                Opcode::ToArrayFromIterable => {
                    let x = self.pop();
                    if let Some(ptr) = x.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_ARRAY {
                            self.push(x);
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                        if tag == TAG_STRING {
                            // String spread: array of code point strings.
                            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                            let mut arr = new_dense_array(self, gc);
                            for ch in s.chars() {
                                let sp = HeapString::allocate(gc, &ch.to_string());
                                arr = unsafe {
                                    RuneArray::push(
                                        gc,
                                        arr as *mut RuneArray,
                                        Value::from_heap_ptr(sp as *mut u8),
                                    )
                                } as *mut u8;
                            }
                            self.push(Value::from_heap_ptr(arr));
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    // Generic iterable: @@iterator → drain.
                    match get_iter_method(self, gc, x) {
                        SymbolMethodResult::NotFound => {
                            if let Some(exit) = self.throw_routed(gc, "value is not iterable") {
                                return exit;
                            }
                            continue;
                        }
                        SymbolMethodResult::NotCallable => {
                            if let Some(exit) =
                                self.throw_routed(gc, "value[Symbol.iterator] is not a function")
                            {
                                return exit;
                            }
                            continue;
                        }
                        SymbolMethodResult::Found(method) => {
                            if method.as_smi().is_some_and(|s| s < 0) {
                                let result = match call_builtin_sync(self, gc, method, x, &[]) {
                                    Ok(v) => v,
                                    Err(Some(exit)) => return exit,
                                    Err(None) => continue,
                                };
                                let fresh_arr = new_dense_array(self, gc);
                                let arr = match drain_iterator(self, gc, result, x, fresh_arr) {
                                    Ok(v) => v,
                                    Err(Some(exit)) => return exit,
                                    Err(None) => continue,
                                };
                                self.push(Value::from_heap_ptr(arr));
                                self.frames[fi].pc = pc + 1;
                            } else if method.is_heap_object() {
                                self.pending_iter_drain = Some(PendingIterDrain {
                                    source_frame_depth: self.frames.len() - 1,
                                    state: IterDrainState::AwaitFactory,
                                    iter: Value::undefined(),
                                    next: Value::undefined(),
                                    result: new_dense_array(self, gc),
                                    receiver: x,
                                });
                                self.push_callback_call(gc, method, x, vec![]);
                                // pc NOT advanced — the Return handler resumes.
                            } else {
                                if let Some(exit) = self
                                    .throw_routed(gc, "value[Symbol.iterator] is not a function")
                                {
                                    return exit;
                                }
                                continue;
                            }
                        }
                    }
                }
                Opcode::LoadProperty => {
                    let raw_key = self.pop();
                    let obj = self.pop();
                    // F3: all reads funnel through vm_get_property.
                    match self.vm_get_property(gc, obj, raw_key, &instr) {
                        PropGetOut::Ready(v) => {
                            self.push(v);
                            self.frames[fi].pc = pc + 1;
                        }
                        PropGetOut::Wait => continue,
                        PropGetOut::Bail(exit) => match exit {
                            Some(e) => return e,
                            None => continue,
                        },
                    }
                }
                Opcode::LoadPropertyIC => {
                    // Shape-guarded fast path. Operands: [cached_shape_id, offset, proto_depth]
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let ic_idx = instr.ic_index as usize;
                    let cached_shape_id = instr.operands.first().copied().unwrap_or(0) as u64;
                    let offset = instr.operands.get(1).copied().unwrap_or(0) as usize;
                    let proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u8;

                    if ic_idx < self.ic_entries.len() {
                        if let Some(ptr) = obj.heap_ptr() {
                            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                            if tag == TAG_OBJECT {
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                if shape.id == cached_shape_id {
                                    // Record shape_id for trace analysis (fast path)
                                    if let Some(key) = self.recording_trace {
                                        if let Some(trace) = self.loop_traces.get_mut(&key) {
                                            if !trace.shape_ids.contains(&cached_shape_id) {
                                                trace.shape_ids.push(cached_shape_id);
                                            }
                                            if let Some(last) = trace.ops.last_mut() {
                                                last.shape_id = cached_shape_id;
                                            }
                                        }
                                    }
                                    // Shape guard passes — direct slot access
                                    self.ic_stats.lookups += 1;
                                    self.ic_stats.hits += 1;
                                    let val = if proto_depth == 0 {
                                        unsafe { JSObject::get_slot(ptr as *mut JSObject, offset) }
                                    } else {
                                        let mut p = ptr;
                                        for _ in 0..proto_depth {
                                            let next =
                                                unsafe { JSObject::prototype(p as *mut JSObject) };
                                            if next.is_null() {
                                                break;
                                            }
                                            p = next;
                                        }
                                        unsafe { JSObject::get_slot(p as *mut JSObject, offset) }
                                    };
                                    let (val, pushed_getter) =
                                        self.resolve_accessor_for_read(val, obj, gc);
                                    if pushed_getter {
                                        continue;
                                    }
                                    self.push(val);
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                            }
                        }
                    }
                    // Shape guard failed — F3: same funnel as LoadProperty
                    // (it performs the identical IC-table + full-dispatch
                    // sequence, including stats accounting).
                    match self.vm_get_property(gc, obj, raw_key, &instr) {
                        PropGetOut::Ready(v) => {
                            self.push(v);
                            self.frames[fi].pc = pc + 1;
                        }
                        PropGetOut::Wait => continue,
                        PropGetOut::Bail(exit) => match exit {
                            Some(e) => return e,
                            None => continue,
                        },
                    }
                }
                Opcode::StoreProperty => {
                    let value = self.pop();
                    let raw_key = self.pop();
                    let obj = self.pop();
                    // F3: all writes funnel through vm_set_property.
                    match self.vm_set_property(
                        gc,
                        obj,
                        raw_key,
                        value,
                        PropSetSite {
                            ic_index: instr.ic_index,
                            prog_ptr,
                            pc,
                            is_strict: self.executing_is_strict(fi),
                        },
                    ) {
                        PropSetOut::Ready(v) => {
                            self.push(v);
                            self.frames[fi].pc = pc + 1;
                        }
                        PropSetOut::Wait => continue 'run,
                        PropSetOut::Bail(exit) => match exit {
                            Some(e) => return e,
                            None => continue,
                        },
                    }
                }
                Opcode::StorePropertyIC => {
                    let value = self.pop();
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let cached_shape_id = instr.operands.first().copied().unwrap_or(0) as u64;
                    let offset = instr.operands.get(1).copied().unwrap_or(0) as usize;
                    if let Some(ptr) = obj.heap_ptr() {
                        // A4: the slot fast path is only sound for
                        // all-default shapes (any non-default attribute takes
                        // the checked funnel path below).
                        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT
                            && unsafe { JSObject::shape_ptr(ptr as *mut JSObject) }.id
                                == cached_shape_id
                            && unsafe { JSObject::shape_ptr(ptr as *mut JSObject) }
                                .all_default_attrs
                        {
                            unsafe { JSObject::set_slot(ptr as *mut JSObject, offset, value) };
                            self.push(value);
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    // Shape guard failed — F3: same funnel as StoreProperty.
                    // (Strictly, the old miss path called do_store_property
                    // directly, skipping the setter scan + patch counting;
                    // the funnel runs both. Verified byte-identical suite
                    // results in the F3 commit — any divergence would show
                    // in the Array/Object/class FAIL-list diffs.)
                    match self.vm_set_property(
                        gc,
                        obj,
                        raw_key,
                        value,
                        PropSetSite {
                            ic_index: instr.ic_index,
                            prog_ptr,
                            pc,
                            is_strict: self.executing_is_strict(fi),
                        },
                    ) {
                        PropSetOut::Ready(v) => {
                            self.push(v);
                            self.frames[fi].pc = pc + 1;
                        }
                        PropSetOut::Wait => continue 'run,
                        PropSetOut::Bail(exit) => match exit {
                            Some(e) => return e,
                            None => continue,
                        },
                    }
                }
                Opcode::DeleteProperty => {
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let result = if let Some(ptr) = obj.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT {
                            if let Some(key) = value_to_prop_key(raw_key) {
                                unsafe { JSObject::remove_property(ptr as *mut JSObject, &key) };
                            }
                            // B1f-5: object configurability enforcement stays
                            // A4-owned (remove_property is authoritative there).
                            Value::boolean(true)
                        } else if tag == TAG_ARRAY {
                            // B1f-5: configurability-checked delete (holes and
                            // missing keys succeed vacuously; locked indices
                            // and `length` fail — sloppy false, strict throw).
                            let mut arr_val = obj;
                            match crate::builtins::array_delete_key(gc, self, &mut arr_val, raw_key)
                            {
                                Ok(()) => Value::boolean(true),
                                Err(e) => {
                                    if self.executing_is_strict(fi) {
                                        if let Some(exit) = self.handle_throw(gc, e) {
                                            return exit;
                                        }
                                    }
                                    Value::boolean(false)
                                }
                            }
                        } else {
                            Value::boolean(true)
                        }
                    } else {
                        Value::boolean(true)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DefineProperty => {
                    let value = self.pop();
                    let key_val = if instr.operands[0] == usize::MAX as i64 {
                        Some(self.pop())
                    } else {
                        None
                    };
                    // B1f: may grow below (refresh on push).
                    let mut obj = self.pop();
                    let key_str = if let Some(kv) = key_val {
                        // Computed key: popped from the stack
                        Some(property_key_string(kv))
                    } else {
                        self.frames[fi]
                            .prog_str(instr.operands[0] as usize)
                            .map(|s| s.to_string())
                    };
                    if let Some(key_str) = key_str {
                        if let Some(ptr) = obj.heap_ptr() {
                            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                            if tag == TAG_OBJECT {
                                let key = PropertyKey::from_string(&key_str);
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                if let Some(slot) = shape.lookup(&key) {
                                    unsafe {
                                        JSObject::set_slot(ptr as *mut JSObject, slot, value)
                                    };
                                } else {
                                    let live =
                                        self.ensure_object_capacity(gc, ptr as *mut JSObject);
                                    unsafe {
                                        JSObject::add_property(
                                            live,
                                            key,
                                            key_str.to_string(),
                                            value,
                                        )
                                    };
                                    obj = Value::from_heap_ptr(live as *mut u8);
                                }
                            } else if tag == TAG_FUNC {
                                // For functions, delegate to do_store_property
                                let raw_key = Value::from_heap_ptr(HeapString::allocate(
                                    gc, &key_str,
                                )
                                    as *mut u8);
                                do_store_property(obj, raw_key, value, gc, self);
                            }
                        }
                    }
                    self.push(obj);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DefineAccessor => {
                    let setter = self.pop();
                    let getter = self.pop();
                    let key_val = if instr.operands[0] == usize::MAX as i64 {
                        Some(self.pop())
                    } else {
                        None
                    };
                    // B1f: may grow below (refresh on push).
                    let mut obj = self.pop();
                    let key_str = if let Some(kv) = key_val {
                        // Computed key: popped from the stack
                        Some(property_key_string(kv))
                    } else {
                        self.frames[fi]
                            .prog_str(instr.operands[0] as usize)
                            .map(|s| s.to_string())
                    };
                    if let Some(key_str) = key_str {
                        if let Some(ptr) = obj.heap_ptr() {
                            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                            if tag == TAG_OBJECT || tag == TAG_FUNC {
                                let acc_ptr = AccessorPair::allocate(gc, getter, setter);
                                let acc_val = Value::from_heap_ptr(acc_ptr);
                                let key = PropertyKey::from_string(&key_str);
                                // Re-resolve obj pointer after allocation (GC may have moved it)
                                let ptr = obj.heap_ptr().unwrap();
                                let ptr = if unsafe { (*(ptr as *const GcHeader)).is_forwarded() } {
                                    unsafe { (*(ptr as *const GcHeader)).forwarding_addr() }
                                } else {
                                    ptr
                                };
                                let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                                if tag == TAG_OBJECT {
                                    let shape =
                                        unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                    if let Some(slot) = shape.lookup(&key) {
                                        unsafe {
                                            JSObject::set_slot(ptr as *mut JSObject, slot, acc_val)
                                        };
                                    } else {
                                        let live =
                                            self.ensure_object_capacity(gc, ptr as *mut JSObject);
                                        unsafe {
                                            JSObject::add_property(
                                                live,
                                                key,
                                                key_str.to_string(),
                                                acc_val,
                                            )
                                        };
                                        obj = Value::from_heap_ptr(live as *mut u8);
                                    }
                                } else {
                                    // TAG_FUNC (after re-resolution tag may have changed)
                                    let raw_key = Value::from_heap_ptr(HeapString::allocate(
                                        gc, &key_str,
                                    )
                                        as *mut u8);
                                    do_store_property(obj, raw_key, acc_val, gc, self);
                                }
                            }
                        }
                    }
                    self.push(obj);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::SpreadIntoObject => {
                    let source = self.pop();
                    // B1f: may grow below (refresh on push).
                    let mut tgt = self.pop();
                    // §13.2.6.5 step 4: null/undefined → no-op
                    if !source.is_null() && !source.is_undefined() {
                        if let (Some(src_ptr), Some(tgt_ptr0)) = (source.heap_ptr(), tgt.heap_ptr())
                        {
                            // B1f: the target may grow below — refresh both
                            // handles together after every add.
                            let mut tgt_ptr = tgt_ptr0;
                            let tag = unsafe { (*(src_ptr as *const GcHeader)).tag() };
                            if tag == TAG_OBJECT {
                                let src_shape =
                                    unsafe { JSObject::shape_ptr(src_ptr as *mut JSObject) };
                                let count = src_shape.entries.len();
                                for i in 0..count {
                                    let key = src_shape.entries[i].0;
                                    let key_name = src_shape.key_names[i].clone();
                                    let val =
                                        unsafe { JSObject::get_slot(src_ptr as *mut JSObject, i) };
                                    let tgt_shape =
                                        unsafe { JSObject::shape_ptr(tgt_ptr as *mut JSObject) };
                                    if let Some(slot) = tgt_shape.lookup(&key) {
                                        unsafe {
                                            JSObject::set_slot(tgt_ptr as *mut JSObject, slot, val)
                                        };
                                    } else {
                                        let live = self
                                            .ensure_object_capacity(gc, tgt_ptr as *mut JSObject);
                                        unsafe { JSObject::add_property(live, key, key_name, val) };
                                        tgt = Value::from_heap_ptr(live as *mut u8);
                                        tgt_ptr = live as *mut u8;
                                    }
                                }
                            } else if tag == TAG_ARRAY {
                                let src_len =
                                    unsafe { RuneArray::length(src_ptr as *mut RuneArray) };
                                for i in 0..src_len as usize {
                                    let elem = unsafe {
                                        RuneArray::get_element(src_ptr as *mut RuneArray, i)
                                    };
                                    let key_str = i.to_string();
                                    let key = PropertyKey::from_string(&key_str);
                                    let tgt_shape =
                                        unsafe { JSObject::shape_ptr(tgt_ptr as *mut JSObject) };
                                    if let Some(slot) = tgt_shape.lookup(&key) {
                                        unsafe {
                                            JSObject::set_slot(tgt_ptr as *mut JSObject, slot, elem)
                                        };
                                    } else {
                                        let live = self
                                            .ensure_object_capacity(gc, tgt_ptr as *mut JSObject);
                                        unsafe { JSObject::add_property(live, key, key_str, elem) };
                                        tgt = Value::from_heap_ptr(live as *mut u8);
                                        tgt_ptr = live as *mut u8;
                                    }
                                }
                                let len_str = "length".to_string();
                                let len_key = PropertyKey::from_string(&len_str);
                                let tgt_shape =
                                    unsafe { JSObject::shape_ptr(tgt_ptr as *mut JSObject) };
                                if let Some(slot) = tgt_shape.lookup(&len_key) {
                                    unsafe {
                                        JSObject::set_slot(
                                            tgt_ptr as *mut JSObject,
                                            slot,
                                            Value::smi(src_len as i32),
                                        )
                                    };
                                } else {
                                    let live =
                                        self.ensure_object_capacity(gc, tgt_ptr as *mut JSObject);
                                    unsafe {
                                        JSObject::add_property(
                                            live,
                                            len_key,
                                            len_str,
                                            Value::smi(src_len as i32),
                                        )
                                    };
                                    tgt = Value::from_heap_ptr(live as *mut u8);
                                }
                            }
                        }
                    }
                    self.push(tgt);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        let val = if let Some(mi) = self.globals_override {
                            let env_v = self.module_records[mi].env.get(&name).copied();
                            if let Some(v) = env_v {
                                if v == Value::empty_sentinel() {
                                    let exc = self.tdz_error(gc, &name);
                                    if let Some(exit) = self.handle_throw(gc, exc) {
                                        return exit;
                                    }
                                    self.push(Value::undefined());
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                                v
                            } else {
                                self.globals
                                    .get(&name)
                                    .copied()
                                    .or_else(|| self.builtin_wrappers.get(&name).copied())
                                    .or_else(|| self.get_builtin(&name))
                                    .unwrap_or(Value::undefined())
                            }
                        } else {
                            self.load_global_from_module_frame(gc, fi, &name)
                                .or_else(|| self.globals.get(&name).copied())
                                .or_else(|| self.builtin_wrappers.get(&name).copied())
                                .or_else(|| self.get_builtin(&name))
                                .unwrap_or(Value::undefined())
                        };
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            self.push(Value::undefined());
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                        self.push(val);
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    let value = self.pop();
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        if let Some(mi) = self.globals_override {
                            self.module_records[mi].env.insert(name, value);
                        } else if let Some(mi) = self.module_mi_of_frame(fi) {
                            let info =
                                unsafe { (*self.module_records[mi].program).module.as_ref() };
                            let imported = info
                                .map(|i| {
                                    i.imports
                                        .iter()
                                        .any(|imp| imp.imported != "*ns*" && imp.local == name)
                                })
                                .unwrap_or(false);
                            if imported {
                                // Assigning to an imported binding is a TypeError.
                                let exc = Value::from_heap_ptr(heap_string(
                                    gc,
                                    "Assignment to constant variable.",
                                ));
                                if let Some(exit) = self.handle_throw(gc, exc) {
                                    return exit;
                                }
                                self.push(value);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                            self.module_records[mi].env.insert(name, value);
                        } else {
                            self.globals.insert(name, value);
                        }
                    }
                    self.push(value);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ImportModule => {
                    let idx = instr.operands[0] as usize;
                    let (specifier, imported, local) = if let Some(mi) = self.module_stack.last() {
                        let program = self.module_records[*mi].program;
                        if let Some(info) = unsafe { (*program).module.as_ref() } {
                            if let Some(imp) = info.imports.get(idx) {
                                (
                                    imp.specifier.clone(),
                                    imp.imported.clone(),
                                    imp.local.clone(),
                                )
                            } else {
                                (String::new(), String::new(), String::new())
                            }
                        } else {
                            (String::new(), String::new(), String::new())
                        }
                    } else {
                        (String::new(), String::new(), String::new())
                    };
                    let dep_idx = match self.modules.get(&specifier) {
                        Some(&d) => d,
                        None => {
                            // Dependency not pre-loaded (loader gap) — treat as
                            // empty module so imports resolve to undefined.
                            self.push(Value::undefined());
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    };
                    if let Err(v) = self.eval_module_rec(gc, dep_idx) {
                        return Exit::Throw(v);
                    }
                    if imported == "*ns*" {
                        let ns = self.make_module_namespace(gc, dep_idx);
                        if let Some(mi) = self.module_stack.last() {
                            let program = self.module_records[*mi].program;
                            if let Some(slot) =
                                unsafe { (*program).local_names.iter().position(|n| *n == local) }
                            {
                                if let Some(frame) = self.frames.last_mut() {
                                    if slot < frame.locals.len() {
                                        frame.locals[slot] = ns;
                                    }
                                }
                            }
                        }
                        self.push(ns);
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadModuleImport => {
                    let idx = instr.operands[0] as usize;
                    if let Some(mi) = self.current_module_mi(fi) {
                        let program = self.module_records[mi].program;
                        if let Some(info) = unsafe { (*program).module.as_ref() } {
                            if let Some(imp) = info.imports.get(idx) {
                                let dep_idx = self.modules.get(&imp.specifier).copied();
                                let v = match dep_idx {
                                    Some(d) => self
                                        .resolve_export_value(gc, d, &imp.imported, &mut Vec::new())
                                        .unwrap_or(Value::undefined()),
                                    None => Value::undefined(),
                                };
                                if v == Value::empty_sentinel() {
                                    let exc = self.tdz_error(gc, &imp.imported);
                                    if let Some(exit) = self.handle_throw(gc, exc) {
                                        return exit;
                                    }
                                    self.push(Value::undefined());
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                                self.push(v);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        }
                    }
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreModuleImport => {
                    // §9.2.2.3 SetMutableBinding on an imported binding: TypeError.
                    let value = self.pop();
                    self.push(value);
                    self.frames[fi].pc = pc + 1;
                    if let Some(exit) =
                        self.throw_type_error(gc, "Assignment to constant variable.")
                    {
                        return exit;
                    }
                    continue;
                }
                Opcode::ExportSync => {
                    let name_idx = instr.operands[0] as usize;
                    let value = self.pop();
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        if let Some(mi) = self.current_module_mi(fi) {
                            self.module_records[mi].env.insert(name, value);
                        }
                    }
                    self.push(value);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ModuleTdz => {
                    // Mark a module binding as uninitialized (§9.2.2.2).
                    let name_idx = instr.operands[0] as usize;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        if let Some(mi) = self.current_module_mi(fi) {
                            self.module_records[mi]
                                .env
                                .insert(name, Value::empty_sentinel());
                        }
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::IncLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let old_val = if idx < self.frames[fi].locals.len() {
                        self.frames[fi].locals[idx]
                    } else {
                        Value::undefined()
                    };
                    if old_val.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let n = to_number(old_val) + 1.0;
                    let new_val = number_result(gc, n);
                    if idx >= self.frames[fi].locals.len() {
                        self.frames[fi].locals.resize(idx + 1, Value::undefined());
                    }
                    self.frames[fi].locals[idx] = new_val;
                    self.push(if is_prefix { new_val } else { old_val });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DecLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let old_val = if idx < self.frames[fi].locals.len() {
                        self.frames[fi].locals[idx]
                    } else {
                        Value::undefined()
                    };
                    if old_val.is_symbol() {
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot convert a Symbol value to a number",
                        );
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let n = to_number(old_val) - 1.0;
                    let new_val = number_result(gc, n);
                    if idx >= self.frames[fi].locals.len() {
                        self.frames[fi].locals.resize(idx + 1, Value::undefined());
                    }
                    self.frames[fi].locals[idx] = new_val;
                    self.push(if is_prefix { new_val } else { old_val });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::IncGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        let old_val = self
                            .globals
                            .get(&name)
                            .copied()
                            .or_else(|| self.builtin_wrappers.get(&name).copied())
                            .or_else(|| self.get_builtin(&name))
                            .unwrap_or(Value::undefined());
                        if old_val.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let n = to_number(old_val) + 1.0;
                        let new_val = number_result(gc, n);
                        self.globals.insert(name, new_val);
                        self.push(if is_prefix { new_val } else { old_val });
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DecGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        let old_val = self
                            .globals
                            .get(&name)
                            .copied()
                            .or_else(|| self.builtin_wrappers.get(&name).copied())
                            .or_else(|| self.get_builtin(&name))
                            .unwrap_or(Value::undefined());
                        if old_val.is_symbol() {
                            let err = crate::errors::error_object(
                                gc,
                                &self.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "Cannot convert a Symbol value to a number",
                            );
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        let n = to_number(old_val) - 1.0;
                        let new_val = number_result(gc, n);
                        self.globals.insert(name, new_val);
                        self.push(if is_prefix { new_val } else { old_val });
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Lexical scoping (let/const/TDZ) ----
                Opcode::BlockEnter => {
                    let count = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let f = &mut self.frames[fi];
                    f.scope_boundaries.push(f.lexical_slots.len());
                    f.lexical_slots
                        .extend(std::iter::repeat_n(Value::undefined(), count));
                    f.lexical_tdz.extend(std::iter::repeat_n(true, count));
                    f.lexical_const.extend(std::iter::repeat_n(false, count));
                    f.pc = pc + 1;
                }
                Opcode::BlockLeave => {
                    let fi = self.frames.len() - 1;
                    let f = &mut self.frames[fi];
                    if let Some(boundary) = f.scope_boundaries.pop() {
                        f.lexical_slots.truncate(boundary);
                        f.lexical_tdz.truncate(boundary);
                        f.lexical_const.truncate(boundary);
                    }
                    f.pc = pc + 1;
                }
                Opcode::DeclareLet => {
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let val = self.pop();
                    let f = &mut self.frames[fi];
                    if slot < f.lexical_slots.len() {
                        f.lexical_slots[slot] = val;
                        f.lexical_tdz[slot] = false;
                    }
                    f.pc = pc + 1;
                }
                Opcode::DeclareConst => {
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let val = self.pop();
                    let f = &mut self.frames[fi];
                    if slot < f.lexical_slots.len() {
                        f.lexical_slots[slot] = val;
                        f.lexical_tdz[slot] = false;
                        f.lexical_const[slot] = true;
                    }
                    f.pc = pc + 1;
                }
                Opcode::LoadLexical => {
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let f = &self.frames[fi];
                    if slot < f.lexical_slots.len() {
                        if f.lexical_tdz[slot] {
                            if let Some(exit) = self.throw_reference_error(
                                gc,
                                &format!("Cannot access '{}' before initialization", slot),
                            ) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(f.lexical_slots[slot]);
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreLexical => {
                    if std::env::var_os("RMD").is_some() {
                        eprintln!(
                            "[sl] op={:?} pc={} frames={} top={:?}",
                            instr.opcode,
                            pc,
                            self.frames.len(),
                            self.stack.last()
                        );
                    }
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let val = self.pop();
                    // Check TDZ before store (per spec §8.1.1.4.4, SetMutableBinding
                    // throws ReferenceError if binding is uninitialized)
                    if slot < self.frames[fi].lexical_slots.len() {
                        if self.frames[fi].lexical_tdz[slot] {
                            if let Some(exit) = self.throw_reference_error(
                                gc,
                                &format!("Cannot access '{}' before initialization", slot),
                            ) {
                                return exit;
                            }
                            continue;
                        }
                        if self.frames[fi].lexical_const[slot] {
                            if let Some(exit) =
                                self.throw_type_error(gc, "Assignment to constant variable")
                            {
                                return exit;
                            }
                            continue;
                        }
                        self.frames[fi].lexical_slots[slot] = val;
                    }
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Unary ----
                Opcode::TypeOf => {
                    let val = self.pop();
                    let s = if val.is_undefined() {
                        "undefined"
                    } else if val.is_null() {
                        "object"
                    } else if val.is_boolean() {
                        "boolean"
                    } else if val.is_symbol() {
                        "symbol"
                    } else if val.is_smi() {
                        // Negative Smis are builtin handles — they are callable.
                        if val.as_smi().unwrap() < 0 {
                            "function"
                        } else {
                            "number"
                        }
                    } else if let Some(ptr) = val.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        match tag {
                            TAG_STRING => "string",
                            TAG_FUNC => "function",
                            TAG_FLOAT64 => "number",
                            TAG_STRING_OBJ => "object",
                            TAG_OBJECT => {
                                if self
                                    .callable_wrappers
                                    .iter()
                                    .any(|w| w.heap_ptr() == Some(ptr))
                                {
                                    "function"
                                } else {
                                    "object"
                                }
                            }
                            _ => "object",
                        }
                    } else {
                        // float64 or unknown sentinel
                        "number"
                    };
                    let str = HeapString::allocate(gc, s);
                    self.push(Value::from_heap_ptr(str as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Control flow ----
                Opcode::Jump => {
                    let target = instr.operands[0] as usize;
                    if target < pc {
                        // Back-edge: loop iteration
                        let key: TraceKey = (prog_ptr as usize, target);
                        // Module programs are not traced: their LoadGlobal/
                        // StoreGlobal target the module env (globals_override),
                        // not the shared globals map the JIT code reads.
                        let module_prog = unsafe { (*prog_ptr).is_module };
                        if module_prog {
                            self.frames[fi].pc = target;
                            continue;
                        }
                        let entry = self.loop_counts.entry(key).or_insert(0);
                        *entry += 1;
                        // Start recording a trace at threshold, or when a
                        // previous recording was discarded (pending_rerecord)
                        // Conservative trace-eligibility gate: loops whose bodies
                        // contain IC-bearing property instructions must stay on the
                        // interpreter. Recording such a loop corrupts execution
                        // (pre-existing recorder bug — receivers/pc flow degrade
                        // right after the recorded pass closes), so we never even
                        // start recording for them. Numeric-only loops still trace.
                        let mut trace_eligible = true;
                        if *entry == 50 {
                            unsafe {
                                let instrs = (*prog_ptr).instructions.as_ptr();
                                for i in target..=pc {
                                    let op = (*instrs.add(i)).opcode;
                                    match op {
                                        // Recorder-corruption gates: IC-bearing
                                        // property loads (silent NaNs) and calls
                                        // (bailout depth-model mismatch when a
                                        // callee tiers up mid-record) stay on the
                                        // interpreter — "bail-to-interpreter stays
                                        // correct". Numeric-only loops still trace.
                                        Opcode::LoadProperty
                                        | Opcode::StoreProperty
                                        | Opcode::LoadPropertyIC
                                        | Opcode::StorePropertyIC
                                            if (*instrs.add(i)).ic_index >= 0 =>
                                        {
                                            trace_eligible = false;
                                            break;
                                        }
                                        Opcode::Call | Opcode::CallFromArray | Opcode::New => {
                                            trace_eligible = false;
                                            break;
                                        }
                                        // Per-iteration env churn allocates a
                                        // fresh EnvObject every iteration; the
                                        // lexical helper's MakeEnv can trigger
                                        // GC mid-native while the recorded
                                        // trace holds raw pointers. Same
                                        // bail-to-interpreter policy.
                                        Opcode::MakeEnv | Opcode::RestoreEnv => {
                                            trace_eligible = false;
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        if *entry == 50 && trace_eligible || self.pending_rerecord.remove(&key) {
                            self.recording_trace = Some(key);
                            self.loop_traces.insert(
                                key,
                                LoopTrace {
                                    target_pc: target,
                                    ops: Vec::new(),
                                    total_iterations: *entry,
                                    shape_ids: Vec::new(),
                                    compiled_entry: std::ptr::null(),
                                    exit_pc: 0,
                                    compiled_prog: std::ptr::null_mut(),
                                    trace_to_original_pc: Vec::new(),
                                    #[cfg(feature = "jit")]
                                    bailout_table: None,
                                    miss_count: 0,
                                    #[cfg(feature = "jit")]
                                    inline_profiles: Vec::new(),
                                },
                            );
                        }
                        // After trace recorded (monomorphic), patch loop body
                        if *entry > 60
                            && self
                                .loop_traces
                                .get(&key)
                                .is_some_and(|t| t.is_monomorphic())
                        {
                            unsafe {
                                self.patch_loop_body(prog_ptr, target, pc);
                            }
                        }
                        // Execute compiled trace natively, bypassing interpreter
                        #[allow(unused_variables)]
                        let compiled = self
                            .loop_traces
                            .get(&key)
                            .map(|t| t.compiled_entry)
                            .unwrap_or(std::ptr::null());
                        #[cfg(feature = "jit")]
                        if !compiled.is_null() {
                            // Execute compiled trace natively.  The trace runs the
                            // entire loop body (condition + body + branch); when the
                            // condition becomes false it exits.  Works for all Smi
                            // values; results above i31 range display as wrapped i32
                            // due to as_smi() truncation, but the underlying u64 is
                            // correct.
                            unsafe {
                                let gc_ptr = gc as *mut SemiSpace as *mut u8;
                                let _ = self.execute_trace(fi, compiled, gc_ptr);
                            }
                            if self.jit_bailout.pending {
                                // Trace bailed mid-loop (e.g. overflow guard).
                                // The bailout PC from the compiled code is a trace
                                // instruction index — translate to the original
                                // program PC so the interpreter resumes correctly.
                                let trace_idx = self.jit_bailout.bc_pc;
                                let original_pc = self
                                    .loop_traces
                                    .get(&key)
                                    .and_then(|t| t.trace_to_original_pc.get(trace_idx).copied())
                                    .unwrap_or(trace_idx);
                                self.jit_bailout.pending = false;
                                self.jit_bailout.bc_pc = 0;
                                // Re-record: if bailout was ShapeMiss, increment
                                // per-trace miss counter and re-record at threshold.
                                let rerecord_needed = {
                                    let trace = self.loop_traces.get_mut(&key);
                                    match trace {
                                        Some(t) => {
                                            let is_shape_miss = t
                                                .bailout_table
                                                .as_ref()
                                                .and_then(|bt| {
                                                    bt.points.iter().find(|bp| bp.bc_pc == trace_idx)
                                                })
                                                .is_some_and(|b| {
                                                    b.reason
                                                        == rune_jit_baseline::BailoutReason::ShapeMiss
                                                });
                                            if is_shape_miss {
                                                t.miss_count += 1;
                                                if t.miss_count >= 100 {
                                                    t.miss_count = 0;
                                                    true
                                                } else {
                                                    false
                                                }
                                            } else {
                                                false
                                            }
                                        }
                                        None => false,
                                    }
                                };
                                if rerecord_needed {
                                    #[cfg(target_arch = "aarch64")]
                                    self.compile_trace_native(prog_ptr, target);
                                }
                                let snapshot = std::mem::take(&mut self.jit_bailout.stack_snapshot);
                                validate_bailout_snapshot(
                                    self.loop_traces
                                        .get(&key)
                                        .and_then(|t| t.bailout_table.as_deref()),
                                    trace_idx,
                                    snapshot.len(),
                                    "trace",
                                );
                                self.frames[fi].pc = original_pc;
                                self.stack.truncate(self.frames[fi].stack_base);
                                for val in snapshot {
                                    self.push(Value::from_raw(val));
                                }
                            } else {
                                // Trace completed normally (loop condition false).
                                self.frames[fi].pc = self
                                    .loop_traces
                                    .get(&key)
                                    .map(|t| t.exit_pc)
                                    .unwrap_or(pc + 1);
                            }
                            continue;
                        }
                    }
                    self.frames[fi].pc = target;
                }
                Opcode::JumpIfTrue => {
                    let val = self.pop();
                    let target = instr.operands[0] as usize;
                    if val.to_bool() {
                        self.frames[fi].pc = target
                    } else {
                        self.frames[fi].pc = pc + 1
                    }
                }
                Opcode::JumpIfFalse => {
                    let val = self.pop();
                    let target = instr.operands[0] as usize;
                    if !val.to_bool() {
                        self.frames[fi].pc = target
                    } else {
                        self.frames[fi].pc = pc + 1
                    }
                }
                Opcode::JumpIfNullOrUndefined => {
                    let val = self.pop();
                    let target = instr.operands[0] as usize;
                    if val.is_null() || val.is_undefined() {
                        self.frames[fi].pc = target
                    } else {
                        self.frames[fi].pc = pc + 1
                    }
                }
                Opcode::Throw => {
                    let val = self.pop();
                    if let Some(exit) = self.handle_throw(gc, val) {
                        return exit;
                    }
                    continue;
                }
                Opcode::ThrowIfNullish => {
                    let val = self.peek();
                    if val.is_null() || val.is_undefined() {
                        self.pop();
                        self.register_roots(gc);
                        let exc = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot destructure null or undefined",
                        );
                        // Now behave like Opcode::Throw
                        let handler_idx = self
                            .try_stack
                            .iter()
                            .rposition(|tf| tf.frame_depth == self.frames.len());
                        if let Some(idx) = handler_idx {
                            let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                let tf = &self.try_stack[idx];
                                (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                            };
                            if in_catch && finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[fi].pc = finally_pc;
                                continue;
                            }
                            if catch_pc != 0 && !in_catch {
                                if finally_pc != 0 {
                                    self.try_stack[idx].in_catch = true;
                                } else {
                                    self.try_stack.remove(idx);
                                }
                                self.stack.truncate(stack_depth);
                                self.push(exc);
                                self.frames[fi].pc = catch_pc;
                                continue;
                            }
                            if finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[fi].pc = finally_pc;
                                continue;
                            }
                        }
                        // No handler — pop frame and check caller
                        let callee_base = self.frames.last().unwrap().stack_base;
                        let popped_frame = self.frames.len() - 1;
                        self.last_locals = self.frames[popped_frame].locals.clone();
                        self.frames.pop();
                        self.try_stack
                            .retain(|tf| tf.frame_depth != popped_frame + 1);
                        if self.frames.is_empty() {
                            self.stack.clear();
                            return Exit::Throw(exc);
                        }
                        let new_fi = self.frames.len() - 1;
                        let caller_idx = self
                            .try_stack
                            .iter()
                            .rposition(|tf| tf.frame_depth == self.frames.len());
                        if let Some(idx) = caller_idx {
                            let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                let tf = &self.try_stack[idx];
                                (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                            };
                            if in_catch && finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[new_fi].pc = finally_pc;
                                continue;
                            }
                            if catch_pc != 0 && !in_catch {
                                if finally_pc != 0 {
                                    self.try_stack[idx].in_catch = true;
                                } else {
                                    self.try_stack.remove(idx);
                                }
                                self.stack.truncate(stack_depth);
                                self.push(exc);
                                self.frames[new_fi].pc = catch_pc;
                                continue;
                            }
                            if finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[new_fi].pc = finally_pc;
                                continue;
                            }
                        }
                        self.stack.truncate(callee_base);
                        self.push(exc);
                        self.frames[new_fi].pc += 1;
                        return Exit::Throw(exc);
                    }
                    self.frames[fi].pc += 1;
                }
                Opcode::TryBegin => {
                    let catch_pc = instr.operands[0] as usize;
                    let finally_pc = instr.operands[1] as usize;
                    self.try_stack.push(TryFrame {
                        catch_pc,
                        finally_pc,
                        stack_depth: self.stack.len(),
                        frame_depth: self.frames.len(),
                        saved_exception: None,
                        in_catch: false,
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::TryEnd => {
                    self.try_stack.pop();
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::FinallyDone => {
                    let rethrow_pc = instr.operands[0] as usize;
                    let tf = self.try_stack.pop().expect("FinallyDone without TryFrame");
                    if let Some(ex) = tf.saved_exception {
                        self.push(ex);
                        self.frames[fi].pc = rethrow_pc;
                    } else {
                        self.frames[fi].pc = pc + 1;
                    }
                }

                // ---- Functions ----
                Opcode::MakeRestArray => {
                    let regular_count = instr.operands[0] as usize;
                    let named_offset = if unsafe { (*self.frames[fi].prog).named_function } {
                        1
                    } else {
                        0
                    };
                    let rest_start = named_offset + regular_count;
                    let rest_end = self.frames[fi].locals.len();
                    let mut elems: Vec<Value> = Vec::new();
                    for i in rest_start..rest_end {
                        elems.push(self.frames[fi].locals[i]);
                    }
                    let arr = RuneArray::allocate(gc, &elems);
                    unsafe {
                        let ptr = arr as *mut u8;
                        let shape_ptr = ptr.add(8) as *mut *const Shape;
                        *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                        let proto_ptr = ptr.add(24) as *mut *mut u8;
                        if self.array_prototype.is_heap_object() {
                            if let Some(proto) = self.array_prototype.heap_ptr() {
                                *proto_ptr = proto;
                            }
                        }
                    }
                    self.push(Value::from_heap_ptr(arr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::MakeArgumentsArray => {
                    let named_offset = if unsafe { (*self.frames[fi].prog).named_function } {
                        1
                    } else {
                        0
                    };
                    let argc = self.frames[fi].passed_argc;
                    let mut elems: Vec<Value> = Vec::with_capacity(argc);
                    for i in 0..argc {
                        elems.push(self.frames[fi].locals[named_offset + i]);
                    }
                    let arr = RuneArray::allocate(gc, &elems);
                    unsafe {
                        let ptr = arr as *mut u8;
                        let shape_ptr = ptr.add(8) as *mut *const Shape;
                        *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                        let proto_ptr = ptr.add(24) as *mut *mut u8;
                        if self.array_prototype.is_heap_object() {
                            if let Some(proto) = self.array_prototype.heap_ptr() {
                                *proto_ptr = proto;
                            }
                        }
                    }
                    self.push(Value::from_heap_ptr(arr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::CopyLexical => {
                    let src_slot = instr.operands[0] as usize;
                    let dst_slot = instr.operands[1] as usize;
                    if std::env::var_os("RMD").is_some() && self.frames.len() <= 1 {
                        let f0 = &self.frames[fi];
                        let sv = f0
                            .lexical_slots
                            .get(src_slot)
                            .copied()
                            .unwrap_or(Value::undefined());
                        eprintln!("[cx] {}->{} pc={} sv={:?}", src_slot, dst_slot, pc, sv);
                    }
                    let f = &self.frames[fi];
                    let val = if src_slot < f.lexical_slots.len() {
                        f.lexical_slots[src_slot]
                    } else {
                        Value::undefined()
                    };
                    let f = &mut self.frames[fi];
                    if dst_slot >= f.lexical_slots.len() {
                        f.lexical_slots.resize(dst_slot + 1, Value::undefined());
                        f.lexical_tdz.resize(dst_slot + 1, false);
                        f.lexical_const.resize(dst_slot + 1, false);
                    }
                    f.lexical_slots[dst_slot] = val;
                    f.lexical_tdz[dst_slot] = false;
                    f.pc = pc + 1;
                }
                Opcode::MakeFunction => {
                    let func_idx = instr.operands[0] as u64;
                    let is_arrow = instr.operands.get(1).copied().unwrap_or(0) != 0;
                    let prog_ptr = prog as *const BytecodeProgram as *const u8;
                    // Allocate the default `.prototype` FIRST so that if GC triggers
                    // during Func::allocate, we can resolve the forwarding address.
                    let default_proto = if !is_arrow {
                        JSObject::allocate(gc, Shape::empty(), &[])
                    } else {
                        std::ptr::null_mut()
                    };
                    let ptr = Func::allocate(gc, func_idx, prog_ptr, is_arrow, self.frames[fi].env);
                    // Both default_proto and ptr may be stale after GC-triggered
                    // collection during either allocate. Resolve via forwarding.
                    unsafe {
                        let resolved_ptr = if (*(ptr as *const GcHeader)).is_forwarded() {
                            (*(ptr as *const GcHeader)).forwarding_addr() as *mut Func
                        } else {
                            ptr
                        };
                        // AFPC: install a cached native entry point if one exists.
                        if let Some(&entry) = self.cached_jit_entries.get(&(func_idx as usize)) {
                            Func::set_jit_entry(resolved_ptr, entry);
                        }
                        Func::set_env_ptr(resolved_ptr, self.frames[fi].env);
                        if !is_arrow {
                            let resolved_proto = if !default_proto.is_null()
                                && (*(default_proto as *const GcHeader)).is_forwarded()
                            {
                                (*(default_proto as *const GcHeader)).forwarding_addr()
                            } else {
                                default_proto as *mut u8
                            };
                            Func::set_prototype(resolved_ptr, resolved_proto);
                        }
                        // MakeConstructor back-ref (§10.2.4 step 9.b):
                        // prototype.constructor = F (writable + configurable,
                        // NON-enumerable). Post-block so the binding below
                        // feeds every later use (ids/flags/push). Inner
                        // unsafe blocks omitted: already inside one.
                        let mut resolved_ptr =
                            if (*(resolved_ptr as *const GcHeader)).is_forwarded() {
                                (*(resolved_ptr as *const GcHeader)).forwarding_addr() as *mut Func
                            } else {
                                resolved_ptr
                            };
                        if !is_arrow {
                            // Proto is authoritative post-set (avoids threading
                            // the inner binding out of its scope).
                            let proto_now = Func::prototype(resolved_ptr);
                            let ckey_str = HeapString::allocate(gc, "constructor");
                            // Key alloc may have GC-moved either object.
                            resolved_ptr = if (*(resolved_ptr as *const GcHeader)).is_forwarded() {
                                (*(resolved_ptr as *const GcHeader)).forwarding_addr() as *mut Func
                            } else {
                                resolved_ptr
                            };
                            let mut proto_now = if !proto_now.is_null()
                                && (*(proto_now as *const GcHeader)).is_forwarded()
                            {
                                (*(proto_now as *const GcHeader)).forwarding_addr()
                            } else {
                                proto_now
                            };
                            proto_now = {
                                let roomy =
                                    self.ensure_object_capacity(gc, proto_now as *mut JSObject);
                                if (*(roomy as *const GcHeader)).is_forwarded() {
                                    (*(roomy as *const GcHeader)).forwarding_addr()
                                } else {
                                    roomy as *mut u8
                                }
                            };
                            let func_val = Value::from_heap_ptr(resolved_ptr as *mut u8);
                            JSObject::add_property_with_attrs(
                                proto_now as *mut JSObject,
                                PropertyKey::from_string(&HeapString::to_string(ckey_str)),
                                "constructor".to_string(),
                                func_val,
                                rune_core::shape::ATTR_WRITABLE
                                    | rune_core::shape::ATTR_CONFIGURABLE,
                            );
                            resolved_ptr = if (*(resolved_ptr as *const GcHeader)).is_forwarded() {
                                (*(resolved_ptr as *const GcHeader)).forwarding_addr() as *mut Func
                            } else {
                                resolved_ptr
                            };
                        }
                        // Propagate private name IDs from class evaluation frame to Func.
                        // Inside a constructor (which itself was created during class
                        // evaluation), the frame has no IDs but the executing Func does —
                        // fall back to it so private methods/accessors defined in the
                        // ctor keep access to the class's private names.
                        let mut ids = self.frames[fi].private_name_ids;
                        if ids.is_null() {
                            let cur_func = self.frames[fi].func_ptr;
                            if !cur_func.is_null() {
                                ids = Func::private_name_ids(cur_func as *mut Func);
                            }
                        }
                        if !ids.is_null() {
                            Func::set_private_name_ids(resolved_ptr, ids);
                        }
                        // F1: mirror the compiled-program kind bits into the Func
                        // flags word (generator/async/class-constructor). The
                        // arrow bit already arrived via the instruction
                        // operand + allocate; record lookup is fallible so
                        // malformed indices degrade to no kind bits.
                        let mut kind_flags = 0u32;
                        if let Some(func_prog) = prog.functions.get(func_idx as usize) {
                            if func_prog.is_generator {
                                kind_flags |= FUNC_FLAG_GENERATOR;
                            }
                            if func_prog.is_async {
                                kind_flags |= FUNC_FLAG_ASYNC;
                            }
                            if func_prog.is_class_constructor {
                                kind_flags |= FUNC_FLAG_CLASS_CTOR;
                            }
                            if func_prog.is_strict {
                                kind_flags |= FUNC_FLAG_STRICT;
                            }
                        }
                        if kind_flags != 0 {
                            Func::add_flags(resolved_ptr, kind_flags);
                        }
                        // Record the owning module (if created during module
                        // evaluation) so LoadGlobal/StoreGlobal inside this
                        // function resolve against the module env.
                        if let Some(mi) = self.module_stack.last() {
                            Func::set_module_mi(resolved_ptr, *mi as i32);
                        }
                        self.push(Value::from_heap_ptr(resolved_ptr as *mut u8));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::SetSuperclass => {
                    let superclass = self.pop();
                    // func is below superclass on stack
                    let func_val = self.pop();
                    if let Some(ptr) = func_val.heap_ptr() {
                        let super_ptr = superclass.heap_ptr().unwrap_or(std::ptr::null_mut());
                        unsafe { Func::set_superclass(ptr as *mut Func, super_ptr) };
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadSuperclass => {
                    let func_ptr = self.frames[fi].func_ptr;
                    if func_ptr.is_null() {
                        self.push(Value::undefined());
                    } else {
                        let super_ptr = unsafe { Func::superclass(func_ptr as *mut Func) };
                        if super_ptr.is_null() {
                            self.push(Value::undefined());
                        } else {
                            self.push(Value::from_heap_ptr(super_ptr));
                        }
                    }
                    self.frames[fi].pc = pc + 1;
                }
                // ---- Private field/method ----
                Opcode::PrivateNameScope => {
                    let count = instr.operands[0] as usize;
                    // Allocate unique private name IDs and store in RuneArray
                    let mut ids = Vec::with_capacity(count);
                    for _ in 0..count {
                        let id = self.next_private_name_id;
                        self.next_private_name_id += 1;
                        ids.push(Value::smi(id as i32));
                    }
                    let array_ptr = RuneArray::allocate(gc, &ids);
                    // Store on current frame's private_name_ids field
                    self.frames[fi].private_name_ids = array_ptr as *mut u8;
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DefinePrivateField => {
                    let slot_idx = instr.operands[0] as u32;
                    let val = self.pop();
                    let obj = self.pop();
                    // Get private name ID from the current executing function
                    let priv_id = self.get_private_name_id(fi, slot_idx);
                    let priv_name_id = if let Some(id) = priv_id {
                        id
                    } else {
                        self.register_roots(gc);
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Private field access outside class body",
                        );
                        self.push(err);
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    };
                    let key_str = format!("\x00private_{}", priv_name_id);
                    let key_val =
                        Value::from_heap_ptr(HeapString::allocate(gc, &key_str) as *mut u8);
                    do_store_property(obj, key_val, val, gc, self);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::MakeAccessorPair => {
                    let setter = self.pop();
                    let getter = self.pop();
                    let acc_ptr = AccessorPair::allocate(gc, getter, setter);
                    self.push(Value::from_heap_ptr(acc_ptr));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadPrivateProperty => {
                    let slot_idx = instr.operands[0] as u32;
                    let obj = self.pop();
                    let priv_id = self.get_private_name_id(fi, slot_idx);
                    let priv_name_id = if let Some(id) = priv_id {
                        id
                    } else {
                        self.register_roots(gc);
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Private field access outside class body",
                        );
                        self.push(err);
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    };
                    let key_str = format!("\x00private_{}", priv_name_id);
                    let key_val =
                        Value::from_heap_ptr(HeapString::allocate(gc, &key_str) as *mut u8);
                    // §7.3.30 PrivateGet: missing private element → TypeError
                    if !has_property(obj, key_val, Some(self.function_prototype)) {
                        self.register_roots(gc);
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot read private member from object",
                        );
                        self.push(err);
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    let result =
                        load_property_recursive(obj, key_val, Some(self.function_prototype), gc);
                    // Private accessor getter dispatch (get #x() {} / set #x() {})
                    if let Some(ptr) = result.heap_ptr() {
                        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_ACCESSOR {
                            let (v, pending) = self.resolve_accessor_for_read(result, obj, gc);
                            if pending {
                                continue;
                            }
                            self.push(v);
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StorePrivateProperty => {
                    let slot_idx = instr.operands[0] as u32;
                    let val = self.pop();
                    let obj = self.pop();
                    let priv_id = self.get_private_name_id(fi, slot_idx);
                    let priv_name_id = if let Some(id) = priv_id {
                        id
                    } else {
                        self.register_roots(gc);
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Private field access outside class body",
                        );
                        self.push(err);
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    };
                    let key_str = format!("\x00private_{}", priv_name_id);
                    let key_val =
                        Value::from_heap_ptr(HeapString::allocate(gc, &key_str) as *mut u8);
                    // §7.3.31 PrivateSet: missing private element → TypeError
                    if !has_property(obj, key_val, Some(self.function_prototype)) {
                        self.register_roots(gc);
                        let err = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Cannot write private member to object",
                        );
                        self.push(err);
                        if let Some(exit) = self.handle_throw(gc, err) {
                            return exit;
                        }
                        continue;
                    }
                    // Private accessor setter dispatch (set #x(v) {})
                    let current =
                        load_property_recursive(obj, key_val, Some(self.function_prototype), gc);
                    if let Some(vptr) = current.heap_ptr() {
                        if unsafe { (*(vptr as *const GcHeader)).tag() } == TAG_ACCESSOR {
                            let setter = unsafe { AccessorPair::setter(vptr) };
                            if !setter.is_undefined() {
                                if let Some(sptr) = setter.heap_ptr() {
                                    if unsafe { (*(sptr as *const GcHeader)).tag() } == TAG_FUNC {
                                        self.pending_accessor_call = Some(PendingAccessorCall {
                                            source_frame_depth: self.frames.len(),
                                            is_getter: false,
                                        });
                                        let func_ptr = sptr;
                                        let func_idx =
                                            unsafe { Func::func_index(func_ptr as *mut Func) }
                                                as usize;
                                        let creator_prog = unsafe {
                                            &*(Func::prog_ptr(func_ptr as *mut Func)
                                                as *const BytecodeProgram)
                                        };
                                        if func_idx < creator_prog.functions.len() {
                                            let func_prog = &creator_prog.functions[func_idx];
                                            let func_env =
                                                unsafe { Func::env_ptr(func_ptr as *mut Func) };
                                            let mut locals = if func_prog.named_function {
                                                vec![setter]
                                            } else {
                                                vec![]
                                            };
                                            locals.push(val);
                                            self.frames.push(Frame {
                                                locals,
                                                lexical_slots: Vec::new(),
                                                lexical_tdz: Vec::new(),
                                                lexical_const: Vec::new(),
                                                scope_boundaries: Vec::new(),
                                                passed_argc: 1,
                                                pc: 0,
                                                stack_base: self.stack.len(),
                                                prog: func_prog as *const BytecodeProgram,
                                                generator_id: None,
                                                this: obj,
                                                is_constructor_call: false,
                                                constructed_object: Value::undefined(),
                                                env: func_env,
                                                func_ptr,
                                                private_name_ids: std::ptr::null_mut(),
                                            });
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    do_store_property(obj, key_val, val, gc, self);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::MakeEnv => {
                    let count = instr.operands[0] as usize;
                    let new_env =
                        EnvObject::allocate(gc, count, self.frames[fi].env as *mut EnvObject);
                    // new_env and parent may be stale after GC-triggered collection;
                    // resolve forwarding and re-read from the (updated) root.
                    unsafe {
                        let resolved = if (*(new_env as *const GcHeader)).is_forwarded() {
                            (*(new_env as *const GcHeader)).forwarding_addr() as *mut EnvObject
                        } else {
                            new_env
                        };
                        EnvObject::set_parent(resolved, self.frames[fi].env as *mut EnvObject);
                        self.frames[fi].env = resolved as *mut u8;
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::RestoreEnv => {
                    let env = self.frames[fi].env as *mut EnvObject;
                    if !env.is_null() {
                        let parent = unsafe { EnvObject::parent(env) };
                        self.frames[fi].env = parent as *mut u8;
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadCaptured => {
                    let depth = instr.operands[0] as usize;
                    let slot = instr.operands[1] as usize;
                    if std::env::var_os("RMD").is_some() {
                        let env = self.frames[fi].env as *mut EnvObject;
                        let v = if env.is_null() {
                            Value::undefined()
                        } else {
                            unsafe { EnvObject::get_slot(env, slot) }
                        };
                        eprintln!(
                            "[lc] d{}s{} pc={} v={:?} env={:?}",
                            depth,
                            slot,
                            pc,
                            v,
                            !env.is_null()
                        );
                    }
                    let env = self.frames[fi].env as *mut EnvObject;
                    let target = unsafe { EnvObject::ancestor(env, depth) };
                    let val = unsafe { EnvObject::get_slot(target, slot) };
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreCaptured => {
                    let depth = instr.operands[0] as usize;
                    let slot = instr.operands[1] as usize;
                    let val = self.pop();
                    if std::env::var_os("RMD").is_some() {
                        eprintln!("[sc] d{}s{} pc={} v={:?}", depth, slot, pc, val);
                    }
                    let env = self.frames[fi].env as *mut EnvObject;
                    let target = unsafe { EnvObject::ancestor(env, depth) };
                    unsafe { EnvObject::set_slot(target, slot, val) };
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::New => {
                    let argc = instr.operands[0] as usize;
                    let mut args: Vec<Value> = (0..argc).map(|_| self.pop()).collect();
                    args.reverse();
                    let constructor = self.pop();
                    // String constructor [[Construct]]: create String wrapper
                    if constructor == self.string_constructor {
                        let arg = args.first().copied().unwrap_or(Value::undefined());
                        let s = arg_to_js_string_for_ctor(arg, gc, self);
                        let str_ptr = HeapString::allocate(gc, &s);
                        let str_obj = if self.string_prototype.is_heap_object() {
                            let proto_ptr = self.string_prototype.heap_ptr();
                            if let Some(ptr) = proto_ptr {
                                StringObject::allocate(
                                    gc,
                                    str_ptr as *mut u8,
                                    Value::from_heap_ptr(ptr),
                                )
                            } else {
                                StringObject::allocate(gc, str_ptr as *mut u8, Value::undefined())
                            }
                        } else {
                            StringObject::allocate(gc, str_ptr as *mut u8, Value::undefined())
                        };
                        self.push(Value::from_heap_ptr(str_obj as *mut u8));
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // §20.4.1.1: `new Symbol()` throws a TypeError — Symbol is not a constructor.
                    if constructor == self.symbol_ctor {
                        if let Some(exit) = self.throw_type_error(gc, "Symbol is not a constructor")
                        {
                            return exit;
                        }
                        continue;
                    }
                    // B1f-1: `new Number(x)` returns the number primitive.
                    // (Real Number wrapper objects are B2 work; the S15
                    // length families only extract the primitive. Returning
                    // the primitive keeps them green without regressing the
                    // Number suite, which already fails without wrappers.)
                    if constructor == self.number_constructor {
                        let result =
                            crate::builtins::number_builtin(gc, Value::undefined(), &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // §20.1.1.1: `new Object(...)` — fresh empty object with
                    // %Object.prototype% as [[Prototype]].
                    if constructor == self.object_constructor {
                        let shape = Shape::empty();
                        let proto_ptr = self.object_prototype.heap_ptr();
                        let obj = JSObject::allocate(gc, shape, &[]);
                        if let Some(pp) = proto_ptr {
                            unsafe {
                                JSObject::set_prototype(obj, pp);
                            }
                        }
                        self.push(Value::from_heap_ptr(obj as *mut u8));
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // Promise constructor [[Construct]] / [[Call]]
                    if constructor == self.promise_constructor {
                        let result = crate::builtins::promise_constructor(
                            gc,
                            Value::undefined(),
                            &args,
                            self,
                        );
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        if self.pending_promise_ctor.is_some()
                            || self.pending_array_op.is_some()
                            || self.pending_call.is_some()
                        {
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // Map/Set constructor [[Construct]]: allocate the tagged
                    // collection object and fill it from the iterable argument.
                    if constructor == self.map_constructor || constructor == self.set_constructor {
                        let is_map = constructor == self.map_constructor;
                        let proto_ptr = if is_map {
                            self.map_prototype.heap_ptr()
                        } else {
                            self.set_prototype.heap_ptr()
                        };
                        let obj_ptr = if is_map {
                            RuneMap::allocate(gc, proto_ptr.unwrap_or(std::ptr::null_mut()))
                        } else {
                            RuneSet::allocate(gc, proto_ptr.unwrap_or(std::ptr::null_mut()))
                        };
                        let obj_val = Value::from_heap_ptr(obj_ptr);
                        let result = if is_map {
                            crate::builtins::map_constructor(gc, obj_val, &args, self)
                        } else {
                            crate::builtins::set_constructor(gc, obj_val, &args, self)
                        };
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        if self.pending_collection_ctor.is_some() {
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // B1d: Array constructor [[Construct]]: 0 args → [],
                    // single Number → length form, else elements.
                    if constructor == self.array_constructor {
                        let result =
                            crate::builtins::array_constructor(gc, Value::undefined(), &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // Date constructor [[Construct]]: allocate the tagged
                    // RuneDate and compute its time value from the arguments.
                    if constructor == self.date_constructor {
                        let proto_ptr = self.date_prototype.heap_ptr();
                        let obj_ptr =
                            RuneDate::allocate(gc, proto_ptr.unwrap_or(std::ptr::null_mut()));
                        let obj_val = Value::from_heap_ptr(obj_ptr);
                        let result = crate::builtins::date_constructor(gc, obj_val, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // ArrayBuffer constructor [[Construct]]: allocate the tagged
                    // RuneArrayBuffer (zero-length) and let the builtin set the
                    // real backing block and byte length.
                    if constructor == self.array_buffer_constructor {
                        let proto_ptr = self.array_buffer_prototype.heap_ptr();
                        let obj_ptr = typedarray::RuneArrayBuffer::allocate(
                            gc,
                            0,
                            proto_ptr.unwrap_or(std::ptr::null_mut()),
                        );
                        let obj_val = Value::from_heap_ptr(obj_ptr);
                        let result =
                            crate::builtins::array_buffer_constructor(gc, obj_val, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // RegExp constructor [[Construct]]: allocate a tagged
                    // RegExp placeholder; the builtin fills in pattern, flags
                    // and lastIndex (or returns the pattern itself for a
                    // plain call — handled in the Call arm).
                    if constructor == self.regexp_constructor {
                        let empty_pat = HeapString::allocate(gc, "");
                        let obj_ptr =
                            rune_core::regexp::RegExp::allocate(gc, empty_pat as *mut u8, 0);
                        if let Some(proto_ptr) = self.regexp_prototype.heap_ptr() {
                            unsafe {
                                rune_core::regexp::RegExp::set_prototype(obj_ptr, proto_ptr);
                            }
                        }
                        let obj_val = Value::from_heap_ptr(obj_ptr);
                        let result = crate::builtins::regexp_constructor(gc, obj_val, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // TypedArray constructor [[Construct]]: find which element
                    // type this ctor wrapper corresponds to, allocate the tagged
                    // RuneTypedArray and run the per-kind builtin.
                    if !self.typed_array_ctors.is_empty() {
                        let mut kind_idx = None;
                        if let Some(ptr) = constructor.heap_ptr() {
                            for (i, c) in self.typed_array_ctors.iter().enumerate() {
                                if let Some(cp) = c.heap_ptr() {
                                    if cp == ptr {
                                        kind_idx = Some(i);
                                        break;
                                    }
                                }
                            }
                        }
                        if let Some(i) = kind_idx {
                            let proto_ptr = self.typed_array_protos[i].heap_ptr();
                            let obj_ptr = typedarray::RuneTypedArray::allocate(
                                gc,
                                proto_ptr.unwrap_or(std::ptr::null_mut()),
                            );
                            let obj_val = Value::from_heap_ptr(obj_ptr);
                            let handle = self.typed_array_ctor_handles[i];
                            let id = ((-handle.as_smi().unwrap()) as usize) - 1;
                            let result = (self.builtins[id].func)(gc, obj_val, &args, &mut *self);
                            if let Some(exc) = self.pending_exception.take() {
                                if let Some(exit) = self.handle_throw(gc, exc) {
                                    return exit;
                                }
                                continue;
                            }
                            self.push(result);
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    // Error-family constructors: new TypeError(message[, options])
                    if let Some(ptr) = constructor.heap_ptr() {
                        if let Some(ti) = self
                            .error_ctors
                            .iter()
                            .position(|c| c.heap_ptr() == Some(ptr))
                        {
                            let result = crate::builtins::error_constructor(gc, ti, &args, self);
                            if let Some(exc) = self.pending_exception.take() {
                                if let Some(exit) = self.handle_throw(gc, exc) {
                                    return exit;
                                }
                                continue;
                            }
                            self.push(result);
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    // Create a new empty object
                    let shape = Shape::empty();
                    let obj = JSObject::allocate(gc, shape, &[]);
                    let obj_val = Value::from_heap_ptr(obj as *mut u8);
                    // If constructor is a builtin, call it with the new object as `this`
                    if let Some(smi_val) = constructor.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                // §20.5.2.4: only Test262Error (the harness error
                                // ctor) is constructible among Smi-handle builtins —
                                // everything else throws "not a constructor"
                                // (previously: silently invoked as a constructor,
                                // so `new Error.prototype.toString()` returned an
                                // object instead of throwing).
                                if self.builtins[id].name != "Test262Error" {
                                    let exc = crate::errors::error_object(
                                        gc,
                                        &self.error_protos,
                                        crate::errors::ErrorKind::TypeError,
                                        &format!("{} is not a constructor", self.builtins[id].name),
                                    );
                                    if let Some(exit) = self.handle_throw(gc, exc) {
                                        return exit;
                                    }
                                    continue;
                                }
                                let result =
                                    (self.builtins[id].func)(gc, obj_val, &args, &mut *self);
                                if let Some(exc) = self.pending_exception.take() {
                                    if let Some(exit) = self.handle_throw(gc, exc) {
                                        return exit;
                                    }
                                    continue;
                                }
                                if result.is_heap_object() {
                                    self.push(result);
                                } else {
                                    self.push(obj_val);
                                }
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        }
                    }
                    // Set prototype from constructor.prototype
                    // §11.2.2 [[Construct]]: new object's [[Prototype]] = constructor.prototype
                    // Use interned PROTOTYPE_KEY to avoid HeapString allocation.
                    if constructor.is_heap_object() {
                        if let Some(ptr) = constructor.heap_ptr() {
                            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                            if tag == TAG_OBJECT {
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                if let Some(slot) = shape.lookup(&PROTOTYPE_KEY) {
                                    let proto_val =
                                        unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                                    if proto_val.is_heap_object() {
                                        if let Some(proto_ptr) = proto_val.heap_ptr() {
                                            unsafe {
                                                JSObject::set_prototype(obj, proto_ptr);
                                            }
                                        }
                                    }
                                }
                            } else if tag == TAG_FUNC {
                                // User-defined function: read prototype from Func struct
                                let proto_ptr = unsafe { Func::prototype(ptr as *mut Func) };
                                if !proto_ptr.is_null() {
                                    unsafe {
                                        JSObject::set_prototype(obj, proto_ptr);
                                    }
                                }
                            }
                        }
                    }
                    // If constructor is a user-defined function, call its body with this = new object
                    if let Some(ptr) = constructor.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_FUNC {
                            // §16.2.1.1.1: Arrow functions have [[Construct]]: undefined
                            if unsafe { Func::is_arrow(ptr as *mut Func) } {
                                let msg = crate::errors::error_object(
                                    gc,
                                    &self.error_protos,
                                    crate::errors::ErrorKind::TypeError,
                                    "Arrow function is not a constructor",
                                );
                                self.push(msg);
                                let val = self.pop();
                                // Manually unwind through try_stack like Opcode::Throw does
                                let handler_idx = self
                                    .try_stack
                                    .iter()
                                    .rposition(|tf| tf.frame_depth == self.frames.len());
                                if let Some(idx) = handler_idx {
                                    let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                        let tf = &self.try_stack[idx];
                                        (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                                    };
                                    if in_catch && finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[fi].pc = finally_pc;
                                        continue;
                                    }
                                    if catch_pc != 0 && !in_catch {
                                        if finally_pc != 0 {
                                            self.try_stack[idx].in_catch = true;
                                        } else {
                                            self.try_stack.remove(idx);
                                        }
                                        self.stack.truncate(stack_depth);
                                        self.push(val);
                                        self.frames[fi].pc = catch_pc;
                                        continue;
                                    }
                                    if finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[fi].pc = finally_pc;
                                        continue;
                                    }
                                }
                                // No handler — pop frame and check caller
                                let popped_frame = self.frames.len() - 1;
                                self.last_locals = self.frames[popped_frame].locals.clone();
                                self.frames.pop();
                                self.try_stack
                                    .retain(|tf| tf.frame_depth != popped_frame + 1);
                                if self.frames.is_empty() {
                                    self.stack.clear();
                                    return Exit::Throw(val);
                                }
                                let new_fi = self.frames.len() - 1;
                                let caller_idx = self
                                    .try_stack
                                    .iter()
                                    .rposition(|tf| tf.frame_depth == self.frames.len());
                                if let Some(idx) = caller_idx {
                                    let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                        let tf = &self.try_stack[idx];
                                        (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                                    };
                                    if in_catch && finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[new_fi].pc = finally_pc;
                                        continue;
                                    }
                                    if catch_pc != 0 && !in_catch {
                                        if finally_pc != 0 {
                                            self.try_stack[idx].in_catch = true;
                                        } else {
                                            self.try_stack.remove(idx);
                                        }
                                        self.stack.truncate(stack_depth);
                                        self.push(val);
                                        self.frames[new_fi].pc = catch_pc;
                                        continue;
                                    }
                                    if finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[new_fi].pc = finally_pc;
                                        continue;
                                    }
                                }
                                self.stack.clear();
                                return Exit::Throw(val);
                            }
                            let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                            let creator_prog = unsafe {
                                &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram)
                            };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];
                                let mut locals: Vec<Value> = if func_prog.named_function {
                                    vec![constructor]
                                } else {
                                    vec![]
                                };
                                let passed_argc = args.len();
                                locals.extend(args);
                                let func_ptr = ptr as *mut Func;
                                let func_env = unsafe { Func::env_ptr(func_ptr) };
                                self.frames.push(Frame {
                                    locals,
                                    lexical_slots: Vec::new(),
                                    lexical_tdz: Vec::new(),
                                    lexical_const: Vec::new(),
                                    scope_boundaries: Vec::new(),
                                    passed_argc,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this: obj_val,
                                    is_constructor_call: true,
                                    constructed_object: obj_val,
                                    env: func_env,
                                    func_ptr: func_ptr as *mut u8,
                                    private_name_ids: std::ptr::null_mut(),
                                });
                                continue;
                            }
                        }
                    }
                    // §13.3.5.1: `new` on a non-constructor throws a TypeError
                    // (previously returned a bare object — a miscompile).
                    let desc = crate::builtins::value_to_js_string(constructor);
                    let exc = crate::errors::error_object(
                        gc,
                        &self.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        &format!("{desc} is not a constructor"),
                    );
                    if let Some(exit) = self.handle_throw(gc, exc) {
                        return exit;
                    }
                    continue;
                }
                Opcode::Call => {
                    let argc = instr.operands[0] as usize;
                    let mut args: Vec<Value> = (0..argc).map(|_| self.pop()).collect();
                    args.reverse();
                    let callee = self.pop();
                    let this = self.pop();

                    // Builtin dispatch: negative Smi handles
                    if let Some(smi_val) = callee.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                let result = (self.builtins[id].func)(gc, this, &args, &mut *self);
                                if let Some(exc) = self.pending_exception.take() {
                                    if let Some(exit) = self.handle_throw(gc, exc) {
                                        return exit;
                                    }
                                    continue;
                                }
                                // The callback-setup builtin (array method / .call() / assert.throws /
                                // collection method) pushed the callback frame
                                // itself — skip the result push and pc advance
                                // so the Return handler's state machine owns
                                // the pc. Only applies when THIS frame is
                                // below the pending op's callback frame
                                // (source_frame_depth = the callback's index):
                                // builtin calls made INSIDE the user callback
                                // must complete normally. F4: membership lives
                                // in pending_owns_call (single place).
                                let skip = self.pending_owns_call(fi);
                                if skip {
                                    continue;
                                }
                                self.push(result);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        } else {
                            // Positive Smi: generator handle — push undefined
                            self.push(Value::undefined());
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }

                    // String constructor called as a function (not new)
                    if callee == self.string_constructor {
                        let result = string_builtin(gc, this, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        if self.pending_call.is_some() {
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }

                    // §20.1.1.1: Object(value) called as a function.
                    if callee == self.object_constructor {
                        let result = crate::builtins::object_builtin(gc, this, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }

                    // Number constructor called as a function (not new)
                    if callee == self.number_constructor {
                        let result = number_builtin(gc, this, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }

                    // B1d: Array() called as a function behaves as new Array().
                    if callee == self.array_constructor {
                        let result =
                            crate::builtins::array_constructor(gc, Value::undefined(), &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }

                    // Symbol constructor called as a function: Symbol(description)
                    if callee == self.symbol_ctor {
                        let result = crate::builtins::symbol_ctor_builtin(gc, this, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        if self.pending_call.is_some() {
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }

                    // Promise constructor called as a function (not new)
                    if callee == self.promise_constructor {
                        let result = crate::builtins::promise_constructor(gc, this, &args, self);
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        if self.pending_promise_ctor.is_some()
                            || self.pending_array_op.is_some()
                            || self.pending_call.is_some()
                        {
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }
                    // §27.1.1.1 / §27.2.1.1: Map/Set are constructors only — a
                    // plain call throws a TypeError.
                    if callee == self.map_constructor || callee == self.set_constructor {
                        let exc = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Constructor Map requires 'new'",
                        );
                        if let Some(exit) = self.handle_throw(gc, exc) {
                            return exit;
                        }
                        continue;
                    }

                    // §25.1.2.1 / §23.2.2: ArrayBuffer and the typed array
                    // constructors are constructors only — a plain call
                    // throws a TypeError.
                    if callee == self.array_buffer_constructor
                        || self.typed_array_ctors.contains(&callee)
                    {
                        let exc = crate::errors::error_object(
                            gc,
                            &self.error_protos,
                            crate::errors::ErrorKind::TypeError,
                            "Constructor requires 'new'",
                        );
                        if let Some(exit) = self.handle_throw(gc, exc) {
                            return exit;
                        }
                        continue;
                    }

                    // §21.4.2.1: Date() called without `new` returns the
                    // string ToDateString(now).
                    if callee == self.date_constructor {
                        let tv = date::now_ms();
                        let s = date::to_date_string(tv);
                        self.push(Value::from_heap_ptr(crate::vm::heap_string(gc, &s)));
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }

                    // §22.2.4.1: RegExp() called without `new` — returns the
                    // pattern itself if it's a RegExp with no flags argument,
                    // otherwise creates a new RegExp.
                    if callee == self.regexp_constructor {
                        let result = crate::builtins::regexp_constructor(
                            gc,
                            Value::undefined(),
                            &args,
                            self,
                        );
                        if let Some(exc) = self.pending_exception.take() {
                            if let Some(exit) = self.handle_throw(gc, exc) {
                                return exit;
                            }
                            continue;
                        }
                        self.push(result);
                        self.frames[fi].pc = pc + 1;
                        continue;
                    }

                    // The test262 `assert` wrapper object (holds sameValue/
                    // notSameValue/throws) is directly callable:
                    // `assert(cond, msg)` → the "assert" builtin. Previously a
                    // silent `undefined` — a vacuous pass for every bare
                    // `assert(...)` in the suites.
                    if let Some(ptr) = callee.heap_ptr() {
                        if self
                            .builtin_wrappers
                            .get("assert")
                            .and_then(|v| v.heap_ptr())
                            == Some(ptr)
                        {
                            if let Some(id) = self.builtins.iter().position(|b| b.name == "assert")
                            {
                                let result = (self.builtins[id].func)(gc, this, &args, &mut *self);
                                if let Some(exc) = self.pending_exception.take() {
                                    if let Some(exit) = self.handle_throw(gc, exc) {
                                        return exit;
                                    }
                                    continue;
                                }
                                self.push(result);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        }
                    }

                    // §20.5.1.1 / §20.5.6.1.1: Error and the native errors are
                    // callable without `new` — they return a new error object.
                    if let Some(ptr) = callee.heap_ptr() {
                        if let Some(ti) = self
                            .error_ctors
                            .iter()
                            .position(|c| c.heap_ptr() == Some(ptr))
                        {
                            let result = crate::builtins::error_constructor(gc, ti, &args, self);
                            if let Some(exc) = self.pending_exception.take() {
                                if let Some(exit) = self.handle_throw(gc, exc) {
                                    return exit;
                                }
                                continue;
                            }
                            self.push(result);
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }

                    if let Some(ptr) = callee.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_FUNC {
                            // A3: [[Call]] on a class constructor throws
                            // (spec §10.2.3 — classes are construct-only).
                            // super() calls carry operand 1 = 1 (construct
                            // form) and are exempt. Throws never populate
                            // call ICs, so no JIT path can bypass this.
                            if unsafe { Func::is_class_constructor(ptr as *mut Func) } {
                                let is_super = instr.operands.get(1).copied().unwrap_or(0) != 0;
                                if !is_super {
                                    let err = crate::errors::error_object(
                                        gc,
                                        &self.error_protos,
                                        crate::errors::ErrorKind::TypeError,
                                        "Class constructor cannot be invoked without 'new'",
                                    );
                                    if let Some(exit) = self.handle_throw(gc, err) {
                                        return exit;
                                    }
                                    continue;
                                }
                            }
                            let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                            let creator_prog = unsafe {
                                &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram)
                            };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];

                                if func_prog.is_async {
                                    let passed_argc = args.len();
                                    let mut g = Generator::new(
                                        args.clone(),
                                        func_prog as *const BytecodeProgram,
                                    );
                                    g.this = this;
                                    g.env = unsafe { Func::env_ptr(ptr as *mut Func) };
                                    g.started = true;
                                    let gen_id = self.generators.len();
                                    self.generators.push(g);
                                    let proto = self.promise_prototype.heap_ptr();
                                    let promise_ptr = Promise::allocate(gc, proto);
                                    self.async_tasks.push(AsyncTask {
                                        gen_id,
                                        promise: promise_ptr,
                                    });
                                    let func_ptr = ptr as *mut Func;
                                    let func_env = unsafe { Func::env_ptr(func_ptr) };
                                    let mut locals = if func_prog.named_function {
                                        vec![callee]
                                    } else {
                                        vec![]
                                    };
                                    locals.extend(args);
                                    self.frames.push(Frame {
                                        locals,
                                        lexical_slots: Vec::new(),
                                        lexical_tdz: Vec::new(),
                                        lexical_const: Vec::new(),
                                        scope_boundaries: Vec::new(),
                                        passed_argc,
                                        pc: 0,
                                        stack_base: self.stack.len(),
                                        prog: func_prog as *const BytecodeProgram,
                                        generator_id: Some(gen_id),
                                        this,
                                        is_constructor_call: false,
                                        constructed_object: Value::undefined(),
                                        env: func_env,
                                        func_ptr: func_ptr as *mut u8,
                                        private_name_ids: std::ptr::null_mut(),
                                    });
                                    continue;
                                }
                                if func_prog.is_generator {
                                    // Mirror the interpreter's locals layout:
                                    // named functions reserve slot 0 for the
                                    // callee (params shift by one).
                                    let mut gen_locals = if func_prog.named_function {
                                        vec![callee]
                                    } else {
                                        vec![]
                                    };
                                    gen_locals.extend(args);
                                    let mut g = Generator::new(
                                        gen_locals,
                                        func_prog as *const BytecodeProgram,
                                    );
                                    g.this = this;
                                    g.env = unsafe { Func::env_ptr(ptr as *mut Func) };
                                    let gen_id = self.generators.len();
                                    self.generators.push(g);
                                    let instance =
                                        crate::builtins::make_generator_instance(gc, self, gen_id);
                                    self.push(instance);
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                                // Phase F-1: Collect inline profile during trace recording.
                                // After the callee Func and bytecode are resolved, record
                                // the callee's JIT status, needs_frame, and size for F-2.
                                #[cfg(feature = "jit")]
                                if let Some(key) = self.recording_trace {
                                    if let Some(trace) = self.loop_traces.get_mut(&key) {
                                        let jit_entry =
                                            unsafe { Func::jit_entry(ptr as *mut Func) };
                                        trace.inline_profiles.push(
                                            rune_jit_baseline::InlineProfile {
                                                call_pc: pc,
                                                hit_count: 1,
                                                jit_count: if jit_entry.is_null() { 0 } else { 1 },
                                                callee_func_idx: func_idx as i64,
                                                callee_prog_ptr: creator_prog
                                                    as *const BytecodeProgram
                                                    as *const u8,
                                                callee_jit_entry: if jit_entry.is_null() {
                                                    None
                                                } else {
                                                    Some(jit_entry)
                                                },
                                                callee_needs_frame: func_prog.needs_frame(),
                                                callee_bytecode_size: func_prog.instructions.len()
                                                    as u32,
                                            },
                                        );
                                    }
                                }
                                // Module top-level call sites are cold-start
                                // only — skip JIT for them (their LoadGlobal
                                // reads the module env and bailout snapshots
                                // assume the shared globals model).
                                let caller_module = prog.is_module;
                                // Functions created during module evaluation
                                // never JIT: their LoadGlobal reads the module
                                // env (JIT code reads shared globals) and their
                                // bailout snapshots mix module-context depths.
                                let caller_is_module_fn =
                                    unsafe { Func::module_mi(ptr as *mut Func) >= 0 };
                                // --- Call IC fast path ---
                                if instr.call_ic_index >= 0
                                    && !caller_module
                                    && !caller_is_module_fn
                                {
                                    let ic_idx = instr.call_ic_index as usize;
                                    if ic_idx < self.call_ics.len() {
                                        let ic = &self.call_ics[ic_idx];
                                        if ic.func_ptr == ptr && ic.argc == argc {
                                            #[cfg(feature = "jit")]
                                            let jit_entry = ic.jit_entry;
                                            #[cfg(feature = "jit")]
                                            if !jit_entry.is_null() {
                                                // IC hit: call JIT entry directly, skip all overhead.
                                                self.jit_locals_buffer.clear();
                                                if func_prog.named_function {
                                                    self.jit_locals_buffer.push(callee);
                                                }
                                                self.jit_locals_buffer.extend(args.iter().copied());
                                                let local_count = func_prog.local_names.len();
                                                while self.jit_locals_buffer.len() < local_count {
                                                    self.jit_locals_buffer.push(Value::undefined());
                                                }
                                                // Universal Frame convention (matches
                                                // rune_jit_call_helper): ALWAYS push
                                                // a callee Frame. A frameless leaf
                                                // holding locals in the shared
                                                // jit_locals_buffer dangles the
                                                // moment a nested helper call takes
                                                // the buffer (fib-class corruption).
                                                // pc is stamped with this Call's
                                                // index so a propagated bailout can
                                                // resume the chain coherently
                                                // (direct bailouts overwrite it).
                                                let func_env =
                                                    unsafe { Func::env_ptr(ptr as *mut Func) };
                                                let callee_locals =
                                                    std::mem::take(&mut self.jit_locals_buffer);
                                                let frame_fi = self.frames.len();
                                                self.frames.push(Frame {
                                                    locals: callee_locals,
                                                    lexical_slots: Vec::new(),
                                                    lexical_tdz: Vec::new(),
                                                    lexical_const: Vec::new(),
                                                    scope_boundaries: Vec::new(),
                                                    passed_argc: argc,
                                                    pc,
                                                    stack_base: self.stack.len(),
                                                    prog: func_prog as *const BytecodeProgram,
                                                    generator_id: None,
                                                    this,
                                                    is_constructor_call: false,
                                                    constructed_object: Value::undefined(),
                                                    env: func_env,
                                                    func_ptr: ptr,
                                                    private_name_ids: std::ptr::null_mut(),
                                                });
                                                let locals_ptr =
                                                    self.frames[frame_fi].locals.as_mut_ptr()
                                                        as *mut u64;
                                                self.jit_entry_count += 1;
                                                let func: JitEntryFn =
                                                    unsafe { std::mem::transmute(jit_entry) };
                                                let vm_ptr = self as *mut Vm as *mut u8;
                                                let gc_ptr = gc as *mut SemiSpace as *mut u8;
                                                self.jit_bailout.pending = false;
                                                JIT_CALL_DEPTH.with(|d| d.set(d.get() + 1));
                                                let result_raw =
                                                    unsafe { func(vm_ptr, gc_ptr, locals_ptr) };
                                                JIT_CALL_DEPTH
                                                    .with(|d| d.set(d.get().saturating_sub(1)));
                                                if self.jit_bailout.pending {
                                                    let bailout_bc_pc = self.jit_bailout.bc_pc;
                                                    self.jit_bailout.pending = false;
                                                    self.jit_bailout.bc_pc = 0;
                                                    let snapshot = std::mem::take(
                                                        &mut self.jit_bailout.stack_snapshot,
                                                    );
                                                    validate_bailout_snapshot(
                                                        self.bailout_tables
                                                            .get(&(jit_entry as usize))
                                                            .map(|b| b.as_ref()),
                                                        bailout_bc_pc,
                                                        snapshot.len(),
                                                        "call-ic",
                                                    );
                                                    // Frame chain stays stacked (helpers
                                                    // keep theirs on pending too); resume
                                                    // the top frame — the innermost bailed
                                                    // unit — at its bailout PC.
                                                    let cf = self.frames.len() - 1;
                                                    self.frames[cf].pc = bailout_bc_pc;
                                                    for val in snapshot {
                                                        self.push(Value::from_raw(val));
                                                    }
                                                    continue;
                                                }
                                                let top = self.frames.len() - 1;
                                                self.last_locals =
                                                    std::mem::take(&mut self.frames[top].locals);
                                                self.frames.pop();
                                                self.push(Value::from_raw(result_raw));
                                                self.frames[fi].pc = pc + 1;
                                                continue;
                                            }
                                        }
                                    }
                                }

                                // --- JIT tier-up (if enabled) ---
                                #[cfg(all(feature = "jit", target_arch = "aarch64"))]
                                if !caller_module && !caller_is_module_fn {
                                    unsafe { Func::increment_call_count(ptr as *mut Func) };
                                    let count = unsafe { Func::call_count(ptr as *mut Func) };
                                    const JIT_THRESHOLD: u32 = 50;
                                    const MIN_JIT_FUNCTION_SIZE: usize = 3;

                                    let large_enough =
                                        func_prog.instructions.len() >= MIN_JIT_FUNCTION_SIZE;

                                    if unsafe { Func::jit_entry(ptr as *mut Func) }.is_null()
                                        && count == JIT_THRESHOLD
                                        && large_enough
                                        && rune_jit_baseline::is_jit_compatible(func_prog)
                                        // Functions whose loops churn
                                        // per-iteration envs stay interpreted:
                                        // MakeEnv allocates via the lexical
                                        // helper, and a GC mid-native with
                                        // JIT-stack raw values live is not
                                        // yet GC-safe (see afpc::aot_safe).
                                        && !func_prog.instructions.iter().any(|i| {
                                            matches!(
                                                i.opcode,
                                                Opcode::MakeEnv | Opcode::RestoreEnv
                                            )
                                        })
                                    {
                                        #[cfg(target_arch = "x86_64")]
                                        let compiled = {
                                            let codegen =
                                                CodeGen::new(func_prog.instructions.len());
                                            codegen.compile(func_prog)
                                        };
                                        #[cfg(target_arch = "aarch64")]
                                        let compiled = {
                                            let codegen =
                                                Aarch64CodeGen::new(func_prog.instructions.len())
                                                    .with_stencil_jit(self.stencil_jit);
                                            codegen.compile(func_prog)
                                        };
                                        #[cfg(not(any(
                                            target_arch = "x86_64",
                                            target_arch = "aarch64"
                                        )))]
                                        let compiled = {
                                            let _ = func_prog;
                                            unreachable!("JIT not supported on this architecture")
                                        };
                                        compiled.mem.make_executable();
                                        let entry = compiled.mem.code_ptr();
                                        unsafe {
                                            Func::set_jit_entry(ptr as *mut Func, entry);
                                        }
                                        self.bailout_tables.insert(
                                            entry as usize,
                                            Box::new(compiled.bailout_table),
                                        );
                                        std::mem::forget(compiled.mem);
                                    }

                                    let jit_entry = unsafe { Func::jit_entry(ptr as *mut Func) };
                                    if !jit_entry.is_null() && large_enough {
                                        if instr.call_ic_index >= 0 {
                                            let ic_idx = instr.call_ic_index as usize;
                                            if ic_idx >= self.call_ics.len() {
                                                self.call_ics
                                                    .resize(ic_idx + 1, CallIcEntry::default());
                                            }
                                            self.call_ics[ic_idx] = CallIcEntry {
                                                func_ptr: ptr,
                                                jit_entry,
                                                argc,
                                            };
                                        }
                                        self.jit_locals_buffer.clear();
                                        if func_prog.named_function {
                                            self.jit_locals_buffer.push(callee);
                                        }
                                        self.jit_locals_buffer.extend(args.iter().copied());
                                        let local_count = func_prog.local_names.len();
                                        while self.jit_locals_buffer.len() < local_count {
                                            self.jit_locals_buffer.push(Value::undefined());
                                        }
                                        // Universal Frame convention (matches
                                        // rune_jit_call_helper): ALWAYS push
                                        // a callee Frame — a frameless leaf's
                                        // shared-buffer locals dangle the
                                        // moment a nested helper call takes
                                        // the buffer. The lexical helper
                                        // targets the top frame, so this also
                                        // keeps lexical ops correct. On bailout
                                        // the chain is kept (pc stamped) so the
                                        // interpreter resumes it (§10.1).
                                        let func_env = unsafe { Func::env_ptr(ptr as *mut Func) };
                                        let callee_locals =
                                            std::mem::take(&mut self.jit_locals_buffer);
                                        let frame_fi = self.frames.len();
                                        self.frames.push(Frame {
                                            locals: callee_locals,
                                            lexical_slots: Vec::new(),
                                            lexical_tdz: Vec::new(),
                                            lexical_const: Vec::new(),
                                            scope_boundaries: Vec::new(),
                                            passed_argc: args.len(),
                                            pc,
                                            stack_base: self.stack.len(),
                                            prog: func_prog as *const BytecodeProgram,
                                            generator_id: None,
                                            this,
                                            is_constructor_call: false,
                                            constructed_object: Value::undefined(),
                                            env: func_env,
                                            func_ptr: ptr,
                                            private_name_ids: std::ptr::null_mut(),
                                        });
                                        let locals_ptr =
                                            self.frames[frame_fi].locals.as_mut_ptr() as *mut u64;
                                        self.jit_entry_count += 1;
                                        let func: JitEntryFn =
                                            unsafe { std::mem::transmute(jit_entry) };
                                        let vm_ptr = self as *mut Vm as *mut u8;
                                        let gc_ptr = gc as *mut SemiSpace as *mut u8;
                                        self.jit_bailout.pending = false;
                                        let result_raw =
                                            unsafe { func(vm_ptr, gc_ptr, locals_ptr) };
                                        if self.jit_bailout.pending {
                                            let bailout_bc_pc = self.jit_bailout.bc_pc;
                                            self.jit_bailout.pending = false;
                                            self.jit_bailout.bc_pc = 0;
                                            let snapshot = std::mem::take(
                                                &mut self.jit_bailout.stack_snapshot,
                                            );
                                            validate_bailout_snapshot(
                                                self.bailout_tables
                                                    .get(&(jit_entry as usize))
                                                    .map(|b| b.as_ref()),
                                                bailout_bc_pc,
                                                snapshot.len(),
                                                "tier-up",
                                            );
                                            // Frame chain stays stacked (helpers keep
                                            // theirs on pending too); resume the top
                                            // frame — the innermost bailed unit — at
                                            // its bailout PC.
                                            let cf = self.frames.len() - 1;
                                            self.frames[cf].pc = bailout_bc_pc;
                                            for val in snapshot {
                                                self.push(Value::from_raw(val));
                                            }
                                            continue;
                                        }
                                        let top = self.frames.len() - 1;
                                        self.last_locals =
                                            std::mem::take(&mut self.frames[top].locals);
                                        self.frames.pop();
                                        self.push(Value::from_raw(result_raw));
                                        self.frames[fi].pc = pc + 1;
                                        continue;
                                    }
                                }
                                // --- End JIT tier-up ---
                                let func_ptr = ptr as *mut Func;
                                let func_env = unsafe { Func::env_ptr(func_ptr) };
                                let mut locals: Vec<Value> = if func_prog.named_function {
                                    vec![callee]
                                } else {
                                    vec![]
                                };
                                let passed_argc = args.len();
                                locals.extend(args);
                                self.frames.push(Frame {
                                    locals,
                                    lexical_slots: Vec::new(),
                                    lexical_tdz: Vec::new(),
                                    lexical_const: Vec::new(),
                                    scope_boundaries: Vec::new(),
                                    passed_argc,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this,
                                    is_constructor_call: false,
                                    constructed_object: Value::undefined(),
                                    env: func_env,
                                    func_ptr: func_ptr as *mut u8,
                                    private_name_ids: std::ptr::null_mut(),
                                });
                                continue;
                            }
                        }
                    }
                    // §13.3.6.1: calling a non-callable throws a TypeError
                    // (previously a silent `undefined` — a miscompile).
                    let desc = crate::builtins::value_to_js_string(callee);
                    let exc = crate::errors::error_object(
                        gc,
                        &self.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        &format!("{desc} is not a function"),
                    );
                    if let Some(exit) = self.handle_throw(gc, exc) {
                        return exit;
                    }
                    continue;
                }
                Opcode::Return => {
                    debug_assert!(
                        self.stack.len() > self.frames.last().unwrap().stack_base,
                        "Return: stack underflow (len={}, base={})",
                        self.stack.len(),
                        self.frames.last().unwrap().stack_base,
                    );
                    debug_assert!(
                        self.stack.len() <= self.frames.last().unwrap().stack_base + 2,
                        "Return: stack too deep (len={}, base={})",
                        self.stack.len(),
                        self.frames.last().unwrap().stack_base,
                    );
                    let result = self.pop();
                    let callee_base = self.frames.last().unwrap().stack_base;
                    let gen_id = self.frames.last().unwrap().generator_id;
                    if let Some(id) = gen_id {
                        self.generators[id].done = true;
                    }
                    let is_async_return =
                        gen_id.is_some_and(|id| self.async_tasks.iter().any(|t| t.gen_id == id));
                    let async_promise_ptr = if is_async_return {
                        self.async_tasks
                            .iter()
                            .find(|t| t.gen_id == gen_id.unwrap())
                            .map(|t| t.promise)
                    } else {
                        None
                    };
                    let popped_frame = self.frames.len() - 1;
                    let is_constructor = self.frames[popped_frame].is_constructor_call;
                    let constructed_obj = self.frames[popped_frame].constructed_object;
                    let popped_func = self.frames[popped_frame].func_ptr;
                    self.last_locals = self.frames[popped_frame].locals.clone();
                    // B1e: callee identity for machine Return arms (nested
                    // callback pushes clobber depths via rebase — arms that
                    // record their awaited callee compare against this).
                    self.last_popped_callee = if popped_func.is_null() {
                        Value::undefined()
                    } else {
                        Value::from_heap_ptr(popped_func)
                    };
                    self.frames.pop();
                    self.try_stack
                        .retain(|tf| tf.frame_depth != popped_frame + 1);
                    // Generator nested-loop return: hand the value to the
                    // Rust resume caller. Do NOT touch the outside operand
                    // stack and do NOT continue into parent frames here.
                    if self.nested_gen_floor == Some(self.frames.len()) {
                        self.nested_gen_floor = None;
                        return Exit::Return(result);
                    }
                    // Check if this return completes a pending array operation callback.
                    if let Some(mut op) = self.pending_array_op.take() {
                        if self.frames.len() == op.source_frame_depth {
                            // B1a: element-getter await — `result` is the
                            // resolved element value, not a callback result.
                            if let Some(idx) = op.awaiting_element.take() {
                                // B1b: search-kind awaits resume the search
                                // (no user callback involved).
                                if matches!(
                                    op.kind,
                                    ArrayOpKind::IndexOf
                                        | ArrayOpKind::LastIndexOf
                                        | ArrayOpKind::Includes
                                ) {
                                    match array_search_step(self, gc, &mut op, Some((idx, result)))
                                    {
                                        SearchStepOut::Done(v) => {
                                            self.pending_array_op = None;
                                            self.stack.truncate(callee_base);
                                            self.push(v);
                                            let frames_len = self.frames.len();
                                            self.frames[frames_len - 1].pc += 1;
                                            continue;
                                        }
                                        SearchStepOut::Wait => {
                                            self.pending_array_op = Some(op);
                                            self.rebase_pending_depths();
                                            continue;
                                        }
                                        SearchStepOut::SyncErr(e) => {
                                            if let Some(exit) = self.handle_throw(gc, e) {
                                                return exit;
                                            }
                                            continue;
                                        }
                                    }
                                }
                                // Reduce-accumulator await: finish setup —
                                // resolve the next element (may await again)
                                // or complete when none remains.
                                if op.awaiting_acc {
                                    op.awaiting_acc = false;
                                    op.accumulator = Some(result);
                                    let next = op.index;
                                    if next == usize::MAX {
                                        self.stack.truncate(callee_base);
                                        self.push(result);
                                        let frames_len = self.frames.len();
                                        self.frames[frames_len - 1].pc += 1;
                                        continue;
                                    }
                                    match array_element_value(self, gc, op.source_val, next) {
                                        ArrayElemOut::Ready(v) => {
                                            let acc = result;
                                            let cb = op.callback;
                                            let src = op.source_val;
                                            op.index = next;
                                            self.pending_array_op = Some(op);
                                            self.push_callback_call(
                                                gc,
                                                cb,
                                                Value::undefined(),
                                                vec![acc, v, Value::smi(next as i32), src],
                                            );
                                            continue;
                                        }
                                        ArrayElemOut::Wait => {
                                            op.awaiting_element = Some(next);
                                            self.pending_array_op = Some(op);
                                            self.rebase_pending_depths();
                                            continue;
                                        }
                                        ArrayElemOut::SyncErr(e) => {
                                            if let Some(exit) = self.handle_throw(gc, e) {
                                                return exit;
                                            }
                                            continue;
                                        }
                                    }
                                }
                                let cb = op.callback;
                                let cb_this = match op.kind {
                                    ArrayOpKind::Reduce | ArrayOpKind::ReduceRight => {
                                        Value::undefined()
                                    }
                                    _ => op.this_val,
                                };
                                let cb_args = match op.kind {
                                    ArrayOpKind::Reduce | ArrayOpKind::ReduceRight => {
                                        let acc = op.accumulator.unwrap_or(Value::undefined());
                                        vec![acc, result, Value::smi(idx as i32), op.source_val]
                                    }
                                    _ => vec![result, Value::smi(idx as i32), op.source_val],
                                };
                                op.index = idx;
                                self.pending_array_op = Some(op);
                                self.push_callback_call(gc, cb, cb_this, cb_args);
                                continue;
                            }
                            // This was the callback frame returning. Process result.
                            // B1b: search kinds never dispatch callbacks (their
                            // awaits resume via the branch above); dropping is
                            // safe even if somehow reached.
                            if matches!(
                                op.kind,
                                ArrayOpKind::IndexOf
                                    | ArrayOpKind::LastIndexOf
                                    | ArrayOpKind::Includes
                            ) {
                                continue;
                            }
                            match op.kind {
                                // ... existing array callback handling ...
                                // B1b: search kinds are filtered by the guard
                                // above; these arms only satisfy exhaustiveness.
                                ArrayOpKind::IndexOf
                                | ArrayOpKind::LastIndexOf
                                | ArrayOpKind::Includes => {
                                    continue;
                                }
                                ArrayOpKind::Filter => {
                                    if result.to_bool() {
                                        // B1f-3: push the raw present value via
                                        // the funnel (pairs stay pairs — getters
                                        // dispatch at read time, exactly like
                                        // the source; own-only slot reads
                                        // dropped proto-served elements to
                                        // undefined). load never dispatches or
                                        // pushes frames, so no abrupt path.
                                        let src_val = load_property_recursive(
                                            op.source_val,
                                            Value::smi(op.index as i32),
                                            None,
                                            gc,
                                        );
                                        op.result = array_result_push(gc, self, op.result, src_val);
                                    }
                                }
                                ArrayOpKind::Map => {
                                    // B1f-3 positional CreateDataProperty at the
                                    // SOURCE index (holes stay holes — 8-c-i-18):
                                    // fill sentinel up to op.index, push mapped.
                                    let mut cur = op.result;
                                    loop {
                                        let cur_len =
                                            unsafe { RuneArray::length(cur as *mut RuneArray) }
                                                as usize;
                                        if cur_len >= op.index {
                                            break;
                                        }
                                        cur = array_result_push(
                                            gc,
                                            self,
                                            cur,
                                            Value::empty_sentinel(),
                                        );
                                    }
                                    op.result = array_result_push(gc, self, cur, result);
                                }
                                ArrayOpKind::FlatMap => {
                                    let old_ptr = op.result;
                                    if result.heap_ptr().is_some_and(|p| unsafe {
                                        (*(p as *const GcHeader)).tag() == TAG_ARRAY
                                    }) {
                                        let src_ptr = result.heap_ptr().unwrap();
                                        let arr_len =
                                            unsafe { RuneArray::length(src_ptr as *mut RuneArray) };
                                        let mut cur_ptr = old_ptr;
                                        for k in 0..arr_len {
                                            let elem = unsafe {
                                                RuneArray::get_element(
                                                    src_ptr as *mut RuneArray,
                                                    k as usize,
                                                )
                                            };
                                            // B1f-3: mapped holes don't spread
                                            // (FlattenIntoArray HasProperty
                                            // gate — [[1,,3]].flatMap(x=>x)
                                            // is [1,3], not [1,,3]).
                                            if elem == Value::empty_sentinel() {
                                                continue;
                                            }
                                            let new_arr = unsafe {
                                                RuneArray::push(gc, cur_ptr as *mut RuneArray, elem)
                                            };
                                            if new_arr as *mut u8 != cur_ptr {
                                                let resolved = if unsafe {
                                                    (*(cur_ptr as *const GcHeader)).is_forwarded()
                                                } {
                                                    unsafe {
                                                        (*(cur_ptr as *const GcHeader))
                                                            .forwarding_addr()
                                                    }
                                                } else {
                                                    cur_ptr
                                                };
                                                if resolved != new_arr as *mut u8 {
                                                    self.update_heap_reference(
                                                        resolved,
                                                        new_arr as *mut u8,
                                                    );
                                                }
                                                cur_ptr = new_arr as *mut u8;
                                            }
                                        }
                                        op.result = cur_ptr;
                                    } else {
                                        let new_arr = unsafe {
                                            RuneArray::push(gc, old_ptr as *mut RuneArray, result)
                                        };
                                        if new_arr as *mut u8 != old_ptr {
                                            let resolved = if unsafe {
                                                (*(old_ptr as *const GcHeader)).is_forwarded()
                                            } {
                                                unsafe {
                                                    (*(old_ptr as *const GcHeader))
                                                        .forwarding_addr()
                                                }
                                            } else {
                                                old_ptr
                                            };
                                            if resolved != new_arr as *mut u8 {
                                                self.update_heap_reference(
                                                    resolved,
                                                    new_arr as *mut u8,
                                                );
                                            }
                                        }
                                        op.result = new_arr as *mut u8;
                                    }
                                }
                                ArrayOpKind::Reduce | ArrayOpKind::ReduceRight => {
                                    op.accumulator = Some(result);
                                }
                                ArrayOpKind::ForEach => {}
                                ArrayOpKind::Find
                                | ArrayOpKind::FindIndex
                                | ArrayOpKind::FindLast
                                | ArrayOpKind::FindLastIndex => {
                                    if result.to_bool() {
                                        let found = match op.kind {
                                            ArrayOpKind::Find | ArrayOpKind::FindLast => {
                                                array_like_index(op.source_val, op.index as u32)
                                                    .unwrap_or(Value::undefined())
                                            }
                                            _ => Value::smi(op.index as i32),
                                        };
                                        let frames_len = self.frames.len();
                                        self.stack.truncate(callee_base);
                                        self.push(found);
                                        self.frames[frames_len - 1].pc += 1;
                                        continue;
                                    }
                                }
                                ArrayOpKind::Some => {
                                    if result.to_bool() {
                                        let frames_len = self.frames.len();
                                        self.stack.truncate(callee_base);
                                        self.push(Value::boolean(true));
                                        self.frames[frames_len - 1].pc += 1;
                                        continue;
                                    }
                                }
                                ArrayOpKind::Every => {
                                    if !result.to_bool() {
                                        let frames_len = self.frames.len();
                                        self.stack.truncate(callee_base);
                                        self.push(Value::boolean(false));
                                        self.frames[frames_len - 1].pc += 1;
                                        continue;
                                    }
                                }
                            }
                            // B1a: the loop bound is fixed at entry per spec
                            // (LengthOfArrayLike runs once); length mutations
                            // during iteration (e.g. a getter side effect)
                            // do not change the visited range. HasProperty
                            // skips deleted indices. B1c: backward kinds walk
                            // down from below the current index.
                            let current_len = op.length;
                            let op_kind = op.kind;
                            let backward = array_op_is_backward(op_kind);
                            let next_index = 'search: {
                                if backward {
                                    if op.index == 0 {
                                        break 'search None::<usize>;
                                    }
                                    let mut i = op.index - 1;
                                    loop {
                                        if has_property(
                                            op.source_val,
                                            Value::smi(i as i32),
                                            Some(self.function_prototype),
                                        ) {
                                            break 'search Some(i);
                                        }
                                        if i == 0 {
                                            break 'search None::<usize>;
                                        }
                                        i -= 1;
                                    }
                                }
                                op.index += 1;
                                // Walk forward to the next existing element
                                // (HasProperty semantics: proto-chain presence,
                                // so setter-only proto accessors count).
                                let mut i = op.index;
                                while i < current_len as usize {
                                    if has_property(
                                        op.source_val,
                                        Value::smi(i as i32),
                                        Some(self.function_prototype),
                                    ) {
                                        break 'search Some(i);
                                    }
                                    i += 1;
                                }
                                None::<usize>
                            };
                            if let Some(i) = next_index {
                                op.index = i;
                                // B1a: accessor elements dispatch getters (the
                                // walk established presence; this resolves).
                                let resolved_val =
                                    match array_element_value(self, gc, op.source_val, i) {
                                        ArrayElemOut::Ready(v) => v,
                                        ArrayElemOut::Wait => {
                                            op.awaiting_element = Some(i);
                                            self.pending_array_op = Some(op);
                                            self.rebase_pending_depths();
                                            continue;
                                        }
                                        ArrayElemOut::SyncErr(e) => {
                                            if let Some(exit) = self.handle_throw(gc, e) {
                                                return exit;
                                            }
                                            continue;
                                        }
                                    };
                                let cb_this = match op_kind {
                                    ArrayOpKind::Filter
                                    | ArrayOpKind::Map
                                    | ArrayOpKind::ForEach
                                    | ArrayOpKind::Find
                                    | ArrayOpKind::FindIndex
                                    | ArrayOpKind::FindLast
                                    | ArrayOpKind::FindLastIndex
                                    | ArrayOpKind::Some
                                    | ArrayOpKind::Every
                                    | ArrayOpKind::FlatMap => op.this_val,
                                    ArrayOpKind::Reduce | ArrayOpKind::ReduceRight => {
                                        Value::undefined()
                                    }
                                    ArrayOpKind::IndexOf
                                    | ArrayOpKind::LastIndexOf
                                    | ArrayOpKind::Includes => Value::undefined(),
                                };
                                let cb_args = match op_kind {
                                    ArrayOpKind::Filter
                                    | ArrayOpKind::Map
                                    | ArrayOpKind::ForEach
                                    | ArrayOpKind::Find
                                    | ArrayOpKind::FindIndex
                                    | ArrayOpKind::FindLast
                                    | ArrayOpKind::FindLastIndex
                                    | ArrayOpKind::Some
                                    | ArrayOpKind::Every
                                    | ArrayOpKind::FlatMap => {
                                        vec![resolved_val, Value::smi(i as i32), op.source_val]
                                    }
                                    ArrayOpKind::Reduce | ArrayOpKind::ReduceRight => {
                                        let acc = op.accumulator.unwrap_or(Value::undefined());
                                        vec![acc, resolved_val, Value::smi(i as i32), op.source_val]
                                    }
                                    // B1b: search kinds never reach callback
                                    // dispatch (see the guard above).
                                    ArrayOpKind::IndexOf
                                    | ArrayOpKind::LastIndexOf
                                    | ArrayOpKind::Includes => {
                                        continue;
                                    }
                                };
                                let callback_func = op.callback;
                                self.stack.truncate(callee_base);
                                self.pending_array_op = Some(op);
                                self.push_callback_call(gc, callback_func, cb_this, cb_args);
                                continue;
                            }
                            // Done: push result and advance pc.
                            let final_result = match op_kind {
                                ArrayOpKind::Filter | ArrayOpKind::FlatMap => {
                                    // B1f-6d: non-dense species result swaps in
                                    // (temp dense takes the writes).
                                    op.species_fallback
                                        .unwrap_or(Value::from_heap_ptr(op.result))
                                }
                                ArrayOpKind::Map => {
                                    // B1f-3: positional writes leave trailing
                                    // holes — set the species length explicitly
                                    // (tail past capacity reads as holes).
                                    // B1f-6d: non-dense customs swap in (temp
                                    // dense takes the writes, unwritten).
                                    match op.species_fallback {
                                        Some(fb) => fb,
                                        None => {
                                            unsafe {
                                                RuneArray::set_length(
                                                    op.result as *mut RuneArray,
                                                    op.length,
                                                )
                                            };
                                            Value::from_heap_ptr(op.result)
                                        }
                                    }
                                }
                                ArrayOpKind::Reduce | ArrayOpKind::ReduceRight => {
                                    op.accumulator.unwrap_or(Value::undefined())
                                }
                                ArrayOpKind::ForEach => Value::undefined(),
                                ArrayOpKind::Find | ArrayOpKind::FindLast => Value::undefined(),
                                ArrayOpKind::FindIndex | ArrayOpKind::FindLastIndex => {
                                    Value::smi(-1)
                                }
                                ArrayOpKind::Some => Value::boolean(false),
                                ArrayOpKind::Every => Value::boolean(true),
                                // B1b: search completions return from the
                                // awaiting branch above, never here.
                                ArrayOpKind::IndexOf
                                | ArrayOpKind::LastIndexOf
                                | ArrayOpKind::Includes => Value::undefined(),
                            };
                            let frames_len = self.frames.len();
                            self.stack.truncate(callee_base);
                            self.push(final_result);
                            self.frames[frames_len - 1].pc += 1;
                            continue;
                        } else {
                            self.pending_array_op = Some(op);
                        }
                    }
                    // Check if this return completes a pending sort/toSorted
                    // step (element getter, ToPrimitive value, comparator
                    // number, or setter call). The machine drives itself to
                    // the next Wait/Done/Raise. Fires only on a callee
                    // match: nested foreign pushes share depths after
                    // rebase but never callees (the String()-in-comparator
                    // cross-talk class).
                    if let Some(mut sop) = self.pending_sort_op.take() {
                        let callee_match = self
                            .last_popped_callee
                            .heap_ptr()
                            .is_some_and(|p| Some(p) == sop.await_callee.heap_ptr());
                        if self.frames.len() <= sop.source_frame_depth && callee_match {
                            let mut out = crate::builtins::sort_resume(self, gc, &mut sop, result);
                            loop {
                                match out {
                                    crate::builtins::SortStepOut::Wait => {
                                        self.pending_sort_op = Some(sop);
                                        self.rebase_pending_depths();
                                        continue 'run;
                                    }
                                    crate::builtins::SortStepOut::Progress => {
                                        out = crate::builtins::sort_drive(self, gc, &mut sop);
                                    }
                                    crate::builtins::SortStepOut::Done(v) => {
                                        let frames_len = self.frames.len();
                                        self.stack.truncate(callee_base);
                                        self.push(v);
                                        self.frames[frames_len - 1].pc += 1;
                                        continue 'run;
                                    }
                                    crate::builtins::SortStepOut::Raise(e) => {
                                        if let Some(exit) = self.handle_throw(gc, e) {
                                            return exit;
                                        }
                                        continue 'run;
                                    }
                                }
                            }
                        } else {
                            self.pending_sort_op = Some(sop);
                        }
                    }
                    // Check if this return completes a pending Array.from step
                    // (iterator factory/next, done/value getter, mapper, or
                    // IteratorClose return call). Same callee+depth guard as
                    // sort (B1f-4).
                    if let Some(mut fop) = self.pending_from_op.take() {
                        let callee_match = self
                            .last_popped_callee
                            .heap_ptr()
                            .is_some_and(|p| Some(p) == fop.await_callee.heap_ptr());
                        if self.frames.len() <= fop.source_frame_depth && callee_match {
                            let mut out = crate::builtins::from_resume(self, gc, &mut fop, result);
                            loop {
                                match out {
                                    crate::builtins::FromStepOut::Wait => {
                                        self.pending_from_op = Some(fop);
                                        self.rebase_pending_depths();
                                        continue 'run;
                                    }
                                    crate::builtins::FromStepOut::Progress => {
                                        out = crate::builtins::from_drive(self, gc, &mut fop);
                                    }
                                    crate::builtins::FromStepOut::Done(v) => {
                                        let frames_len = self.frames.len();
                                        self.stack.truncate(callee_base);
                                        self.push(v);
                                        self.frames[frames_len - 1].pc += 1;
                                        continue 'run;
                                    }
                                    crate::builtins::FromStepOut::Raise(e) => {
                                        if let Some(exit) = self.handle_throw(gc, e) {
                                            return exit;
                                        }
                                        continue 'run;
                                    }
                                }
                            }
                        } else {
                            self.pending_from_op = Some(fop);
                        }
                    }
                    // Check if this return completes a pending length
                    // resolution (B1f-6b: JS length getter / valueOf / toString
                    // / @@toPrimitive). Same callee+depth guard as sort/from.
                    if let Some(mut lop) = self.pending_length_op.take() {
                        let callee_match = self
                            .last_popped_callee
                            .heap_ptr()
                            .is_some_and(|p| Some(p) == lop.await_callee.heap_ptr());
                        if self.frames.len() <= lop.source_frame_depth && callee_match {
                            match crate::builtins::length_resume(self, gc, &mut lop, result) {
                                crate::builtins::LengthOut::Wait => {
                                    self.pending_length_op = Some(lop);
                                    self.rebase_pending_depths();
                                    continue 'run;
                                }
                                crate::builtins::LengthOut::Done(resolved) => {
                                    // Publish for the re-dispatched builtin,
                                    // which consumes it first-thing.
                                    self.length_resume = Some(resolved);
                                    let depth_before = self.frames.len();
                                    let r = (lop.caller)(gc, lop.stash_this, &lop.stash_args, self);
                                    if let Some(exc) = self.pending_exception.take() {
                                        if let Some(exit) = self.handle_throw(gc, exc) {
                                            return exit;
                                        }
                                        continue 'run;
                                    }
                                    if self.frames.len() > depth_before {
                                        // Re-entry armed a nested machine
                                        // (e.g. map's first JS callback):
                                        // it owns pc/stack from here.
                                        continue 'run;
                                    }
                                    let frames_len = self.frames.len();
                                    self.stack.truncate(callee_base);
                                    self.push(r);
                                    self.frames[frames_len - 1].pc += 1;
                                    continue 'run;
                                }
                                crate::builtins::LengthOut::Raise(e) => {
                                    if let Some(exit) = self.handle_throw(gc, e) {
                                        return exit;
                                    }
                                    continue 'run;
                                }
                            }
                        } else {
                            self.pending_length_op = Some(lop);
                        }
                    }
                    // Check if this return completes a pending species Get
                    // (B1f-6c: JS "constructor" / @@species getter). Same
                    // callee+depth guard as length/from.
                    if let Some(mut sop) = self.pending_species_op.take() {
                        let callee_match = self
                            .last_popped_callee
                            .heap_ptr()
                            .is_some_and(|p| Some(p) == sop.await_callee.heap_ptr());
                        if self.frames.len() <= sop.source_frame_depth && callee_match {
                            match crate::builtins::species_resume(self, gc, &mut sop, result) {
                                crate::builtins::SpeciesOut::Wait => {
                                    self.pending_species_op = Some(sop);
                                    self.rebase_pending_depths();
                                    continue 'run;
                                }
                                crate::builtins::SpeciesOut::Passed => {
                                    // Publish for the re-dispatched builtin,
                                    // which consumes it first-thing.
                                    self.species_passed = true;
                                    let depth_before = self.frames.len();
                                    let r = (sop.caller)(gc, sop.stash_this, &sop.stash_args, self);
                                    if let Some(exc) = self.pending_exception.take() {
                                        if let Some(exit) = self.handle_throw(gc, exc) {
                                            return exit;
                                        }
                                        continue 'run;
                                    }
                                    if self.frames.len() > depth_before {
                                        // Re-entry armed a nested machine
                                        // (e.g. map's first JS callback):
                                        // it owns pc/stack from here.
                                        continue 'run;
                                    }
                                    let frames_len = self.frames.len();
                                    self.stack.truncate(callee_base);
                                    self.push(r);
                                    self.frames[frames_len - 1].pc += 1;
                                    continue 'run;
                                }
                                crate::builtins::SpeciesOut::Made(made) => {
                                    // Publish the constructed object for the
                                    // re-dispatched builtin, which installs
                                    // it (B1f-6d). Shared re-dispatch body.
                                    self.species_made = Some(made);
                                    let depth_before = self.frames.len();
                                    let r = (sop.caller)(gc, sop.stash_this, &sop.stash_args, self);
                                    if let Some(exc) = self.pending_exception.take() {
                                        if let Some(exit) = self.handle_throw(gc, exc) {
                                            return exit;
                                        }
                                        continue 'run;
                                    }
                                    if self.frames.len() > depth_before {
                                        continue 'run;
                                    }
                                    let frames_len = self.frames.len();
                                    self.stack.truncate(callee_base);
                                    self.push(r);
                                    self.frames[frames_len - 1].pc += 1;
                                    continue 'run;
                                }
                                crate::builtins::SpeciesOut::Done(v) => {
                                    // Terminal value (of-length-set resume):
                                    // no re-dispatch — push as the builtin's
                                    // result (sort-Done shape).
                                    let frames_len = self.frames.len();
                                    self.stack.truncate(callee_base);
                                    self.push(v);
                                    self.frames[frames_len - 1].pc += 1;
                                    continue 'run;
                                }
                                crate::builtins::SpeciesOut::Fail(e) => {
                                    if let Some(exit) = self.handle_throw(gc, e) {
                                        return exit;
                                    }
                                    continue 'run;
                                }
                            }
                        } else {
                            self.pending_species_op = Some(sop);
                        }
                    }
                    // Check if this return completes a pending assert.throws callback.
                    if let Some(pa) = self.pending_assert.take() {
                        if self.frames.len() == pa.source_frame_depth {
                            // Function returned without throwing — assert.throws failed.
                            let expected = self.describe_expected_error(pa.expected_error);
                            let msg = format!("Expected {} to throw an exception", expected);
                            let err = make_error(gc, &self.error_protos, &msg);
                            self.stack.truncate(callee_base);
                            if let Some(exit) = self.handle_throw(gc, err) {
                                return exit;
                            }
                            continue;
                        }
                        // Nested return from inside the thunk (e.g. a setup
                        // call or `new E()`) — not the thunk itself. Keep
                        // waiting for the thunk's own return/throw.
                        self.pending_assert = Some(pa);
                    }
                    // Check if this return completes a pending @@method dispatch
                    // (String.prototype.match/search/split/replace with an object
                    // whose @@match/@@search/@@split/@@replace is callable). The
                    // method's return value is the builtin's final result.
                    if let Some(sd) = self.pending_symbol_dispatch.take() {
                        if self.frames.len() == sd.source_frame_depth {
                            self.stack.truncate(callee_base);
                            self.push(result);
                            let frames_len = self.frames.len();
                            self.frames[frames_len - 1].pc += 1;
                            continue;
                        }
                        self.pending_symbol_dispatch = Some(sd);
                    }
                    // Check if this return completes a pending for..of iterator
                    // acquisition (user-defined @@iterator factory call).
                    if let Some(pfoi) = self.pending_for_of_init.take() {
                        if self.frames.len() == pfoi.source_frame_depth {
                            if let Err(exit) = complete_for_of_init(self, gc, result) {
                                return exit;
                            }
                            let frames_len = self.frames.len();
                            self.frames[frames_len - 1].pc += 1;
                            continue;
                        }
                        self.pending_for_of_init = Some(pfoi);
                    }
                    // Check if this return completes a pending for..of iteration
                    // step (the iterator's JS `next` method returned).
                    if let Some(pfon) = self.pending_for_of_next.take() {
                        if self.frames.len() == pfon.source_frame_depth {
                            match process_for_of_next_result(self, gc, result, pfon.end_target) {
                                Ok(()) => continue,
                                Err(exit) => return exit,
                            }
                        }
                        self.pending_for_of_next = Some(pfon);
                    }
                    if let Some(pfon) = self.pending_yield_star_next.take() {
                        if self.frames.len() == pfon.source_frame_depth {
                            match process_yieldstar_next_result(self, gc, result, pfon.end_target) {
                                Ok(()) => continue,
                                Err(exit) => return exit,
                            }
                        }
                        self.pending_yield_star_next = Some(pfon);
                    }
                    if let Some(pg) = self.pending_yield_star_get.take() {
                        if self.frames.len() == pg.source_frame_depth {
                            match crate::builtins::continue_yieldstar_get(self, gc, &pg, result) {
                                Ok(()) => continue,
                                Err(exit) => return exit,
                            }
                        }
                        self.pending_yield_star_get = Some(pg);
                    }
                    if let Some(pa) = self.pending_yield_star_afr.take() {
                        if self.frames.len() == pa.source_frame_depth {
                            match crate::builtins::finish_yieldstar_afr_result(
                                self,
                                gc,
                                pa.gen_id,
                                pa.is_throw,
                                pa.abrupt_arg,
                                pa.iter,
                                pa.closing,
                                result,
                            ) {
                                Ok(v) => {
                                    self.push(v);
                                    let frames_len = self.frames.len();
                                    self.frames[frames_len - 1].pc += 1;
                                    continue;
                                }
                                Err(Some(exit)) => return exit,
                                Err(None) => continue,
                            }
                        }
                        self.pending_yield_star_afr = Some(pa);
                    }
                    // Check if this return completes a pending spread drain
                    // (ToArrayFromIterable with user-defined callbacks).
                    if let Some(pid) = self.pending_iter_drain.take() {
                        if self.frames.len() == pid.source_frame_depth {
                            match pid.state {
                                IterDrainState::AwaitFactory => {
                                    let arr = match drain_iterator(
                                        self,
                                        gc,
                                        result,
                                        pid.receiver,
                                        pid.result,
                                    ) {
                                        Ok(v) => v,
                                        Err(Some(exit)) => return exit,
                                        Err(None) => continue,
                                    };
                                    self.stack.truncate(callee_base);
                                    self.push(Value::from_heap_ptr(arr));
                                    let frames_len = self.frames.len();
                                    self.frames[frames_len - 1].pc += 1;
                                    continue;
                                }
                                IterDrainState::AwaitNext => {
                                    if !result.is_heap_object() {
                                        self.stack.truncate(callee_base);
                                        if let Some(exit) = self.throw_type_error(
                                            gc,
                                            "Iterator result is not an object",
                                        ) {
                                            return exit;
                                        }
                                        continue;
                                    }
                                    let done =
                                        load_property_recursive(result, self.done_key, None, gc)
                                            .to_bool();
                                    if done {
                                        let arr = Value::from_heap_ptr(pid.result);
                                        self.stack.truncate(callee_base);
                                        self.push(arr);
                                        let frames_len = self.frames.len();
                                        self.frames[frames_len - 1].pc += 1;
                                    } else {
                                        let value = load_property_recursive(
                                            result,
                                            self.value_key,
                                            None,
                                            gc,
                                        );
                                        let new_arr = unsafe {
                                            RuneArray::push(gc, pid.result as *mut RuneArray, value)
                                        };
                                        self.pending_iter_drain = Some(PendingIterDrain {
                                            source_frame_depth: self.frames.len() - 1,
                                            state: IterDrainState::AwaitNext,
                                            iter: pid.iter,
                                            next: pid.next,
                                            result: new_arr as *mut u8,
                                            receiver: pid.receiver,
                                        });
                                        self.push_callback_call(gc, pid.next, pid.iter, vec![]);
                                    }
                                    continue;
                                }
                            }
                        }
                        self.pending_iter_drain = Some(pid);
                    }
                    // Check if this return completes a pending Map/Set constructor
                    // fill (user-defined @@iterator factory or `next` method).
                    if let Some(pcc) = self.pending_collection_ctor.take() {
                        if self.frames.len() == pcc.source_frame_depth {
                            match pcc.state {
                                CollectionCtorState::AwaitFactory => {
                                    if !result.is_heap_object() {
                                        self.stack.truncate(pcc.root_base);
                                        if let Some(exit) =
                                            self.throw_type_error(gc, "value is not iterable")
                                        {
                                            return exit;
                                        }
                                        continue;
                                    }
                                    self.stack.truncate(pcc.root_base);
                                    let base = self.stack.len();
                                    self.stack.push(pcc.collection);
                                    self.stack.push(result);
                                    let outcome = crate::builtins::fill_collection_from_iterator(
                                        self,
                                        gc,
                                        base,
                                        base + 1,
                                        pcc.is_map,
                                    );
                                    if outcome == crate::builtins::FillOutcome::Done {
                                        let collection = self.stack[base];
                                        self.stack.truncate(pcc.root_base);
                                        self.push(collection);
                                        let frames_len = self.frames.len();
                                        self.frames[frames_len - 1].pc += 1;
                                    } else if outcome == crate::builtins::FillOutcome::Threw {
                                        self.stack.truncate(pcc.root_base);
                                        if let Some(exc) = self.pending_exception.take() {
                                            if let Some(exit) = self.handle_throw(gc, exc) {
                                                return exit;
                                            }
                                        }
                                    }
                                    continue;
                                }
                                CollectionCtorState::AwaitNext => {
                                    if !result.is_heap_object() {
                                        self.stack.truncate(pcc.root_base);
                                        if let Some(exit) = self.throw_type_error(
                                            gc,
                                            "Iterator result is not an object",
                                        ) {
                                            return exit;
                                        }
                                        continue;
                                    }
                                    let done =
                                        load_property_recursive(result, self.done_key, None, gc)
                                            .to_bool();
                                    if done {
                                        self.stack.truncate(pcc.root_base);
                                        self.push(pcc.collection);
                                        let frames_len = self.frames.len();
                                        self.frames[frames_len - 1].pc += 1;
                                        continue;
                                    }
                                    let value =
                                        load_property_recursive(result, self.value_key, None, gc);
                                    if pcc.is_map && !crate::builtins::is_object_value(value) {
                                        self.stack.truncate(pcc.root_base);
                                        if let Some(exit) = self
                                            .throw_type_error(gc, "Iterator value is not an object")
                                        {
                                            return exit;
                                        }
                                        continue;
                                    }
                                    self.stack.truncate(pcc.root_base);
                                    let base = self.stack.len();
                                    if pcc.is_map {
                                        let k =
                                            load_property_recursive(value, Value::smi(0), None, gc);
                                        let v =
                                            load_property_recursive(value, Value::smi(1), None, gc);
                                        self.stack.push(pcc.collection);
                                        self.stack.push(pcc.iter);
                                        self.stack.push(pcc.next);
                                        {
                                            let collection_slot = &mut self.stack[base];
                                            crate::builtins::map_set_internal(
                                                gc,
                                                collection_slot,
                                                k,
                                                v,
                                            );
                                        }
                                    } else {
                                        self.stack.push(pcc.collection);
                                        self.stack.push(pcc.iter);
                                        self.stack.push(pcc.next);
                                        {
                                            let collection_slot = &mut self.stack[base];
                                            crate::builtins::set_add_internal(
                                                gc,
                                                collection_slot,
                                                value,
                                            );
                                        }
                                    }
                                    // Re-read from the rooted stack slots — the GC
                                    // may have forwarded the collection/iter/next.
                                    let collection = self.stack[base];
                                    let iter = self.stack[base + 1];
                                    let next = self.stack[base + 2];
                                    self.pending_collection_ctor = Some(PendingCollectionCtor {
                                        source_frame_depth: self.frames.len() - 1,
                                        root_base: pcc.root_base,
                                        state: CollectionCtorState::AwaitNext,
                                        iter,
                                        next,
                                        collection,
                                        is_map: pcc.is_map,
                                    });
                                    self.stack.truncate(base);
                                    self.push_callback_call(gc, next, iter, vec![]);
                                    continue;
                                }
                            }
                        }
                        self.pending_collection_ctor = Some(pcc);
                    }
                    // Check if this return completes a pending Map/Set forEach
                    // dispatch (a JS callback returned).
                    if let Some(pfe) = self.pending_collection_foreach.take() {
                        if self.frames.len() == pfe.source_frame_depth {
                            let snapshot = pfe.snapshot as *mut RuneArray;
                            let mut idx = pfe.idx;
                            let mut found = pfe.found;
                            let len = unsafe { RuneArray::length(snapshot) } as usize;
                            let mut pushed = false;
                            while idx < len && found < pfe.size {
                                let k = unsafe { RuneArray::get_element(snapshot, idx) };
                                // Entries deleted before being visited are skipped.
                                let live_ptr = if pfe.is_map {
                                    let map_ptr = pfe.collection.heap_ptr().unwrap();
                                    unsafe { RuneMap::entries(map_ptr) }
                                } else {
                                    let set_ptr = pfe.collection.heap_ptr().unwrap();
                                    unsafe { RuneSet::entries(set_ptr) }
                                };
                                if let Some(live) =
                                    crate::builtins::key_index(live_ptr, k, pfe.is_map)
                                {
                                    found += 1;
                                    let v = if pfe.is_map {
                                        unsafe {
                                            RuneArray::get_element(
                                                live_ptr as *mut RuneArray,
                                                live + 1,
                                            )
                                        }
                                    } else {
                                        k
                                    };
                                    if pfe.callback.as_smi().is_some_and(|s| s < 0) {
                                        let id = (-pfe.callback.as_smi().unwrap() as usize) - 1;
                                        if id < self.builtins.len() {
                                            (self.builtins[id].func)(
                                                gc,
                                                pfe.this_arg,
                                                &[v, k, pfe.collection],
                                                self,
                                            );
                                            if self.pending_exception.is_some() {
                                                break;
                                            }
                                        }
                                    } else {
                                        self.pending_collection_foreach =
                                            Some(PendingCollectionForEach {
                                                source_frame_depth: self.frames.len() - 1,
                                                snapshot: pfe.snapshot,
                                                idx: idx + 1,
                                                found,
                                                size: pfe.size,
                                                is_map: pfe.is_map,
                                                callback: pfe.callback,
                                                this_arg: pfe.this_arg,
                                                collection: pfe.collection,
                                            });
                                        self.push_callback_call(
                                            gc,
                                            pfe.callback,
                                            pfe.this_arg,
                                            vec![v, k, pfe.collection],
                                        );
                                        pushed = true;
                                        break;
                                    }
                                }
                                idx += 1;
                            }
                            if pushed {
                                continue;
                            }
                            self.stack.truncate(callee_base);
                            if let Some(exc) = self.pending_exception.take() {
                                if let Some(exit) = self.handle_throw(gc, exc) {
                                    return exit;
                                }
                            }
                            self.push(Value::undefined());
                            let frames_len = self.frames.len();
                            self.frames[frames_len - 1].pc += 1;
                            continue;
                        }
                        self.pending_collection_foreach = Some(pfe);
                    }
                    // Check if this return completes a pending Promise constructor (executor).
                    if self.pending_promise_ctor.is_some() {
                        if let Some(ref ppc) = self.pending_promise_ctor {
                            if self.frames.len() == ppc.source_frame_depth {
                                let ppc = self.pending_promise_ctor.take().unwrap();
                                if ppc.resolve_with_result {
                                    if let Some(ptr) = ppc.promise.heap_ptr() {
                                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                                        if tag == TAG_PROMISE
                                            && unsafe { Promise::state(ptr) == PROMISE_PENDING }
                                        {
                                            unsafe {
                                                Promise::set_state(ptr, PROMISE_FULFILLED);
                                                Promise::set_result(ptr, result);
                                            }
                                            let reactions_ptr = unsafe { Promise::reactions(ptr) };
                                            if !reactions_ptr.is_null() {
                                                let arr = reactions_ptr as *mut RuneArray;
                                                let len = unsafe { RuneArray::length(arr) };
                                                let mut idx = 0;
                                                while idx + 1 < len as usize {
                                                    let cb =
                                                        unsafe { RuneArray::get_element(arr, idx) };
                                                    let chained = unsafe {
                                                        RuneArray::get_element(arr, idx + 1)
                                                    };
                                                    if cb.is_heap_object() {
                                                        let ppc2 = PendingPromiseCtor {
                                                            source_frame_depth: 0,
                                                            promise: chained,
                                                            resolve_handle: Value::undefined(),
                                                            reject_handle: Value::undefined(),
                                                            resolve_with_result: true,
                                                        };
                                                        self.enqueue_microtask(
                                                            cb,
                                                            vec![result],
                                                            Some(ppc2),
                                                        );
                                                    }
                                                    idx += 2;
                                                }
                                            }
                                        }
                                    }
                                }
                                self.stack.truncate(callee_base);
                                self.push(ppc.promise);
                                if self.frames.is_empty() {
                                    return Exit::Return(ppc.promise);
                                }
                                let caller_idx = self.frames.len() - 1;
                                self.frames[caller_idx].pc += 1;
                                continue;
                            }
                        }
                    }
                    // Check if this return completes a pending Symbol(desc)/Symbol.for(key)
                    // description coercion (the toString callback returned).
                    // MUST run before the pending_call resume below — the same
                    // return satisfies both, and only we wrap the result into a symbol.
                    if let Some(psc) = self.pending_symbol_coercion.take() {
                        if self.frames.len() == psc.source_frame_depth {
                            // The to_primitive_string callback (pending_call) is the
                            // same frame that just returned — retire it as well.
                            self.pending_call = None;
                            let sym = if psc.is_for {
                                let key = to_primitive_string_sync(result, gc, self);
                                Value::symbol(symbol_for(&key))
                            } else {
                                let desc = if result.is_symbol() {
                                    None
                                } else {
                                    Some(to_primitive_string_sync(result, gc, self))
                                };
                                Value::symbol(register_symbol(desc))
                            };
                            self.stack.truncate(callee_base);
                            self.push(sym);
                            let frames_len = self.frames.len();
                            self.frames[frames_len - 1].pc += 1;
                            continue;
                        }
                        self.pending_symbol_coercion = Some(psc);
                    }
                    // Check if this return completes a pending .call() invocation
                    // (or a to_primitive_string user-method call, which needs
                    // result post-processing per its continuation).
                    if let Some(pc) = self.pending_call.take() {
                        if self.frames.len() == pc.source_frame_depth {
                            match pc.cont {
                                PendingCallCont::Raw => {
                                    // Target function returned. Push result
                                    // and advance caller PC.
                                    self.stack.truncate(callee_base);
                                    self.push(result);
                                    let caller_idx = self.frames.len() - 1;
                                    self.frames[caller_idx].pc += 1;
                                    continue;
                                }
                                PendingCallCont::ToPrimString {
                                    value,
                                    tried_valueof,
                                } => {
                                    if crate::builtins::toprim_is_primitive(result) {
                                        if let Some(exit) =
                                            self.complete_toprim_string(gc, result, callee_base)
                                        {
                                            return exit;
                                        }
                                        continue;
                                    }
                                    if !tried_valueof {
                                        match crate::builtins::toprim_resume_valueof(
                                            self, gc, value,
                                        ) {
                                            crate::builtins::ToprimResumeOut::Ready(p) => {
                                                if let Some(exit) =
                                                    self.complete_toprim_string(gc, p, callee_base)
                                                {
                                                    return exit;
                                                }
                                                continue;
                                            }
                                            crate::builtins::ToprimResumeOut::WaitPushed => {
                                                continue;
                                            }
                                            crate::builtins::ToprimResumeOut::Raise(e) => {
                                                if let Some(exit) = self.handle_throw(gc, e) {
                                                    return exit;
                                                }
                                                continue;
                                            }
                                        }
                                    }
                                    let err = crate::errors::error_object(
                                        gc,
                                        &self.error_protos,
                                        crate::errors::ErrorKind::TypeError,
                                        "Cannot convert object to primitive value",
                                    );
                                    if let Some(exit) = self.handle_throw(gc, err) {
                                        return exit;
                                    }
                                    continue;
                                }
                            }
                        }
                    }
                    // Check if this return completes a Promise.prototype.finally callback.
                    if let Some(ref fop) = self.pending_finally_op {
                        if self.frames.len() == fop.source_frame_depth {
                            let fop = self.pending_finally_op.take().unwrap();
                            let ptr = match fop.promise.heap_ptr() {
                                Some(p) => p,
                                None => continue,
                            };
                            unsafe {
                                if fop.is_reject {
                                    Promise::set_state(ptr, PROMISE_REJECTED);
                                } else {
                                    Promise::set_state(ptr, PROMISE_FULFILLED);
                                }
                                Promise::set_result(ptr, fop.orig_value);
                            }
                            // Drain reactions on the chained promise
                            let reactions_ptr = unsafe { Promise::reactions(ptr) };
                            if !reactions_ptr.is_null() {
                                let arr = reactions_ptr as *mut RuneArray;
                                let len = unsafe { RuneArray::length(arr) };
                                let mut idx = 0;
                                while idx + 1 < len as usize {
                                    let cb = unsafe { RuneArray::get_element(arr, idx) };
                                    let chained = unsafe { RuneArray::get_element(arr, idx + 1) };
                                    if cb.is_heap_object() {
                                        let ppc = PendingPromiseCtor {
                                            source_frame_depth: 0,
                                            promise: chained,
                                            resolve_handle: Value::undefined(),
                                            reject_handle: Value::undefined(),
                                            resolve_with_result: true,
                                        };
                                        self.enqueue_microtask(cb, vec![fop.orig_value], Some(ppc));
                                    }
                                    idx += 2;
                                }
                            }
                            self.stack.truncate(callee_base);
                            self.push(fop.promise);
                            if self.frames.is_empty() {
                                self.stack.clear();
                                return Exit::Return(fop.promise);
                            }
                            let new_fi = self.frames.len() - 1;
                            self.frames[new_fi].pc += 1;
                            continue;
                        }
                    }
                    // Check if this return completes an accessor (getter/setter) call.
                    if let Some(acc) = self.pending_accessor_call.take() {
                        if self.frames.len() == acc.source_frame_depth {
                            let caller_idx = self.frames.len() - 1;
                            self.stack.truncate(callee_base);
                            self.push(result);
                            self.frames[caller_idx].pc += 1;
                            continue;
                        }
                    }
                    // Check if this return completes a pending primitive conversion.
                    if let Some(pc) = self.pending_primitive_conversion.take() {
                        if self.frames.len() == pc.source_frame_depth {
                            // Conversion callback returned. Push result + saved operand
                            // but do NOT advance the PC, so the original opcode re-executes
                            // with the converted primitive on the stack.
                            self.stack.truncate(callee_base);
                            self.push(result);
                            self.push(pc.other_operand);
                            continue;
                        }
                    }
                    // Check if this return completes a String.prototype.replace callback.
                    if let Some(ref pro) = self.pending_replace_op {
                        if self.frames.len() == pro.source_frame_depth {
                            let pro = self.pending_replace_op.take().unwrap();
                            let repl_str = crate::builtins::value_to_js_string(result);
                            let (start, end) = pro.groups[0];
                            let final_str =
                                pro.input[..start].to_string() + &repl_str + &pro.input[end..];
                            let ptr = HeapString::allocate(gc, &final_str);
                            self.stack.truncate(callee_base);
                            self.push(Value::from_heap_ptr(ptr as *mut u8));
                            let caller_idx = self.frames.len() - 1;
                            self.frames[caller_idx].pc += 1;
                            continue;
                        }
                    }
                    // Check if this return completes a String.prototype.replaceAll
                    // callback: append the substitution, find the next match and
                    // either invoke the callback again or finish.
                    if let Some(ref pra) = self.pending_replace_all_op {
                        if self.frames.len() == pra.source_frame_depth {
                            let mut pra = self.pending_replace_all_op.take().unwrap();
                            let repl_str = crate::builtins::value_to_js_string(result);
                            pra.accumulated.push_str(&repl_str);
                            let input = pra.input.clone();
                            let search_str = pra.search_str.clone();
                            let regex_pattern = pra.regex_pattern.clone();
                            let regex_flags = pra.regex_flags;
                            let next_pos = pra.next_pos;
                            let last_end = pra.last_end;
                            let fn_val = pra.fn_val;
                            // Find the next match at/after next_pos.
                            let next_match: Option<(Vec<(usize, usize)>, usize)> =
                                if let Some(pat) = &regex_pattern {
                                    match rune_regex::parse_regex(pat) {
                                        Ok(expr) => {
                                            let nfa = rune_regex::nfa::compile(&expr);
                                            let pike_vm = rune_regex::pikevm::PikeVm::new();
                                            pike_vm
                                                .exec_with_flags(
                                                    &nfa,
                                                    &input,
                                                    next_pos,
                                                    regex_flags,
                                                )
                                                .map(|m| {
                                                    let start = m.groups[0].0;
                                                    (m.groups, start)
                                                })
                                        }
                                        Err(_) => None,
                                    }
                                } else if next_pos <= input.len() {
                                    // String mode: StringIndexOf from next_pos.
                                    if let Some(rel) = input[next_pos..].find(&search_str) {
                                        let p = next_pos + rel;
                                        Some((vec![(p, p + search_str.len())], p))
                                    } else {
                                        None
                                    }
                                } else {
                                    // Non-object/array link (see above): end the scan.
                                    break;
                                };
                            if let Some((groups, start)) = next_match {
                                let end = groups[0].1;
                                let preserved = if start >= last_end {
                                    input[last_end..start].to_string()
                                } else {
                                    String::new()
                                };
                                pra.accumulated.push_str(&preserved);
                                let empty = start == end;
                                pra.next_pos = if empty {
                                    if regex_pattern.is_some() {
                                        start + 1
                                    } else {
                                        // String mode: skip one full char (byte
                                        // offsets must stay on UTF-8 boundaries).
                                        start
                                            + input[start..]
                                                .chars()
                                                .next()
                                                .map(|c| c.len_utf8())
                                                .unwrap_or(1)
                                    }
                                } else {
                                    end
                                };
                                pra.last_end = end;
                                // Build callback args: (match, ...captures, position, string)
                                // for regex mode; (searchString, position, string) for
                                // string mode.
                                let mut fn_args = Vec::new();
                                if regex_pattern.is_some() {
                                    let match_str = HeapString::allocate(gc, &input[start..end]);
                                    fn_args.push(Value::from_heap_ptr(match_str as *mut u8));
                                    for &(gs, ge) in &groups[1..] {
                                        let cap = HeapString::allocate(gc, &input[gs..ge]);
                                        fn_args.push(Value::from_heap_ptr(cap as *mut u8));
                                    }
                                    fn_args.push(Value::smi(start as i32));
                                    let input_str = HeapString::allocate(gc, &input);
                                    fn_args.push(Value::from_heap_ptr(input_str as *mut u8));
                                } else {
                                    let ss = HeapString::allocate(gc, &search_str);
                                    fn_args.push(Value::from_heap_ptr(ss as *mut u8));
                                    fn_args.push(Value::smi(start as i32));
                                    let input_str = HeapString::allocate(gc, &input);
                                    fn_args.push(Value::from_heap_ptr(input_str as *mut u8));
                                }
                                self.pending_replace_all_op = Some(PendingReplaceAllOp {
                                    source_frame_depth: pra.source_frame_depth,
                                    input,
                                    search_str,
                                    regex_pattern,
                                    regex_flags: pra.regex_flags,
                                    fn_val,
                                    next_pos: pra.next_pos,
                                    accumulated: pra.accumulated,
                                    last_end: pra.last_end,
                                });
                                self.push_callback_call(gc, fn_val, Value::undefined(), fn_args);
                                continue;
                            }
                            // No more matches — finish: append the tail.
                            let final_str =
                                pra.accumulated + &input[pra.last_end.min(input.len())..];
                            let ptr = HeapString::allocate(gc, &final_str);
                            self.stack.truncate(callee_base);
                            self.push(Value::from_heap_ptr(ptr as *mut u8));
                            let caller_idx = self.frames.len() - 1;
                            self.frames[caller_idx].pc += 1;
                            continue;
                        }
                    }
                    // Async return: resolve outer Promise when async generator completes.
                    if let Some(ptr) = async_promise_ptr {
                        unsafe {
                            Promise::set_state(ptr, PROMISE_FULFILLED);
                            Promise::set_result(ptr, result);
                        }
                        let reactions_ptr = unsafe { Promise::reactions(ptr) };
                        if !reactions_ptr.is_null() {
                            let arr = reactions_ptr as *mut RuneArray;
                            let len = unsafe { RuneArray::length(arr) };
                            let mut idx = 0;
                            while idx + 1 < len as usize {
                                let cb = unsafe { RuneArray::get_element(arr, idx) };
                                let chained = unsafe { RuneArray::get_element(arr, idx + 1) };
                                if cb.is_heap_object() {
                                    let ppc = PendingPromiseCtor {
                                        source_frame_depth: 0,
                                        promise: chained,
                                        resolve_handle: Value::undefined(),
                                        reject_handle: Value::undefined(),
                                        resolve_with_result: true,
                                    };
                                    self.enqueue_microtask(cb, vec![result], Some(ppc));
                                }
                                idx += 2;
                            }
                        }
                        self.stack.truncate(callee_base);
                        let promise_val = Value::from_heap_ptr(ptr);
                        self.push(promise_val);
                        if self.frames.is_empty() {
                            self.stack.clear();
                            return Exit::Return(promise_val);
                        }
                        let new_fi = self.frames.len() - 1;
                        self.frames[new_fi].pc += 1;
                        continue;
                    }
                    // Check if this return completes an async generator resume bridge.
                    if let Some(pag) = self.pending_async_gen.take() {
                        if self.frames.is_empty() {
                            let g = &self.generators[pag.gen_id];
                            self.frames.push(Frame {
                                locals: g.locals.clone(),
                                lexical_slots: g.lexical_slots.clone(),
                                lexical_tdz: g.lexical_tdz.clone(),
                                lexical_const: g.lexical_const.clone(),
                                scope_boundaries: g.scope_boundaries.clone(),
                                passed_argc: 0,
                                pc: g.pc,
                                stack_base: self.stack.len(),
                                prog: g.prog,
                                generator_id: Some(pag.gen_id),
                                this: g.this,
                                is_constructor_call: false,
                                constructed_object: Value::undefined(),
                                env: g.env,
                                func_ptr: std::ptr::null_mut(),
                                private_name_ids: std::ptr::null_mut(),
                            });
                            if g.started {
                                self.push(pag.arg);
                            }
                            continue;
                        }
                    }
                    if self.frames.is_empty() {
                        self.stack.clear();
                        return Exit::Return(result);
                    }
                    if self.frames.len() <= self.return_frame_floor {
                        // Nested module evaluation: the module frame returned.
                        // The stack above the caller's base is the module's
                        // temporaries — discard them.
                        let caller_base = self.frames.last().unwrap().stack_base;
                        self.stack.truncate(caller_base);
                        return Exit::Return(result);
                    }
                    let new_fi = self.frames.len() - 1;
                    self.stack.truncate(callee_base);
                    // §11.2.2 [[Construct]]: if constructor returns a heap object, use it;
                    // otherwise use the originally constructed object.
                    // A3: derived constructors (§10.2.3) may only return an
                    // object or undefined — any other value throws a
                    // TypeError. Derived = the callee Func has a superclass.
                    // (Only Object-type results count as objects here: heap
                    // strings/primitives are not Ordinary objects.)
                    if is_constructor {
                        // Object-typed per is_object_value (heap strings and
                        // other primitives don't count, even though boxed).
                        let is_object_result = crate::builtins::is_object_value(result);
                        if !is_object_result && !result.is_undefined() {
                            let derived = !popped_func.is_null()
                                && unsafe { !Func::superclass(popped_func as *mut Func).is_null() };
                            if derived {
                                let err = crate::errors::error_object(
                                    gc,
                                    &self.error_protos,
                                    crate::errors::ErrorKind::TypeError,
                                    "Derived constructors may only return object or undefined",
                                );
                                if let Some(exit) = self.handle_throw(gc, err) {
                                    return exit;
                                }
                                continue;
                            }
                        }
                        if result.is_heap_object() {
                            self.push(result);
                        } else {
                            self.push(constructed_obj);
                        }
                    } else {
                        self.push(result);
                    }
                    self.frames[new_fi].pc += 1;
                }
                Opcode::InitGenerator => {
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Yield => {
                    let val = self.pop();
                    if let Some(gen_id) = self.frames[fi].generator_id {
                        let g = &mut self.generators[gen_id];
                        g.locals = self.frames[fi].locals.clone();
                        g.lexical_slots = self.frames[fi].lexical_slots.clone();
                        g.lexical_tdz = self.frames[fi].lexical_tdz.clone();
                        g.lexical_const = self.frames[fi].lexical_const.clone();
                        g.scope_boundaries = self.frames[fi].scope_boundaries.clone();
                        g.pc = pc + 1;
                        g.prog = self.frames[fi].prog;
                        g.started = true;
                        g.this = self.frames[fi].this;
                        g.env = self.frames[fi].env;
                        g.in_delegate = false;
                        g.try_frames = std::mem::take(&mut self.try_stack);
                        let base = self.frames[fi].stack_base;
                        g.stack_base_saved = base;
                        g.stack = self.stack[base..].to_vec();
                    }
                    let callee_base = self.frames.last().unwrap().stack_base;
                    let popped_frame = self.frames.len() - 1;
                    self.last_locals = self.frames[popped_frame].locals.clone();
                    self.frames.pop();
                    self.try_stack
                        .retain(|tf| tf.frame_depth != popped_frame + 1);
                    if self.frames.is_empty() {
                        self.stack.clear();
                        return Exit::Yield(val);
                    }
                    // Nested-resume case (generator_next): the frames below
                    // belong to run_loops OUTSIDE this nested call — pushing
                    // the yielded value onto them leaked one slot PER YIELD
                    // into the caller's operand stack and desynchronized any
                    // enclosing for-of. The value travels via Exit::Yield.
                    self.stack.truncate(callee_base);
                    return Exit::Yield(val);
                }
                Opcode::YieldStarYield => {
                    // Identical suspension to Yield, but marks the generator
                    // as suspended inside a yield* delegation (live [iter,
                    // next] pair is on top of the banked operand stack).
                    let val = self.pop();
                    if let Some(gen_id) = self.frames[fi].generator_id {
                        let g = &mut self.generators[gen_id];
                        g.locals = self.frames[fi].locals.clone();
                        g.lexical_slots = self.frames[fi].lexical_slots.clone();
                        g.lexical_tdz = self.frames[fi].lexical_tdz.clone();
                        g.lexical_const = self.frames[fi].lexical_const.clone();
                        g.scope_boundaries = self.frames[fi].scope_boundaries.clone();
                        g.pc = pc + 1;
                        g.prog = self.frames[fi].prog;
                        g.started = true;
                        g.this = self.frames[fi].this;
                        g.env = self.frames[fi].env;
                        g.in_delegate = true;
                        g.try_frames = std::mem::take(&mut self.try_stack);
                        let base = self.frames[fi].stack_base;
                        g.stack_base_saved = base;
                        g.stack = self.stack[base..].to_vec();
                    }
                    let callee_base = self.frames.last().unwrap().stack_base;
                    let popped_frame = self.frames.len() - 1;
                    self.last_locals = self.frames[popped_frame].locals.clone();
                    self.frames.pop();
                    self.try_stack
                        .retain(|tf| tf.frame_depth != popped_frame + 1);
                    if self.frames.is_empty() {
                        self.stack.clear();
                        return Exit::Yield(val);
                    }
                    self.stack.truncate(callee_base);
                    return Exit::Yield(val);
                }
                Opcode::Await => {
                    let val = self.pop();
                    if let Some(gen_id) = self.frames[fi].generator_id {
                        // Save generator state
                        let g = &mut self.generators[gen_id];
                        g.locals = self.frames[fi].locals.clone();
                        g.lexical_slots = self.frames[fi].lexical_slots.clone();
                        g.lexical_tdz = self.frames[fi].lexical_tdz.clone();
                        g.lexical_const = self.frames[fi].lexical_const.clone();
                        g.scope_boundaries = self.frames[fi].scope_boundaries.clone();
                        g.pc = pc + 1;
                        g.prog = self.frames[fi].prog;
                        g.started = true;
                        g.this = self.frames[fi].this;
                        g.env = self.frames[fi].env;
                        // Create bridge functions for resume/reject
                        let continue_handle = self.find_builtin_handle("async_continue");
                        let reject_handle = self.find_builtin_handle("async_reject");
                        let continue_bridge = self.create_async_bridge(gc, gen_id, continue_handle);
                        let reject_bridge = self.create_async_bridge(gc, gen_id, reject_handle);
                        // Call Promise.resolve(val)
                        let resolved = crate::builtins::promise_static_resolve(
                            gc,
                            Value::undefined(),
                            &[val],
                            self,
                        );
                        // Call .then(continue_bridge, reject_bridge) on the resolved promise
                        crate::builtins::promise_prototype_then(
                            gc,
                            resolved,
                            &[continue_bridge, reject_bridge],
                            self,
                        );
                        // Push the outer Promise as the "return value" for the caller
                        let promise_ptr = self
                            .async_tasks
                            .iter()
                            .find(|t| t.gen_id == gen_id)
                            .map(|t| t.promise)
                            .unwrap();
                        let callee_base = self.frames.last().unwrap().stack_base;
                        let popped_frame = self.frames.len() - 1;
                        self.last_locals = self.frames[popped_frame].locals.clone();
                        self.frames.pop();
                        self.try_stack
                            .retain(|tf| tf.frame_depth != popped_frame + 1);
                        self.stack.truncate(callee_base);
                        self.push(Value::from_heap_ptr(promise_ptr));
                        if self.frames.is_empty() {
                            break;
                        }
                        // Advance the caller's PC past the Call instruction
                        let caller_fi = self.frames.len() - 1;
                        self.frames[caller_fi].pc += 1;
                        continue;
                    }
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::YieldStar => {
                    // Stub: return undefined (delegate yield not yet implemented)
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Resume => {
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::CallFromArray => {
                    let args_arr = self.pop();
                    let callee = self.pop();
                    let this = self.pop();
                    let argc = if let Some(ptr) = args_arr.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_ARRAY {
                            unsafe { RuneArray::length(ptr as *mut RuneArray) as usize }
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    let mut args: Vec<Value> = Vec::with_capacity(argc);
                    if let Some(ptr) = args_arr.heap_ptr() {
                        let arr_ptr = ptr as *mut RuneArray;
                        for i in 0..argc {
                            let v = unsafe { RuneArray::get_element(arr_ptr, i) };
                            args.push(v);
                        }
                    }

                    // Builtin dispatch: negative Smi handles
                    if let Some(smi_val) = callee.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                let result = (self.builtins[id].func)(gc, this, &args, &mut *self);
                                if let Some(exc) = self.pending_exception.take() {
                                    if let Some(exit) = self.handle_throw(gc, exc) {
                                        return exit;
                                    }
                                    continue;
                                }
                                if self.pending_array_op.is_some() || self.pending_call.is_some() {
                                    continue;
                                }
                                self.push(result);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        } else {
                            self.push(Value::undefined());
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }

                    if let Some(ptr) = callee.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_FUNC {
                            // A3: same class-call check as Opcode::Call;
                            // operand 0 = 1 marks super(...args) spread form.
                            if unsafe { Func::is_class_constructor(ptr as *mut Func) } {
                                let is_super = instr.operands.first().copied().unwrap_or(0) != 0;
                                if !is_super {
                                    let err = crate::errors::error_object(
                                        gc,
                                        &self.error_protos,
                                        crate::errors::ErrorKind::TypeError,
                                        "Class constructor cannot be invoked without 'new'",
                                    );
                                    if let Some(exit) = self.handle_throw(gc, err) {
                                        return exit;
                                    }
                                    continue;
                                }
                            }
                            let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                            let creator_prog = unsafe {
                                &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram)
                            };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];

                                // CallFromArray for async
                                if func_prog.is_async {
                                    let passed_argc = args.len();
                                    let mut g =
                                        Generator::new(args, func_prog as *const BytecodeProgram);
                                    g.this = this;
                                    g.env = unsafe { Func::env_ptr(ptr as *mut Func) };
                                    g.started = true;
                                    let gen_id = self.generators.len();
                                    self.generators.push(g);
                                    let proto = self.promise_prototype.heap_ptr();
                                    let promise_ptr = Promise::allocate(gc, proto);
                                    self.async_tasks.push(AsyncTask {
                                        gen_id,
                                        promise: promise_ptr,
                                    });
                                    let func_ptr = ptr as *mut Func;
                                    let func_env = unsafe { Func::env_ptr(func_ptr) };
                                    // Restore args for the frame: Generator::new moved them,
                                    // so repack from the generator we just stored.
                                    let g_args = self.generators[gen_id].locals.clone();
                                    let mut locals = if func_prog.named_function {
                                        vec![callee]
                                    } else {
                                        vec![]
                                    };
                                    locals.extend(g_args);
                                    self.frames.push(Frame {
                                        locals,
                                        lexical_slots: Vec::new(),
                                        lexical_tdz: Vec::new(),
                                        lexical_const: Vec::new(),
                                        scope_boundaries: Vec::new(),
                                        passed_argc,
                                        pc: 0,
                                        stack_base: self.stack.len(),
                                        prog: func_prog as *const BytecodeProgram,
                                        generator_id: Some(gen_id),
                                        this,
                                        is_constructor_call: false,
                                        constructed_object: Value::undefined(),
                                        env: func_env,
                                        func_ptr: func_ptr as *mut u8,
                                        private_name_ids: std::ptr::null_mut(),
                                    });
                                    continue;
                                }
                                if func_prog.is_generator {
                                    // Mirror the interpreter's locals layout:
                                    // named functions reserve slot 0 for the
                                    // callee (params shift by one).
                                    let mut gen_locals = if func_prog.named_function {
                                        vec![callee]
                                    } else {
                                        vec![]
                                    };
                                    gen_locals.extend(args);
                                    let mut g = Generator::new(
                                        gen_locals,
                                        func_prog as *const BytecodeProgram,
                                    );
                                    g.this = this;
                                    g.env = unsafe { Func::env_ptr(ptr as *mut Func) };
                                    let gen_id = self.generators.len();
                                    self.generators.push(g);
                                    let instance =
                                        crate::builtins::make_generator_instance(gc, self, gen_id);
                                    self.push(instance);
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                                let func_ptr = ptr as *mut Func;
                                let func_env = unsafe { Func::env_ptr(func_ptr) };
                                let mut locals: Vec<Value> = if func_prog.named_function {
                                    vec![callee]
                                } else {
                                    vec![]
                                };
                                let passed_argc = args.len();
                                locals.extend(args);
                                self.frames.push(Frame {
                                    locals,
                                    lexical_slots: Vec::new(),
                                    lexical_tdz: Vec::new(),
                                    lexical_const: Vec::new(),
                                    scope_boundaries: Vec::new(),
                                    passed_argc,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this,
                                    is_constructor_call: false,
                                    constructed_object: Value::undefined(),
                                    env: func_env,
                                    func_ptr: func_ptr as *mut u8,
                                    private_name_ids: std::ptr::null_mut(),
                                });
                                continue;
                            }
                        }
                    }
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
            }
        }

        let result = self.stack.pop().unwrap_or(Value::undefined());
        let saved_locals = self
            .frames
            .first()
            .map(|f| f.locals.clone())
            .unwrap_or_default();
        self.frames.clear();
        self.stack.clear();
        // Save locals for sync by execute()
        self.last_locals = saved_locals;
        Exit::Return(result)
    }

    pub fn push(&mut self, val: Value) {
        self.stack.push(val);
    }

    /// Push a value and advance the top frame's pc (shared by pending-state
    /// continuations in builtins.rs).
    pub(crate) fn push_and_advance(&mut self, val: Value) {
        self.push(val);
        let n = self.frames.len();
        self.frames[n - 1].pc += 1;
    }

    /// Pop two values (e.g. [iterator, nextMethod]), push a value, and jump
    /// the top frame to `target`.
    pub(crate) fn pop2_push_jump(&mut self, val: Value, target: usize) {
        self.pop();
        self.pop();
        self.push(val);
        let n = self.frames.len();
        self.frames[n - 1].pc = target;
    }

    pub fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or(Value::undefined())
    }

    pub fn peek(&self) -> Value {
        self.stack.last().copied().unwrap_or(Value::undefined())
    }

    /// Get the private name ID for a given slot index from the executing frame.
    /// First checks the frame's direct private_name_ids, then falls back to
    /// the Func's private_name_ids (propagated via MakeFunction).
    fn get_private_name_id(&self, fi: usize, slot_idx: u32) -> Option<u64> {
        let check_array = |ids: *mut u8| -> Option<u64> {
            if !ids.is_null() {
                unsafe {
                    let len = RuneArray::length(ids as *mut RuneArray) as usize;
                    if (slot_idx as usize) < len {
                        let val = RuneArray::get_element(ids as *mut RuneArray, slot_idx as usize);
                        return val.as_smi().map(|v| v as u64);
                    }
                }
            }
            None
        };
        // Check the frame's private_name_ids (set by PrivateNameScope)
        if let Some(id) = check_array(self.frames[fi].private_name_ids) {
            return Some(id);
        }
        // Fall back to Func's private_name_ids (propagated by MakeFunction)
        let func_ptr = self.frames[fi].func_ptr;
        if !func_ptr.is_null() {
            let ids = unsafe { Func::private_name_ids(func_ptr as *mut Func) };
            if let Some(id) = check_array(ids) {
                return Some(id);
            }
        }
        None
    }

    /// Update all root references from `old_ptr` to `new_ptr` after a heap object
    /// has been relocated (e.g., array grow reallocation).
    /// Scans stack, all frame locals, and globals for matching heap pointers.
    pub fn update_heap_reference(&mut self, old_ptr: *mut u8, new_ptr: *mut u8) {
        for v in &mut self.stack {
            if let Some(p) = v.heap_ptr() {
                if p == old_ptr {
                    *v = Value::from_heap_ptr(new_ptr);
                }
            }
        }
        for frame in &mut self.frames {
            for v in &mut frame.locals {
                if let Some(p) = v.heap_ptr() {
                    if p == old_ptr {
                        *v = Value::from_heap_ptr(new_ptr);
                    }
                }
            }
            // Update frame's this and constructed_object fields too
            if let Some(p) = frame.this.heap_ptr() {
                if p == old_ptr {
                    frame.this = Value::from_heap_ptr(new_ptr);
                }
            }
            if let Some(p) = frame.constructed_object.heap_ptr() {
                if p == old_ptr {
                    frame.constructed_object = Value::from_heap_ptr(new_ptr);
                }
            }
            // Also update env object slots (the GC-managed EnvObject)
            if !frame.env.is_null() {
                let env_ptr = frame.env;
                unsafe {
                    let slot_count = *(env_ptr.add(8) as *const u32) as usize;
                    let slots = env_ptr.add(24) as *mut Value;
                    for i in 0..slot_count {
                        let slot = &mut *slots.add(i);
                        if let Some(p) = slot.heap_ptr() {
                            if p == old_ptr {
                                *slot = Value::from_heap_ptr(new_ptr);
                            }
                        }
                    }
                }
            }
        }
        for v in self.globals.values_mut() {
            if let Some(p) = v.heap_ptr() {
                if p == old_ptr {
                    *v = Value::from_heap_ptr(new_ptr);
                }
            }
        }
        // Also update jit_locals_buffer and last_locals (may hold stale array
        // pointers between JIT calls).
        for v in &mut self.jit_locals_buffer {
            if let Some(p) = v.heap_ptr() {
                if p == old_ptr {
                    *v = Value::from_heap_ptr(new_ptr);
                }
            }
        }
        for v in &mut self.last_locals {
            if let Some(p) = v.heap_ptr() {
                if p == old_ptr {
                    *v = Value::from_heap_ptr(new_ptr);
                }
            }
        }
    }
}

impl RootProvider for Vm {
    fn register_roots(&mut self, gc: &mut SemiSpace) {
        gc.clear_roots();
        self.register_roots(gc);
    }
}

impl Vm {
    /// Return a summary of IC hit/miss statistics.
    /// Note: `hits` counts IC LOOKUP hits; `misses` counts both IC lookup misses
    /// AND LoadPropertyIC shape-guard misses. For polymorphic sites the guard
    /// miss dominates, so `hits / lookups` is the accurate IC lookup hit rate.
    /// `gap = lookups - (hits + misses)` measures counter consistency:
    ///   - gap = 0: all accesses are cleanly counted
    ///   - gap > 0: some accesses counted as lookups but not as hits or misses
    ///     (likely: cache hits on the run before opcode patching)
    pub fn dump_ic_stats(&self) -> String {
        let ic_hit_rate = if self.ic_stats.lookups > 0 {
            (self.ic_stats.hits as f64 / self.ic_stats.lookups as f64) * 100.0
        } else {
            0.0
        };
        let gap = self.ic_stats.lookups as i64 - (self.ic_stats.hits + self.ic_stats.misses) as i64;
        debug_assert!(
            self.ic_stats.lookups as i64 >= self.ic_stats.hits as i64 + self.ic_stats.misses as i64,
            "IC stats violate: lookups({}) < hits({}) + misses({})",
            self.ic_stats.lookups,
            self.ic_stats.hits,
            self.ic_stats.misses
        );
        format!(
            "IC stats: {} lookups, {} hits, {} misses (IC hit rate: {:.1}%, gap: {})",
            self.ic_stats.lookups, self.ic_stats.hits, self.ic_stats.misses, ic_hit_rate, gap
        )
    }

    /// Return JIT entry/bailout counters (for --jit-stats).
    pub fn dump_jit_stats(&self) -> String {
        format!(
            "JIT stats: {} entries, {} bailouts ({} bailed)",
            self.jit_entry_count,
            self.jit_bailout_count,
            if self.jit_entry_count > 0 {
                (self.jit_bailout_count as f64 / self.jit_entry_count as f64) * 100.0
            } else {
                0.0
            }
        )
    }

    /// Return a summary of loop hotness and recorded traces (for --trace-stats).
    pub fn dump_trace_stats(&self) -> String {
        if self.loop_counts.is_empty() {
            return "Trace stats: no loops detected.".to_string();
        }
        let mut lines = vec![format!(
            "Trace stats: {} loop(s) detected",
            self.loop_counts.len()
        )];
        for ((prog, target), count) in self.loop_counts.iter() {
            let label = if *count >= 50 { "HOT" } else { "warm" };
            lines.push(format!(
                "  prog={:#x} pc={} → {} iterations ({})",
                prog, target, count, label
            ));
            if let Some(trace) = self.loop_traces.get(&(*prog, *target)) {
                let mono = if trace.is_monomorphic() {
                    "MONO (1 shape)"
                } else {
                    "POLY"
                };
                let icost = trace.estimated_interpreter_cost();
                let ncost = trace.estimated_native_cost();
                let speedup = if ncost > 0 {
                    (icost as f64 / ncost as f64) as u32
                } else {
                    0
                };
                lines.push(format!(
                    "    trace: {} ops, {} shapes ({})",
                    trace.ops.len(),
                    trace.shape_ids.len(),
                    mono
                ));
                lines.push(format!(
                    "    estimated speedup: {}→{} instrs ≈ {}×",
                    icost,
                    ncost,
                    speedup.max(1)
                ));
                #[cfg(feature = "jit")]
                if !trace.inline_profiles.is_empty() {
                    for p in &trace.inline_profiles {
                        lines.push(format!(
                            "    inline profile: call_pc={}, func_idx={}, jit={}, frame={}, size={}",
                            p.call_pc,
                            p.callee_func_idx,
                            if p.callee_jit_entry.is_some() { "yes" } else { "no" },
                            if p.callee_needs_frame { "yes" } else { "no" },
                            p.callee_bytecode_size,
                        ));
                    }
                }
            }
        }
        lines.join("\n")
    }

    /// Patch LoadProperty instructions in a hot monomorphic loop to
    /// LoadPropertyIC with cached IC values, eliminating IC lookup overhead.
    /// Compile a recorded loop trace to native AArch64 code.
    /// The trace is compiled as a self-contained loop: the back-edge Jump is
    /// remapped to loop back to the first instruction, and JumpIfFalse is
    /// remapped to exit the trace.  The interpreter never enters the compiled
    /// code — it runs until the loop condition is false, then returns.
    #[cfg(all(feature = "jit", target_arch = "aarch64"))]
    fn compile_trace_native(&mut self, prog_ptr: *const BytecodeProgram, target_pc: usize) {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};

        let key: TraceKey = (prog_ptr as usize, target_pc);
        let trace = match self.loop_traces.get_mut(&key) {
            Some(t) => t,
            None => return,
        };
        // The original program whose string/float pools the recorded
        // name/float indices reference.  Must be the one currently
        // executing (top frame's prog).
        let original_prog = unsafe { &*prog_ptr };

        let mut instrs: Vec<Instruction> = Vec::with_capacity(trace.ops.len() + 2);
        // Build mapping: trace instruction index → original program PC.
        // Used to translate bailout PCs when the trace bails mid-loop.
        let mut trace_to_original_pc: Vec<usize> = Vec::with_capacity(trace.ops.len());
        let mut exit_pc: usize = 0;
        // The last recorded op is the first instruction of the iteration
        // that triggered the recording stop — it's a duplicate of op 0.
        let ops_slice = if trace.ops.len() > 1
            && trace.ops.first().map(|t| t.opcode) == trace.ops.last().map(|t| t.opcode)
        {
            &trace.ops[..trace.ops.len() - 1]
        } else {
            &trace.ops[..]
        };
        // Snapshot interpreter ICs for trace-compiled property accesses.
        let mut trace_ic_tables: Vec<rune_jit_baseline::ic::TraceIcTable> =
            Vec::with_capacity(ops_slice.len());
        for t in ops_slice {
            let opcode: Opcode = unsafe { std::mem::transmute(t.opcode) };
            let mut operands = t.operands.clone();
            trace_to_original_pc.push(t.original_pc);
            // Snapshot InlineCache for property IC instructions
            let needs_ic = matches!(opcode, Opcode::LoadPropertyIC | Opcode::StorePropertyIC);
            if needs_ic && t.ic_index >= 0 {
                let ic_idx = t.ic_index as usize;
                if ic_idx < self.ics.len() {
                    let ic = &self.ics[ic_idx];
                    let mut entries: [rune_jit_baseline::ic::TraceIcEntry; 16] =
                        [rune_jit_baseline::ic::TraceIcEntry {
                            shape_id: 0,
                            slot_offset: 0,
                        }; 16];
                    let mut count = 0;
                    for (k, v) in &ic.entries {
                        if count >= 16 {
                            break;
                        }
                        entries[count] = rune_jit_baseline::ic::TraceIcEntry {
                            shape_id: k.shape_id,
                            slot_offset: 32 + (v.offset as u64) * 8,
                        };
                        count += 1;
                    }
                    trace_ic_tables.push(rune_jit_baseline::ic::TraceIcTable { entries, count });
                } else {
                    trace_ic_tables.push(rune_jit_baseline::ic::TraceIcTable::default());
                }
            } else {
                trace_ic_tables.push(rune_jit_baseline::ic::TraceIcTable::default());
            }
            // Remap branch targets from original bytecode indices to in-trace
            // indices.
            match opcode {
                Opcode::Jump | Opcode::JumpIfTrue | Opcode::JumpIfFalse => {
                    let orig_target = operands.first().copied().unwrap_or(0) as usize;
                    if opcode == Opcode::JumpIfFalse && exit_pc == 0 {
                        exit_pc = orig_target;
                    }
                    if orig_target == target_pc {
                        // Back-edge → branch to trace start (loop)
                        operands[0] = 0;
                    } else if orig_target > target_pc {
                        // Forward branch → target is past the end of our trace
                        // (exit path). Point to a trailing Return instruction.
                        operands[0] = -1; // will be replaced with actual return index
                    } else {
                        // Other backward branch (unlikely in a simple loop).
                        // Keep as-is; will be within the trace body.
                    }
                    // Store the position that needs exit-target patching
                }
                _ => {}
            }
            instrs.push(Instruction::new(opcode, operands));
        }

        if instrs.is_empty() {
            return;
        }

        // Build InlinePlan from collected inline profiles (F-2 Layer 2a).
        // Must happen before `instrs` is moved into `prog` below.
        // Gated by --inline feature flag: plan is empty under --no-inline.
        let mut inline_plan = rune_jit_baseline::InlinePlan::default();
        if self.enable_inlining && !trace.inline_profiles.is_empty() {
            for profile in &trace.inline_profiles {
                let found_idx = ops_slice
                    .iter()
                    .position(|op| op.original_pc == profile.call_pc)
                    .filter(|&idx| idx < instrs.len());
                if let Some(instr_idx) = found_idx {
                    let instr = &instrs[instr_idx];
                    if instr.opcode == Opcode::Call {
                        let argc = instr.operands[0] as u32;
                        let callee_prog =
                            unsafe { &*(profile.callee_prog_ptr as *const BytecodeProgram) };
                        let callee_named_function = if profile.callee_func_idx >= 0
                            && (profile.callee_func_idx as usize) < callee_prog.functions.len()
                        {
                            callee_prog.functions[profile.callee_func_idx as usize].named_function
                        } else {
                            false
                        };
                        // F-2 Layer 2b eligibility: only inline callees whose opcodes
                        // are all supported by emit_inline_call.
                        // P25: This whitelist duplicates emit_inline_call's match arms at
                        // codegen_aarch64.rs:535-597. Must stay in sync manually. Fix:
                        // move to a single pub fn is_inlineable_opcode() in the codegen crate.
                        let eligible = if profile.callee_func_idx >= 0
                            && (profile.callee_func_idx as usize) < callee_prog.functions.len()
                        {
                            let func = &callee_prog.functions[profile.callee_func_idx as usize];
                            // Must match the opcodes handled in emit_inline_call.
                            func.instructions.iter().all(|i| {
                                matches!(
                                    i.opcode,
                                    Opcode::Return
                                        | Opcode::LoadLocal
                                        | Opcode::StoreLocal
                                        | Opcode::Add
                                        | Opcode::Sub
                                        | Opcode::LoadSmi
                                        | Opcode::LoadUndefined
                                        | Opcode::LoadNull
                                        | Opcode::LoadBoolean
                                        | Opcode::Pop
                                        | Opcode::Dup
                                        | Opcode::Swap
                                )
                            })
                        } else {
                            false
                        };
                        if eligible {
                            inline_plan.entries.push(rune_jit_baseline::InlineEntry {
                                call_instr_idx: instr_idx,
                                callee_func_idx: profile.callee_func_idx,
                                callee_prog_ptr: profile.callee_prog_ptr,
                                callee_named_function,
                                argc,
                            });
                        }
                    }
                }
            }
        }

        // Patch forward-branch targets to point past the last instruction.
        // Also add a Return at the end so the trace exits cleanly.
        let return_index = instrs.len();
        for instr in &mut instrs {
            if matches!(
                instr.opcode,
                Opcode::Jump | Opcode::JumpIfTrue | Opcode::JumpIfFalse
            ) && instr.operands.first().copied() == Some(-1)
            {
                instr.operands[0] = return_index as i64;
            }
        }
        instrs.push(Instruction::new(Opcode::LoadUndefined, vec![]));
        instrs.push(Instruction::new(Opcode::Return, vec![]));

        // Copy the original program's string/float pools so that name/float
        // indices recorded in the trace resolve correctly at JIT time.
        let prog = BytecodeProgram::new(instrs, original_prog.string_pool.clone(), vec![]);
        if !rune_jit_baseline::is_jit_compatible(&prog) {
            return; // trace contains unsupported opcodes (strings, objects, etc.)
        }

        let codegen = Aarch64CodeGen::new(prog.instructions.len())
            .with_trace_ic_tables(trace_ic_tables)
            .with_inline_plan(inline_plan)
            .with_stencil_jit(self.stencil_jit);
        // Leak the program so its address stays valid for the compiled trace's
        // embedded prog_ptr reference (used by LoadStringConst, globals, etc.).
        let leaked_prog = Box::leak(Box::new(prog));
        let compiled = codegen.compile(leaked_prog);

        compiled.mem.make_executable();
        let entry = compiled.mem.code_ptr();
        trace.compiled_entry = entry;
        trace.exit_pc = exit_pc;
        trace.compiled_prog = leaked_prog as *mut BytecodeProgram as *mut u8;
        trace.trace_to_original_pc = trace_to_original_pc;
        trace.bailout_table = Some(Box::new(compiled.bailout_table));
        self._compiled_trace_mem.push(compiled.mem);
    }

    /// Call a compiled loop trace. Returns the raw u64 result (unused for
    /// loop traces — the locals are updated in-place by the trace).
    #[cfg(feature = "jit")]
    unsafe fn execute_trace(&mut self, fi: usize, entry: *const u8, gc_ptr: *mut u8) -> u64 {
        self.jit_entry_count += 1;
        let func: rune_jit_baseline::JitEntryFn = unsafe { std::mem::transmute(entry) };
        let locals = self.frames[fi].locals.as_mut_ptr() as *mut u64;
        unsafe { func(self as *mut Vm as *mut u8, gc_ptr, locals) }
    }

    unsafe fn patch_loop_body(
        &mut self,
        prog_ptr: *const BytecodeProgram,
        target_pc: usize,
        back_edge_pc: usize,
    ) {
        let key: TraceKey = (prog_ptr as usize, target_pc);
        if self.loop_patched.contains(&key) {
            return;
        }
        let trace = match self.loop_traces.get(&key) {
            Some(t) if t.is_monomorphic() => t,
            _ => return,
        };
        let shape_id = trace.shape_ids.first().copied().unwrap_or(0);
        if shape_id == 0 {
            return;
        }

        let mut _patched = 0u32;
        for pc in target_pc..=back_edge_pc {
            let instr_ptr = unsafe {
                let instrs = (*prog_ptr).instructions.as_ptr() as *mut Instruction;
                &mut *instrs.add(pc)
            };
            if instr_ptr.opcode == Opcode::LoadProperty && instr_ptr.ic_index >= 0 {
                let ic_idx = instr_ptr.ic_index as usize;
                if ic_idx < self.ics.len() {
                    // Find the IC entry matching the trace's monomorphic shape_id
                    for (key, entry) in &self.ics[ic_idx].entries {
                        if key.shape_id == shape_id {
                            instr_ptr.opcode = Opcode::LoadPropertyIC;
                            instr_ptr.operands.clear();
                            instr_ptr.operands.extend_from_slice(&[
                                shape_id as i64,
                                entry.offset as i64,
                                entry.proto_depth as i64,
                            ]);
                            _patched += 1;
                            break;
                        }
                    }
                }
            }
        }

        self.loop_patched.insert(key);
    }
}

/// Allocate a GC-managed string and return it as a raw pointer (for builtins).
pub fn heap_string(gc: &mut SemiSpace, s: &str) -> *mut u8 {
    HeapString::allocate(gc, s) as *mut u8
}

impl Frame {
    fn prog_str(&self, idx: usize) -> Option<String> {
        let prog = unsafe { &*self.prog };
        prog.string_pool.get(idx).cloned()
    }
}

/// Per §7.2.14 IsStrictlyEqual.
/// §7.2.13 Abstract Equality Comparison.
/// Returns true if `a == b` per the spec.
fn values_loosely_equal(a: Value, b: Value) -> bool {
    // Same type or same-heap-tag → strict equality
    if a.is_boolean() && b.is_boolean() {
        return a == b;
    }
    if (a.is_smi() || a.is_float64()) && (b.is_smi() || b.is_float64()) {
        return values_strictly_equal(a, b);
    }
    // null == undefined → true (and vice versa)
    if (a.is_null() && b.is_undefined()) || (a.is_undefined() && b.is_null()) {
        return true;
    }
    // §7.2.13 step 6-7: Boolean → ToNumber(b), then compare
    if a.is_boolean() {
        return values_loosely_equal(
            if a.to_boolean() == Some(true) {
                Value::smi(1)
            } else {
                Value::smi(0)
            },
            b,
        );
    }
    if b.is_boolean() {
        return values_loosely_equal(
            a,
            if b.to_boolean() == Some(true) {
                Value::smi(1)
            } else {
                Value::smi(0)
            },
        );
    }
    // §7.2.13 step 8-9: Number vs String → compare ToNumber(string) with number
    let (num_val, str_val) = if (a.is_smi() || a.is_float64()) && values_is_string(b) {
        (a, b)
    } else if (b.is_smi() || b.is_float64()) && values_is_string(a) {
        (b, a)
    } else {
        // §7.2.13 step 10: Object vs String/Number/Symbol → ToPrimitive (deferred)
        // Fall back to strict equality for now.
        return values_strictly_equal(a, b);
    };
    let na = value_to_f64(num_val);
    let nb = to_number(str_val);
    if na.is_nan() || nb.is_nan() {
        return false;
    }
    if na == 0.0 && nb == 0.0 {
        // +0 === -0 per loose equality too
        return true;
    }
    na == nb
}

fn values_is_string(v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        tag == TAG_STRING
    } else {
        false
    }
}

/// Extract f64 from a value known to be numeric (Smi or Float64).
fn value_to_f64(v: Value) -> f64 {
    if let Some(n) = v.as_smi() {
        n as f64
    } else {
        v.as_float64().unwrap_or(f64::NAN)
    }
}

pub(crate) fn values_strictly_equal(a: Value, b: Value) -> bool {
    // Both are Number type (Smi or Float64)
    if a.is_smi() || b.is_smi() || a.is_float64() || b.is_float64() {
        let na = if a.is_smi() {
            a.as_smi().map(|s| s as f64)
        } else {
            a.as_float64()
        };
        let nb = if b.is_smi() {
            b.as_smi().map(|s| s as f64)
        } else {
            b.as_float64()
        };
        if let (Some(av), Some(bv)) = (na, nb) {
            if av.is_nan() || bv.is_nan() {
                return false;
            }
            return av == bv;
        }
        return false;
    }
    // Same raw value (same Smi or same heap pointer)
    if a == b {
        return true;
    }
    // String content comparison
    if let (Some(pa), Some(pb)) = (a.heap_ptr(), b.heap_ptr()) {
        let ta = unsafe { (*(pa as *const GcHeader)).tag() };
        let tb = unsafe { (*(pb as *const GcHeader)).tag() };
        if ta == TAG_STRING && tb == TAG_STRING {
            let la = unsafe { HeapString::len(pa as *mut HeapString) };
            let lb = unsafe { HeapString::len(pb as *mut HeapString) };
            if la != lb {
                return false;
            }
            let da = unsafe { HeapString::data(pa as *mut HeapString) };
            let db = unsafe { HeapString::data(pb as *mut HeapString) };
            for i in 0..la {
                if unsafe { *da.add(i) != *db.add(i) } {
                    return false;
                }
            }
            return true;
        }
    }
    false
}

/// Compare two values as strings for IsLessThan semantics.
/// Returns None if either value is not a string.
fn compare_strings_lt(a: Value, b: Value) -> Option<bool> {
    if let (Some(pa), Some(pb)) = (a.heap_ptr(), b.heap_ptr()) {
        let ta = unsafe { (*(pa as *const GcHeader)).tag() };
        let tb = unsafe { (*(pb as *const GcHeader)).tag() };
        if ta == TAG_STRING && tb == TAG_STRING {
            let la = unsafe { HeapString::len(pa as *mut HeapString) };
            let lb = unsafe { HeapString::len(pb as *mut HeapString) };
            let da = unsafe { HeapString::data(pa as *mut HeapString) };
            let db = unsafe { HeapString::data(pb as *mut HeapString) };
            let min_len = la.min(lb);
            for i in 0..min_len {
                let ca = unsafe { *da.add(i) };
                let cb = unsafe { *db.add(i) };
                if ca < cb {
                    return Some(true);
                }
                if ca > cb {
                    return Some(false);
                }
            }
            return Some(la < lb);
        }
    }
    None
}

pub(crate) fn value_to_debug_string(val: Value) -> String {
    if val.is_undefined() {
        "undefined".to_string()
    } else if val.is_null() {
        "null".to_string()
    } else if val.is_symbol() {
        val.as_symbol_id()
            .map(symbol_display)
            .unwrap_or_else(|| "Symbol".to_string())
    } else if let Some(b) = val.to_boolean() {
        b.to_string()
    } else if let Some(v) = val.as_smi() {
        v.to_string()
    } else if let Some(v) = val.as_float64() {
        v.to_string()
    } else if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            unsafe { HeapString::to_string(ptr as *mut HeapString) }
        } else if tag == TAG_STRING_OBJ {
            let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            unsafe { HeapString::to_string(str_ptr as *mut HeapString) }
        } else {
            "[object Object]".to_string()
        }
    } else {
        "undefined".to_string()
    }
}

/// For TAG_OBJECT values, try to convert to a primitive via OrdinaryToPrimitive.
/// Returns Ok(converted_value) on success (sync), or Err(()) if a callback is pending.
fn try_convert_object_to_string(val: Value, gc: &mut SemiSpace, vm: &mut Vm) -> Result<Value, ()> {
    let ptr = match val.heap_ptr() {
        Some(p) => p,
        None => return Ok(val),
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_DATE {
        // §7.1.1.1 ToPrimitive(Date, string): Date's default hint is string.
        let s = date::to_date_string(unsafe { date::RuneDate::tv(ptr) });
        let heap_ptr = HeapString::allocate(gc, &s);
        return Ok(Value::from_heap_ptr(heap_ptr as *mut u8));
    }
    if tag != TAG_OBJECT {
        return Ok(val);
    }
    match to_primitive_string(gc, val, vm) {
        Some(s) => {
            let heap_ptr = HeapString::allocate(gc, &s);
            Ok(Value::from_heap_ptr(heap_ptr as *mut u8))
        }
        None => {
            // to_primitive_string set pending_call + pushed callback frame.
            // Clear pending_call (we use pending_primitive_conversion instead).
            vm.pending_call = None;
            Err(())
        }
    }
}

/// Convert a value to a string for the String constructor (new String(x)).
/// Uses the sync version (user-defined toString/valueOf fall through to [object Object]).
fn arg_to_js_string_for_ctor(val: Value, gc: &mut SemiSpace, vm: &mut Vm) -> String {
    to_primitive_string_sync(val, gc, vm)
}

pub(crate) fn value_to_prop_key(val: Value) -> Option<PropertyKey> {
    // Symbols are valid property keys (stored under their registry id).
    if let Some(id) = val.as_symbol_id() {
        return Some(PropertyKey::from_symbol(id));
    }
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            return Some(PropertyKey::from_string(&s));
        }
        // §7.1.17 ToPropertyKey: String wrapper objects ToPrimitive → their
        // inner string ("x[new String("0")]" must hit element 0).
        if tag == TAG_STRING_OBJ {
            let sptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            let s = unsafe { HeapString::to_string(sptr as *mut HeapString) };
            return Some(PropertyKey::from_string(&s));
        }
    }
    if let Some(v) = val.as_smi() {
        return Some(PropertyKey::from_string(&v.to_string()));
    }
    // B1f: integral floats are property keys by their canonical string
    // (obj[4294967295] must hit the "4294967295" slot, not miss).
    if let Some(f) = val.as_float64() {
        if !f.is_nan() && f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 {
            return Some(PropertyKey::from_string(&format!("{}", f as i64)));
        }
        return None;
    }
    None
}

/// Check if a Value is the string `"__proto__"` (the special prototype setter key).
fn is_proto_key(val: Value) -> bool {
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            return s == "__proto__";
        }
    }
    false
}

/// Result of GetMethod-style well-known-symbol lookup (§7.3.11 GetMethod).
pub(crate) enum SymbolMethodResult {
    /// No @@method (undefined or null) — fall back to the legacy algorithm.
    NotFound,
    /// The @@method property exists but is not callable — TypeError per spec.
    NotCallable,
    Found(Value),
}

/// §7.3.11 GetMethod: look up a well-known-symbol method on an object.
/// Walks the prototype chain; returns NotFound for undefined/null values.
pub(crate) fn get_symbol_method(
    gc: &mut SemiSpace,
    obj: Value,
    symbol_id: u32,
    function_prototype: Option<Value>,
) -> SymbolMethodResult {
    let method = load_property_recursive(obj, Value::symbol(symbol_id), function_prototype, gc);
    if method.is_undefined() || method.is_null() {
        return SymbolMethodResult::NotFound;
    }
    let callable = method
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_FUNC })
        || method.as_smi().is_some_and(|s| s < 0);
    if callable {
        SymbolMethodResult::Found(method)
    } else {
        SymbolMethodResult::NotCallable
    }
}

/// GetMethod(value, @@iterator): resolve the iteration method for a value.
/// Primitive strings route directly to String.prototype (which holds the
/// symbol-keyed @@iterator property); other objects walk the prototype chain.
pub(crate) fn get_iter_method(vm: &mut Vm, gc: &mut SemiSpace, value: Value) -> SymbolMethodResult {
    if let Some(ptr) = value.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            if vm.string_prototype.is_heap_object() {
                if let Some(proto_ptr) = vm.string_prototype.heap_ptr() {
                    let shape = unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
                    if let Some(slot) =
                        shape.lookup(&PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR))
                    {
                        let m = unsafe { JSObject::get_slot(proto_ptr as *mut JSObject, slot) };
                        if m.is_undefined() || m.is_null() {
                            return SymbolMethodResult::NotFound;
                        }
                        let callable = m.heap_ptr().is_some_and(|p| unsafe {
                            (*(p as *const GcHeader)).tag() == TAG_FUNC
                        }) || m.as_smi().is_some_and(|s| s < 0);
                        return if callable {
                            SymbolMethodResult::Found(m)
                        } else {
                            SymbolMethodResult::NotCallable
                        };
                    }
                }
            }
            SymbolMethodResult::NotFound
        } else if tag == TAG_OBJECT
            || tag == TAG_ARRAY
            || tag == TAG_FUNC
            || tag == TAG_REGEXP
            || tag == TAG_PROMISE
            || tag == TAG_STRING_OBJ
            || tag == TAG_MAP
            || tag == TAG_SET
            || tag == TAG_TYPED_ARRAY
        {
            get_symbol_method(
                gc,
                value,
                rune_core::symbol::SYM_ITERATOR,
                Some(vm.function_prototype),
            )
        } else {
            SymbolMethodResult::NotFound
        }
    } else {
        SymbolMethodResult::NotFound
    }
}

/// Call a builtin by its negative-smi handle. Returns Ok(Value) on success.
/// If the builtin raised an exception: Err(Some(exit)) means it must propagate;
/// Err(None) means the exception was consumed internally (continue the loop).
pub(crate) fn call_builtin_sync(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    handle: Value,
    this: Value,
    args: &[Value],
) -> Result<Value, Option<Exit>> {
    if let Some(smi) = handle.as_smi() {
        if smi < 0 {
            let id = ((-smi) as usize) - 1;
            if id < vm.builtins.len() {
                let result = (vm.builtins[id].func)(gc, this, args, vm);
                if let Some(exc) = vm.pending_exception.take() {
                    return Err(vm.handle_throw(gc, exc));
                }
                return Ok(result);
            }
        }
    }
    Err(vm.throw_type_error(gc, "not a function"))
}

/// Allocate a dense array with DENSE_ARRAY_SHAPE and Array.prototype
/// (mirrors the NewArray opcode setup).
pub(crate) fn new_dense_array(vm: &mut Vm, gc: &mut SemiSpace) -> *mut u8 {
    let arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = arr as *mut u8;
        let shape_ptr = ptr.add(8) as *mut *const Shape;
        *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
        let proto_ptr = ptr.add(24) as *mut *mut u8;
        if vm.array_prototype.is_heap_object() {
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *proto_ptr = proto;
            }
        }
    }
    arr as *mut u8
}

/// Validate an iterator factory result and push [iterator, nextMethod] onto the
/// stack (for..of loop head).
fn complete_for_of_init(vm: &mut Vm, gc: &mut SemiSpace, iterator: Value) -> Result<(), Exit> {
    if !iterator.is_heap_object() {
        if let Some(exit) = vm.throw_routed(gc, "value is not iterable") {
            return Err(exit);
        }
        return Ok(());
    }
    let next = load_property_recursive(iterator, vm.next_key, Some(vm.function_prototype), gc);
    let callable = next.as_smi().is_some_and(|s| s < 0)
        || next
            .heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_FUNC });
    if !callable {
        if let Some(exit) = vm.throw_routed(gc, "iterator.next is not a function") {
            return Err(exit);
        }
        return Ok(());
    }
    vm.push(iterator);
    vm.push(next);
    Ok(())
}

/// Process an iterator `next()` result: done → jump to the loop end (dropping
/// [iterator, nextMethod]); otherwise push the value and advance pc.
/// The stack must be [..., iterator, nextMethod] when called.
fn process_for_of_next_result(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    result: Value,
    end_target: usize,
) -> Result<(), Exit> {
    if !result.is_heap_object() {
        if let Some(exit) = vm.throw_routed(gc, "Iterator result is not an object") {
            return Err(exit);
        }
        return Ok(());
    }
    let done = load_property_recursive(result, vm.done_key, None, gc).to_bool();
    if done {
        // Leave [iterator, nextMethod] on the stack — the loop-end Pop Pop
        // (emitted at the exit target) discards them on this path.
        let frames_len = vm.frames.len();
        vm.frames[frames_len - 1].pc = end_target;
    } else {
        let value = load_property_recursive(result, vm.value_key, None, gc);
        // Push the value ON TOP of [iterator, nextMethod] — the LHS store
        // sequence pops it, leaving [iterator, nextMethod] for the next round.
        vm.push(value);
        let frames_len = vm.frames.len();
        vm.frames[frames_len - 1].pc += 1;
    }
    Ok(())
}

/// Process a `yield*` delegation step result: done → drop [iter, next],
/// push the delegate's return value and jump to the loop end (it becomes
/// the yield* expression value); otherwise push the yielded value.
/// The stack must be [..., iterator, nextMethod] when called.
fn process_yieldstar_next_result(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    result: Value,
    end_target: usize,
) -> Result<(), Exit> {
    if !result.is_heap_object() {
        if let Some(exit) = vm.throw_routed(gc, "Iterator result is not an object") {
            return Err(exit);
        }
        return Ok(());
    }
    // done read (async-capable; result itself is the stash).
    let done = match crate::builtins::ys_read_gen(
        vm,
        gc,
        0,
        Value::undefined(),
        Value::undefined(),
        result,
        crate::vm::YsGetPhase::StarDone { end_target },
        result,
        vm.done_key,
    ) {
        crate::builtins::YsOut::Sync(v) => v.to_bool(),
        crate::builtins::YsOut::Wait => return Ok(()),
        crate::builtins::YsOut::Raise(e) => match vm.handle_throw(gc, e) {
            Some(exit) => return Err(exit),
            None => return Ok(()),
        },
        crate::builtins::YsOut::Bail(Some(exit)) => return Err(exit),
        crate::builtins::YsOut::Bail(None) => return Ok(()),
    };
    if done {
        match crate::builtins::ys_read_gen(
            vm,
            gc,
            0,
            Value::undefined(),
            Value::undefined(),
            result,
            crate::vm::YsGetPhase::StarValueDone { end_target },
            result,
            vm.value_key,
        ) {
            crate::builtins::YsOut::Sync(v) => {
                vm.pop();
                vm.pop();
                vm.push(v);
                let frames_len = vm.frames.len();
                vm.frames[frames_len - 1].pc = end_target;
            }
            crate::builtins::YsOut::Wait => return Ok(()),
            crate::builtins::YsOut::Raise(e) => match vm.handle_throw(gc, e) {
                Some(exit) => return Err(exit),
                None => return Ok(()),
            },
            crate::builtins::YsOut::Bail(Some(exit)) => return Err(exit),
            crate::builtins::YsOut::Bail(None) => return Ok(()),
        }
    } else {
        match crate::builtins::ys_read_gen(
            vm,
            gc,
            0,
            Value::undefined(),
            Value::undefined(),
            result,
            crate::vm::YsGetPhase::StarValue,
            result,
            vm.value_key,
        ) {
            crate::builtins::YsOut::Sync(v) => {
                vm.push(v);
                let frames_len = vm.frames.len();
                vm.frames[frames_len - 1].pc += 1;
            }
            crate::builtins::YsOut::Wait => return Ok(()),
            crate::builtins::YsOut::Raise(e) => match vm.handle_throw(gc, e) {
                Some(exit) => return Err(exit),
                None => return Ok(()),
            },
            crate::builtins::YsOut::Bail(Some(exit)) => return Err(exit),
            crate::builtins::YsOut::Bail(None) => return Ok(()),
        }
    }
    Ok(())
}

/// Start draining an iterator into `arr` (spread conversion). If `next` is a
/// builtin handle the drain is fully synchronous. If `next` is a JS function,
/// sets `pending_iter_drain` and pushes the callback — the caller must
/// `continue` without advancing pc (Err(None) signals this).
fn drain_iterator(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    iterator: Value,
    receiver: Value,
    arr: *mut u8,
) -> Result<*mut u8, Option<Exit>> {
    if !iterator.is_heap_object() {
        return Err(vm.throw_type_error(gc, "value is not iterable"));
    }
    let next = load_property_recursive(iterator, vm.next_key, Some(vm.function_prototype), gc);
    if next.as_smi().is_some_and(|s| s < 0) {
        // Builtin next — drain synchronously.
        let mut cur = arr;
        loop {
            let result = match call_builtin_sync(vm, gc, next, iterator, &[]) {
                Ok(v) => v,
                Err(Some(exit)) => return Err(Some(exit)),
                Err(None) => return Err(None),
            };
            if !result.is_heap_object() {
                return Err(vm.throw_type_error(gc, "Iterator result is not an object"));
            }
            let done = load_property_recursive(result, vm.done_key, None, gc).to_bool();
            if done {
                return Ok(cur);
            }
            let value = load_property_recursive(result, vm.value_key, None, gc);
            cur = unsafe { RuneArray::push(gc, cur as *mut RuneArray, value) } as *mut u8;
        }
    } else if next.is_heap_object()
        && unsafe { (*(next.heap_ptr().unwrap() as *const GcHeader)).tag() } == TAG_FUNC
    {
        vm.pending_iter_drain = Some(PendingIterDrain {
            source_frame_depth: vm.frames.len() - 1,
            state: IterDrainState::AwaitNext,
            iter: iterator,
            next,
            result: arr,
            receiver,
        });
        vm.push_callback_call(gc, next, iterator, vec![]);
        Err(None)
    } else {
        Err(vm.throw_type_error(gc, "iterator.next is not a function"))
    }
}

/// Maximum depth to walk the prototype chain before giving up (cycle guard).
pub(crate) const MAX_PROTOTYPE_DEPTH: usize = 256;

/// Walk the prototype chain to resolve a property.
/// Implements OrdinaryGet (§10.1.8.1): check own property, then recurse on [[Prototype]].
/// For dense arrays: numeric keys access elements directly; non-numeric walks to prototype.
/// Returns undefined if the chain exceeds MAX_PROTOTYPE_DEPTH (prevents infinite loops on cycles).
pub(crate) fn load_property_recursive(
    obj: Value,
    raw_key: Value,
    function_prototype: Option<Value>,
    gc: &mut SemiSpace,
) -> Value {
    let mut current = obj;
    let mut depth = 0;
    loop {
        if depth >= MAX_PROTOTYPE_DEPTH {
            return Value::undefined();
        }
        depth += 1;
        // Builtin handles (negative Smis) are function-like: check Function.prototype
        if let Some(smi) = current.as_smi() {
            if smi < 0 {
                if let Some(fp) = function_prototype {
                    if fp.is_heap_object() {
                        if let Some(ptr) = fp.heap_ptr() {
                            if let Some(key) = value_to_prop_key(raw_key) {
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                if let Some(slot) = shape.lookup(&key) {
                                    return unsafe {
                                        JSObject::get_slot(ptr as *mut JSObject, slot)
                                    };
                                }
                            }
                        }
                    }
                }
                return Value::undefined();
            }
            // Non-negative Smis (real integers) have no properties
            return Value::undefined();
        }
        if let Some(ptr) = current.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_OBJECT {
                if let Some(key) = value_to_prop_key(raw_key) {
                    // __proto__ read returns the internal [[Prototype]]
                    if is_proto_key(raw_key) {
                        let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
                        if proto.is_null() {
                            return Value::undefined();
                        }
                        return Value::from_heap_ptr(proto);
                    }
                    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                    if let Some(slot) = shape.lookup(&key) {
                        let v = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                        return v;
                    }
                    // Not found — walk to prototype
                    let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
                    if proto.is_null() {
                        return Value::undefined();
                    }
                    current = Value::from_heap_ptr(proto);
                    continue;
                } else {
                    return Value::undefined();
                }
            } else if tag == TAG_ARRAY {
                // Dense array: numeric key → overlay entry (accessor pair or
                // single-locked data), then direct element access; holes (B1e)
                // fall through to the prototype walk below. Out-of-bounds
                // indices also walk the prototype (ordinary Get semantics —
                // e.g. Array.prototype[3] serves a shrunk array; TypedArrays
                // keep exotic OOB→undefined below).
                if let Some(index) = value_to_array_index(raw_key) {
                    if let Some(entry) = array_overlay_entry(ptr as *mut RuneArray, index) {
                        return entry;
                    }
                    let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                    // B1e: only allocated slots are read (length-extended
                    // tails and holes fall through to the prototype walk).
                    if index < len as usize
                        && index < unsafe { RuneArray::capacity(ptr as *mut RuneArray) } as usize
                    {
                        let elem = unsafe { RuneArray::get_element(ptr as *mut RuneArray, index) };
                        if elem != Value::empty_sentinel() {
                            return elem;
                        }
                    }
                }
                // "length" property → return stored length
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    let key_tag = unsafe { (*(key_ptr as *const GcHeader)).tag() };
                    if key_tag == TAG_STRING {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        if key_str == "length" {
                            let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                            return crate::builtins::length_value(len as u64);
                        }
                    }
                }
                // Non-numeric key: consult extra_props (named properties like
                // the non-enumerable "index"/"input" on match-result arrays)
                let extra = unsafe { RuneArray::extra_props(ptr as *mut RuneArray) };
                if !extra.is_null() {
                    if let Some(key) = value_to_prop_key(raw_key) {
                        let extra_shape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                        if let Some(slot) = extra_shape.lookup(&key) {
                            return unsafe { JSObject::get_slot(extra as *mut JSObject, slot) };
                        }
                    }
                }
                // Non-numeric key → walk to prototype
                let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
                if proto.is_null() {
                    return Value::undefined();
                }
                current = Value::from_heap_ptr(proto);
                continue;
            } else if tag == TAG_STRING_OBJ {
                // String wrapper: own "length" property, then walk to String.prototype
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    let key_tag = unsafe { (*(key_ptr as *const GcHeader)).tag() };
                    if key_tag == TAG_STRING {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        if key_str == "length" {
                            let str_ptr =
                                unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                            let s = unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
                            let len = s.encode_utf16().count();
                            return Value::smi(len as i32);
                        }
                    }
                }
                // Walk to String.prototype
                let proto = unsafe { StringObject::prototype(ptr as *mut StringObject) };
                if !proto.is_null() {
                    current = Value::from_heap_ptr(proto);
                    continue;
                }
                return Value::undefined();
            } else if tag == TAG_PROMISE {
                let proto = unsafe { Promise::prototype(ptr) };
                if !proto.is_null() {
                    current = Value::from_heap_ptr(proto);
                    continue;
                }
                return Value::undefined();
            } else if tag == TAG_FUNC {
                if let Some(key) = value_to_prop_key(raw_key) {
                    if key == *PROTOTYPE_KEY {
                        let proto_ptr = unsafe { Func::prototype(ptr as *mut Func) };
                        if !proto_ptr.is_null() {
                            return Value::from_heap_ptr(proto_ptr);
                        }
                    }
                }
                // Check extra properties stored on the function (e.g. static methods)
                let extra_props_ptr = unsafe { Func::extra_props(ptr as *mut Func) };
                if !extra_props_ptr.is_null() {
                    let result = load_property_recursive(
                        Value::from_heap_ptr(extra_props_ptr),
                        raw_key,
                        None,
                        gc,
                    );
                    if !result.is_undefined() {
                        return result;
                    }
                }
                // Walk Function.prototype for other properties (e.g. .call, .apply, .bind)
                if let Some(fp) = function_prototype {
                    if fp.is_heap_object() {
                        current = fp;
                        continue;
                    }
                }
                return Value::undefined();
            } else if tag == TAG_REGEXP {
                // Check own properties (source, flags, lastIndex)
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    if unsafe { (*(key_ptr as *const GcHeader)).tag() == TAG_STRING } {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        if key_str == "source" {
                            let pattern_ptr = unsafe { rune_core::regexp::RegExp::pattern(ptr) };
                            return Value::from_heap_ptr(pattern_ptr);
                        }
                        if key_str == "flags" {
                            let f = unsafe { rune_core::regexp::RegExp::flags(ptr) };
                            let mut s = String::new();
                            if f & 1 != 0 {
                                s.push('g');
                            }
                            if f & 2 != 0 {
                                s.push('i');
                            }
                            if f & 4 != 0 {
                                s.push('m');
                            }
                            if f & 8 != 0 {
                                s.push('s');
                            }
                            if f & 16 != 0 {
                                s.push('u');
                            }
                            if f & 32 != 0 {
                                s.push('y');
                            }
                            if f & 64 != 0 {
                                s.push('d');
                            }
                            if f & 128 != 0 {
                                s.push('v');
                            }
                            let ptr = HeapString::allocate(gc, &s);
                            return Value::from_heap_ptr(ptr as *mut u8);
                        }
                        if key_str == "lastIndex" {
                            let li = unsafe { rune_core::regexp::RegExp::last_index(ptr) };
                            return Value::smi(li as i32);
                        }
                    }
                }
                // Walk RegExp.prototype for exec/test and other properties
                let proto_ptr = unsafe { rune_core::regexp::RegExp::prototype(ptr) };
                if !proto_ptr.is_null() {
                    current = Value::from_heap_ptr(proto_ptr);
                    continue;
                }
                return Value::undefined();
            } else if tag == TAG_MAP {
                // Own "size" property (§27.1.3.11) — the live entry count.
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    if unsafe { (*(key_ptr as *const GcHeader)).tag() == TAG_STRING } {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        if key_str == "size" {
                            return Value::smi(unsafe { RuneMap::size(ptr) } as i32);
                        }
                    }
                }
                let proto_ptr = unsafe { RuneMap::prototype(ptr) };
                if !proto_ptr.is_null() {
                    current = Value::from_heap_ptr(proto_ptr);
                    continue;
                }
                return Value::undefined();
            } else if tag == TAG_SET {
                // Own "size" property (§27.2.3.10)
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    if unsafe { (*(key_ptr as *const GcHeader)).tag() == TAG_STRING } {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        if key_str == "size" {
                            return Value::smi(unsafe { RuneSet::size(ptr) } as i32);
                        }
                    }
                }
                let proto_ptr = unsafe { RuneSet::prototype(ptr) };
                if !proto_ptr.is_null() {
                    current = Value::from_heap_ptr(proto_ptr);
                    continue;
                }
                return Value::undefined();
            } else if tag == TAG_TYPED_ARRAY {
                // Integer-indexed exotic object (§10.4.5.1): canonical numeric
                // keys read elements directly (out of bounds → undefined, no
                // prototype consult); own computed keys; else walk proto.
                if let Some(index) = value_to_array_index(raw_key) {
                    let len = unsafe { typedarray::RuneTypedArray::length(ptr) };
                    if index < len {
                        return unsafe { typedarray::read_element(ptr, index) };
                    }
                    return Value::undefined();
                }
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    if unsafe { (*(key_ptr as *const GcHeader)).tag() == TAG_STRING } {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        match key_str.as_str() {
                            "length" => {
                                let len = unsafe { typedarray::RuneTypedArray::length(ptr) };
                                return Value::smi(len as i32);
                            }
                            "byteLength" => {
                                let bl = unsafe { typedarray::RuneTypedArray::byte_length(ptr) };
                                return if bl <= i32::MAX as usize {
                                    Value::smi(bl as i32)
                                } else {
                                    Value::from_float64(bl as f64)
                                };
                            }
                            "byteOffset" => {
                                let off = unsafe { typedarray::RuneTypedArray::byte_offset(ptr) };
                                return Value::smi(off as i32);
                            }
                            "buffer" => {
                                let buf = unsafe { typedarray::RuneTypedArray::buffer(ptr) };
                                return Value::from_heap_ptr(buf);
                            }
                            _ => {}
                        }
                    }
                }
                // Walk %TypedArray.prototype% (per-type prototype stored at 32)
                let proto_ptr = unsafe { typedarray::RuneTypedArray::prototype(ptr) };
                if !proto_ptr.is_null() {
                    current = Value::from_heap_ptr(proto_ptr);
                    continue;
                }
                return Value::undefined();
            } else if tag == TAG_ARRAY_BUFFER {
                // Own "byteLength" (§25.1.5.3); else walk ArrayBuffer.prototype
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    if unsafe { (*(key_ptr as *const GcHeader)).tag() == TAG_STRING } {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        if key_str == "byteLength" {
                            let bl = unsafe { typedarray::RuneArrayBuffer::byte_length(ptr) };
                            return if bl <= i32::MAX as usize {
                                Value::smi(bl as i32)
                            } else {
                                Value::from_float64(bl as f64)
                            };
                        }
                    }
                }
                let proto_ptr = unsafe { typedarray::RuneArrayBuffer::prototype(ptr) };
                if !proto_ptr.is_null() {
                    current = Value::from_heap_ptr(proto_ptr);
                    continue;
                }
                return Value::undefined();
            } else if tag == TAG_DATE {
                // RuneDate has no own properties; walk to Date.prototype
                let proto_ptr = unsafe { date::RuneDate::prototype(ptr) };
                if !proto_ptr.is_null() {
                    current = Value::from_heap_ptr(proto_ptr);
                    continue;
                }
                return Value::undefined();
            }
        }
        return Value::undefined();
    }
}

/// Full property lookup that populates the inline cache on miss.
#[allow(clippy::too_many_arguments)] // several distinct mutable VM subsystems are required
fn load_property_recursive_ic(
    gc: &mut SemiSpace,
    ics: &mut Vec<InlineCache>,
    ic_entries: &mut Vec<IcEntry>,
    ic_hit_counts: &mut Vec<u32>,
    ic_stats: &mut IcStats,
    instr: &Instruction,
    obj: Value,
    raw_key: Value,
    function_prototype: Option<Value>,
) -> Value {
    // Check IC first before doing full lookup
    if instr.ic_index >= 0 {
        if let Some(ptr) = obj.heap_ptr() {
            let ic_idx = instr.ic_index as usize;
            if ic_idx < ics.len() {
                let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                if tag == TAG_OBJECT {
                    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                    let (shape_id, key_hash) = ic_cache_key(shape.id, raw_key);
                    if let Some(entry) = ics[ic_idx].get(shape_id, key_hash) {
                        ic_stats.hits += 1;
                        if entry.is_own {
                            unsafe {
                                return JSObject::get_slot(ptr as *mut JSObject, entry.offset);
                            }
                        } else {
                            // Inherited property: re-walk the proto chain and verify
                            // the key is actually found at the cached depth. The cached
                            // depth was measured from the first instance's chain; other
                            // objects sharing the same shape may resolve the same key at
                            // a different depth (e.g. all Error subtypes share the
                            // "message" shape but chain to different prototypes), so a
                            // blind hop-and-read would read a slot out of the wrong
                            // object.
                            let key = value_to_prop_key(raw_key);
                            let mut p = ptr;
                            let mut depth = 0usize;
                            let mut hit = false;
                            let mut found = Value::undefined();
                            loop {
                                let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                                if next.is_null() {
                                    break;
                                }
                                depth += 1;
                                let next_shape =
                                    unsafe { JSObject::shape_ptr(next as *mut JSObject) };
                                if let Some(offset) = key.and_then(|k| next_shape.lookup(&k)) {
                                    if depth == entry.proto_depth as usize {
                                        found = unsafe {
                                            JSObject::get_slot(next as *mut JSObject, offset)
                                        };
                                        hit = true;
                                    }
                                    break;
                                }
                                p = next;
                            }
                            if hit {
                                return found;
                            }
                            // Mismatch — fall through to full lookup below.
                        }
                    }
                }
            }
        }
    }

    let result = load_property_recursive(obj, raw_key, function_prototype, gc);
    // Don't cache accessor properties in the IC (getter/setter dispatch needs
    // the non-IC path to call the getter function).
    if result.is_heap_object() {
        if let Some(rptr) = result.heap_ptr() {
            if unsafe { (*(rptr as *const GcHeader)).tag() } == TAG_ACCESSOR {
                return result;
            }
        }
    }
    // Populate IC for all result types
    if instr.ic_index >= 0 {
        if let Some(ptr) = obj.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            let ic_idx = instr.ic_index as usize;
            while ics.len() <= ic_idx {
                ics.push(InlineCache::new());
                ic_entries.push(IcEntry::default());
                ic_hit_counts.push(0);
            }
            if tag == TAG_OBJECT {
                if let Some(key) = value_to_prop_key(raw_key) {
                    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                    let (shape_id, key_hash) = ic_cache_key(shape.id, raw_key);
                    if let Some(offset) = shape.lookup(&key) {
                        // Own property
                        ics[ic_idx].insert(
                            shape_id,
                            key_hash,
                            IcEntry {
                                offset,
                                is_own: true,
                                proto_depth: 0,
                            },
                        );
                    } else {
                        // Inherited — walk prototype chain to find offset and depth
                        let mut depth = 0usize;
                        let mut p = ptr;
                        loop {
                            let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                            if next.is_null() {
                                break;
                            }
                            depth += 1;
                            if depth >= MAX_PROTOTYPE_DEPTH {
                                break;
                            }
                            let next_shape = unsafe { JSObject::shape_ptr(next as *mut JSObject) };
                            if let Some(offset) = next_shape.lookup(&key) {
                                ics[ic_idx].insert(
                                    shape_id,
                                    key_hash,
                                    IcEntry {
                                        offset,
                                        is_own: false,
                                        proto_depth: depth as u8,
                                    },
                                );
                                break;
                            }
                            p = next;
                        }
                    }
                }
            } else if tag == TAG_ARRAY {
                // Dense array IC: numeric keys cache element index directly
                if let Some(index) = value_to_array_index(raw_key) {
                    let (shape_id, key_hash) = ic_cache_key(DENSE_ARRAY_SHAPE.id, raw_key);
                    ics[ic_idx].insert(
                        shape_id,
                        key_hash,
                        IcEntry {
                            offset: index,
                            is_own: true,
                            proto_depth: 0,
                        },
                    );
                } else if let Some(key) = value_to_prop_key(raw_key) {
                    // Arrays with extra_props (e.g. match-result "index"/
                    // "input") can't use the proto-walk cache — the key may be
                    // an own named property.
                    let extra = unsafe { RuneArray::extra_props(ptr as *mut RuneArray) };
                    if !extra.is_null() {
                        let extra_shape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                        if extra_shape.lookup(&key).is_some() {
                            return result;
                        }
                    }
                    // Non-numeric key — inherited from Array.prototype
                    let (shape_id, key_hash) = ic_cache_key(DENSE_ARRAY_SHAPE.id, raw_key);
                    let mut depth = 0usize;
                    let mut p = ptr;
                    loop {
                        let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                        if next.is_null() {
                            break;
                        }
                        depth += 1;
                        if depth >= MAX_PROTOTYPE_DEPTH {
                            break;
                        }
                        let next_shape = unsafe { JSObject::shape_ptr(next as *mut JSObject) };
                        if let Some(offset) = next_shape.lookup(&key) {
                            ics[ic_idx].insert(
                                shape_id,
                                key_hash,
                                IcEntry {
                                    offset,
                                    is_own: false,
                                    proto_depth: depth as u8,
                                },
                            );
                            break;
                        }
                        p = next;
                    }
                }
            }
        }
    }
    result
}

/// Perform the full store-property logic (modelled after StoreProperty handler body).
/// Ordinary [[Set]] core (§10.1.8.1 lite). Returns true when the store took
/// effect, false when rejected (non-writable existing prop or new key on a
/// non-extensible object — A4). The caller decides strict-throw vs sloppy
/// silence. Only TAG_OBJECT consults attributes/extensibility; other tags
/// keep legacy behavior (their exotic semantics live inline below).
pub(crate) fn do_store_property(
    obj: Value,
    raw_key: Value,
    value: Value,
    gc: &mut SemiSpace,
    vm: &mut Vm,
) -> bool {
    if let Some(ptr) = obj.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_OBJECT {
            if is_proto_key(raw_key) {
                // __proto__ assignment = SetPrototypeOf (fails on
                // non-extensible receivers).
                if unsafe { !JSObject::is_extensible(ptr as *mut JSObject) } {
                    return false;
                }
                if let Some(val_ptr) = value.heap_ptr() {
                    unsafe { JSObject::set_prototype(ptr as *mut JSObject, val_ptr) };
                } else {
                    unsafe { JSObject::set_prototype(ptr as *mut JSObject, std::ptr::null_mut()) };
                }
                return true;
            } else if let Some(key) = value_to_prop_key(raw_key) {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                if let Some(slot) = shape.lookup(&key) {
                    // Fast path: all-default shapes are writable by construction.
                    if !shape.all_default_attrs
                        && shape.attr_at(slot) & rune_core::shape::ATTR_WRITABLE == 0
                    {
                        return false;
                    }
                    unsafe { JSObject::set_slot(ptr as *mut JSObject, slot, value) };
                    return true;
                } else {
                    if unsafe { !JSObject::is_extensible(ptr as *mut JSObject) } {
                        return false;
                    }
                    let key_name = if key.is_symbol() {
                        // Marker name for symbol-keyed entries (never enumerated —
                        // for-in/Object.keys/JSON.stringify all skip symbol keys).
                        "\u{0}".to_string()
                    } else {
                        value_to_debug_string(raw_key)
                    };
                    // B1f: grow beyond reserve (with root updates) instead of
                    // hitting the capacity assert — the local `ptr` may be
                    // stale afterwards, but this path returns immediately.
                    let live = vm.ensure_object_capacity(gc, ptr as *mut JSObject);
                    unsafe { JSObject::add_property(live, key, key_name, value) };
                    return true;
                }
            }
            return true;
        } else if tag == TAG_ARRAY {
            if let Some(index) = value_to_array_index(raw_key) {
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                if (index as u32) < len {
                    // B1f-5 integrity gates: frozen elements reject all
                    // in-bounds writes; hole-fills create (need extensible).
                    let arr = ptr as *mut RuneArray;
                    if unsafe { RuneArray::elems_are_nonwritable(arr) } {
                        return false;
                    }
                    // B1f-5: overlay entries own their index. Pairs are
                    // dispatched upstream (setter scan); a stray pair here is
                    // a backstop no-op. Data entries write their slot gated
                    // by their own writability (frozen already rejected).
                    {
                        let extra = unsafe { RuneArray::extra_props(arr) };
                        if !extra.is_null() {
                            let ekey = PropertyKey::from_string(&index.to_string());
                            let eshape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                            if let Some(slot) = eshape.lookup(&ekey) {
                                let v = unsafe { JSObject::get_slot(extra as *mut JSObject, slot) };
                                let is_pair = v.heap_ptr().is_some_and(|vp| unsafe {
                                    (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR
                                });
                                if is_pair {
                                    return true;
                                }
                                if eshape.attr_at(slot) & rune_core::shape::ATTR_WRITABLE == 0 {
                                    return false;
                                }
                                unsafe { JSObject::set_slot(extra as *mut JSObject, slot, value) };
                                return true;
                            }
                        }
                    }
                    // Sealed (non-extensible, writable) arrays: hole (absent)
                    // writes create → reject; present rewrites stay allowed.
                    if !unsafe { RuneArray::is_extensible(arr) } {
                        // Hole (absent) writes create → reject; present
                        // rewrites stay allowed on sealed (writable) arrays.
                        let cap = unsafe { RuneArray::capacity(arr) };
                        let present = index < cap as usize
                            && unsafe { RuneArray::get_element(arr, index) }
                                != Value::empty_sentinel();
                        if !present {
                            return false;
                        }
                    }
                    // B1e: the slot may be unallocated (length-extended
                    // arrays have length > capacity) — grow first, never
                    // write OOB. Absurd indices stay holes (same 1M bound
                    // as ensure_dense_capacity — materializing them OOMs).
                    let cap = unsafe { RuneArray::capacity(ptr as *mut RuneArray) };
                    if index >= cap as usize {
                        if index > 1_000_000 {
                            return true;
                        }
                        unsafe {
                            let (resolved_old, new_arr) =
                                RuneArray::grow(gc, ptr as *mut RuneArray);
                            if resolved_old != new_arr as *mut u8 {
                                vm.update_heap_reference(resolved_old, new_arr as *mut u8);
                            }
                            RuneArray::set_element(new_arr, index, value);
                            // The caller's `obj` may hold the old address;
                            // refresh it the same way the grow path below does.
                            let _ = obj;
                        }
                    } else {
                        unsafe { RuneArray::set_element(ptr as *mut RuneArray, index, value) };
                    }
                } else {
                    // B1f-5 exotic creation gates (§10.4.2.1 step 2.3):
                    // beyond-length indices always create — non-extensible
                    // arrays reject, as do non-writable lengths.
                    let arr = ptr as *mut RuneArray;
                    if !unsafe { RuneArray::is_extensible(arr) }
                        || !unsafe { RuneArray::length_is_writable(arr) }
                    {
                        return false;
                    }
                    // §7.3.4 CreateDataPropertyOrThrow on an array: indices at
                    // or beyond length GROW the array (length = index+1).
                    // Pad with holes via push (handles grow + root updates),
                    // mirroring the array_push discipline. Huge indices
                    // (past the 1M sparse threshold) become named extra_props
                    // entries instead — materializing billions of slots OOMs
                    // (B1f: S15.4.5.2_A3_T4 stores at 2^32-2). Length is not
                    // extended for the named route (B1f-5 length work owns
                    // huge lengths); reads still find the value through the
                    // extra_props fallthrough in load_property_recursive.
                    if index > 1_000_000 {
                        let key = value_to_prop_key(raw_key);
                        let mut arr_ptr = obj.heap_ptr().unwrap();
                        unsafe {
                            let mut props = RuneArray::extra_props(arr_ptr as *mut RuneArray);
                            if props.is_null() {
                                let new_obj = JSObject::allocate(gc, Shape::empty(), &[]);
                                let gc_tag = (*(arr_ptr as *const GcHeader)).tag();
                                if gc_tag == TAG_ARRAY
                                    && (*(arr_ptr as *const GcHeader)).is_forwarded()
                                {
                                    arr_ptr = (*(arr_ptr as *const GcHeader)).forwarding_addr();
                                }
                                RuneArray::set_extra_props(
                                    arr_ptr as *mut RuneArray,
                                    new_obj as *mut u8,
                                );
                                props = new_obj as *mut u8;
                            }
                            if let Some(key) = key {
                                let eshape = JSObject::shape_ptr(props as *mut JSObject);
                                if let Some(slot) = eshape.lookup(&key) {
                                    JSObject::set_slot(props as *mut JSObject, slot, value);
                                } else {
                                    let key_name = value_to_debug_string(raw_key);
                                    JSObject::add_property(
                                        props as *mut JSObject,
                                        key,
                                        key_name,
                                        value,
                                    );
                                }
                            }
                        }
                        return true;
                    }
                    let mut cur = ptr as *mut RuneArray;
                    unsafe {
                        for k in len..=(index as u32) {
                            let v = if k == index as u32 {
                                value
                            } else {
                                Value::empty_sentinel()
                            };
                            let new_arr = RuneArray::push(gc, cur, v);
                            if new_arr as *mut u8 != cur as *mut u8 {
                                let resolved_old = if (*(cur as *const GcHeader)).is_forwarded() {
                                    (*(cur as *const GcHeader)).forwarding_addr()
                                } else {
                                    cur as *mut u8
                                };
                                vm.update_heap_reference(resolved_old, new_arr as *mut u8);
                                // Keep obj's stack slot fresh too — callers may
                                // still hold the old address.
                                let _ = obj;
                            }
                            cur = new_arr;
                        }
                        let _ = obj; // obj refreshed by update_heap_reference above
                    }
                }
            } else if let Some(key_str) = raw_key.heap_ptr() {
                let k = unsafe { HeapString::to_string(key_str as *mut HeapString) };
                if k == "length" {
                    // B1f-5: full ArraySetLength (coercion RangeErrors,
                    // non-writable rejection, shrink deletes with k+1 abort).
                    // Only the funnel observes the stashed RangeError (all
                    // other do_store callers use non-"length" keys); plain
                    // denials stay silent-false for the funnel's strict gate.
                    let mut arr_val = obj;
                    match crate::builtins::array_store_length_value(gc, vm, &mut arr_val, value) {
                        Ok(()) => return true,
                        Err(crate::builtins::LengthSetErr::Range(e)) => {
                            vm.set_pending_exception(e);
                            return false;
                        }
                        Err(crate::builtins::LengthSetErr::Deny) => return false,
                    }
                } else if let Some(key) = value_to_prop_key(raw_key) {
                    // Named property → extra_props JSObject (lazily allocated).                } else if let Some(key) = value_to_prop_key(raw_key) {
                    // Named property → extra_props JSObject (lazily allocated).
                    // Re-resolve ptr after allocation (GC may move objects).
                    let _key = key;
                    let mut arr_ptr = obj.heap_ptr().unwrap();
                    unsafe {
                        let mut props = RuneArray::extra_props(arr_ptr as *mut RuneArray);
                        if props.is_null() {
                            let new_obj = JSObject::allocate(gc, Shape::empty(), &[]);
                            let gc_tag = (*(arr_ptr as *const GcHeader)).tag();
                            if gc_tag == TAG_ARRAY && (*(arr_ptr as *const GcHeader)).is_forwarded()
                            {
                                arr_ptr = (*(arr_ptr as *const GcHeader)).forwarding_addr();
                            }
                            RuneArray::set_extra_props(
                                arr_ptr as *mut RuneArray,
                                new_obj as *mut u8,
                            );
                            props = new_obj as *mut u8;
                        }
                        do_store_property(Value::from_heap_ptr(props), raw_key, value, gc, vm);
                    }
                }
            }
        } else if tag == TAG_TYPED_ARRAY {
            // Integer-indexed exotic object (§10.4.5.2): numeric canonical
            // keys write elements (out of range / non-canonical → no-op).
            if let Some(index) = value_to_array_index(raw_key) {
                let len = unsafe { typedarray::RuneTypedArray::length(ptr) };
                if index < len {
                    let kind = unsafe { typedarray::RuneTypedArray::kind(ptr) };
                    let n = to_number(value);
                    unsafe {
                        typedarray::write_element(ptr, index, typedarray::convert_number(kind, n));
                    }
                }
            }
        } else if tag == TAG_REGEXP {
            // RegExp instance properties: only "lastIndex" is writable.
            // ToLength (§22.2.7.2 step 3) coerces the stored value on use.
            if let Some(key_ptr) = raw_key.heap_ptr() {
                let key_tag = unsafe { (*(key_ptr as *const GcHeader)).tag() };
                if key_tag == TAG_STRING {
                    let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                    if key_str == "lastIndex" {
                        let n = to_number(value);
                        let clamped = if n.is_nan() || n <= 0.0 { 0 } else { n as u32 };
                        unsafe { rune_core::regexp::RegExp::set_last_index(ptr, clamped) };
                    }
                }
            }
        } else if tag == TAG_FUNC {
            if let Some(key) = value_to_prop_key(raw_key) {
                if key == *PROTOTYPE_KEY {
                    if let Some(val_ptr) = value.heap_ptr() {
                        unsafe {
                            Func::set_prototype(ptr as *mut Func, val_ptr);
                        }
                    }
                } else {
                    // Store arbitrary properties on the function's extra_props object
                    // Must re-resolve ptr from obj after each allocation since GC may move objects
                    let mut obj_ptr = obj.heap_ptr().unwrap();
                    unsafe {
                        let mut props = Func::extra_props(obj_ptr as *mut Func);
                        if props.is_null() {
                            // Lazily allocate a JSObject for extra properties.
                            // GC may move objects during allocation; re-resolve from the Value.
                            let new_obj = JSObject::allocate(gc, Shape::empty(), &[]);
                            // After allocation, resolve forwarding for obj (it may have moved)
                            let gc_tag = (*(obj_ptr as *const GcHeader)).tag();
                            if gc_tag == TAG_FUNC && (*(obj_ptr as *const GcHeader)).is_forwarded()
                            {
                                obj_ptr = (*(obj_ptr as *const GcHeader)).forwarding_addr();
                            }
                            Func::set_extra_props(obj_ptr as *mut Func, new_obj as *mut u8);
                            props = new_obj as *mut u8;
                        }
                        do_store_property(Value::from_heap_ptr(props), raw_key, value, gc, vm);
                    }
                }
            }
        }
    }
    // Legacy paths above preserve silent no-op behavior (treated as success;
    // only TAG_OBJECT reports real rejections for now).
    true
}

/// Convert a Value to an f64 for numeric operations.
/// Returns NaN for non-numeric types (undefined, null, objects, strings).
pub(crate) fn to_number(v: Value) -> f64 {
    if let Some(n) = v.as_smi() {
        n as f64
    } else if let Some(n) = v.as_float64() {
        n
    } else if v.is_null() {
        0.0
    } else if v.is_undefined() {
        f64::NAN
    } else if v.is_boolean() {
        // §7.1.4: ToNumber(Boolean) — true → 1, false → 0
        if v.to_boolean() == Some(true) {
            1.0
        } else {
            0.0
        }
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING || tag == TAG_STRING_OBJ {
            let s = if tag == TAG_STRING {
                unsafe { HeapString::to_string(ptr as *mut HeapString) }
            } else {
                let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                unsafe { HeapString::to_string(str_ptr as *mut HeapString) }
            };
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return 0.0;
            }
            if let Ok(n) = trimmed.parse::<f64>() {
                return n;
            }
            // Hex literals like "0x1F"
            let upper = trimmed.to_uppercase();
            if let Some(rest) = upper.strip_prefix("0X") {
                if let Ok(n) = u64::from_str_radix(rest, 16) {
                    return n as f64;
                }
            }
            // Infinity
            if trimmed.eq_ignore_ascii_case("infinity")
                || trimmed == "+Infinity"
                || trimmed == "-Infinity"
            {
                return if trimmed.starts_with('-') {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                };
            }
            f64::NAN
        } else if tag == TAG_FLOAT64 {
            unsafe { *(ptr.add(std::mem::size_of::<GcHeader>()) as *const f64) }
        } else if tag == TAG_DATE {
            // §7.1.4 + §7.1.1.1 ToPrimitive(Date, number): Date's default hint is
            // string, but Number(x) uses the number hint → [[DateValue]]
            unsafe { date::RuneDate::tv(ptr) }
        } else {
            f64::NAN
        }
    } else {
        // Symbol and other exotic values fall through to NaN here;
        // callers that need TypeError must check is_symbol() before calling.
        f64::NAN
    }
}

/// §7.1.6 ToInt32: Convert a Value to a signed 32-bit integer.
fn to_int32(v: Value) -> i32 {
    let n = to_number(v);
    if n.is_nan() || n.is_infinite() {
        return 0;
    }
    // Truncate toward zero
    let int = n.trunc();
    // Mod 2^32 (positive)
    let int32bit = int.rem_euclid(4294967296.0);
    // If ≥ 2^31, wrap to negative
    if int32bit >= 2147483648.0 {
        (int32bit - 4294967296.0) as i32
    } else {
        int32bit as i32
    }
}

/// Wrap an f64 result back into a Value, trying to use Smi for small integers.
/// Uses NaN-boxing (not heap allocation) for float64 values.
fn number_result(_gc: &mut SemiSpace, val: f64) -> Value {
    if val.is_nan() || val.is_infinite() {
        return Value::from_float64(val);
    }
    if val.fract() == 0.0 {
        // Preserve -0.0 as NaN-boxed f64; Smi would lose the sign bit
        if val == 0.0 && val.is_sign_negative() {
            return Value::from_float64(val);
        }
        let i = val as i64;
        if i >= -(1 << 30) as i64 && i < (1 << 30) as i64 {
            return Value::smi(val as i32);
        }
    }
    Value::from_float64(val)
}

/// Compute the IC cache key combining shape.id with the property key,
/// so that different keys on the same shape produce distinct cache entries.
fn ic_cache_key(shape_id: u64, raw_key: Value) -> (u64, u64) {
    if let Some(idx) = value_to_array_index(raw_key) {
        (shape_id, idx as u64)
    } else if let Some(key) = value_to_prop_key(raw_key) {
        (shape_id, key.as_u64())
    } else {
        (shape_id, 0)
    }
}

/// Check if an object has a property (for the `in` operator).
/// Returns false for non-object values (primitives are not objects).
/// HasProperty for hole-skipping (B1a): own or proto-chain presence.
/// B1e: own-accessor overlay for dense arrays. `defineProperty` with a
/// getter/setter on an array index stores the AccessorPair in extra_props
/// under the numeric key (dense elements can't hold pairs inline); the
/// dense slot itself is a hole. Returns the pair when present.
pub(crate) fn array_overlay_accessor(arr_ptr: *mut RuneArray, idx: usize) -> Option<Value> {
    let v = array_overlay_entry(arr_ptr, idx)?;
    v.heap_ptr()
        .filter(|vp| unsafe { (*(*vp as *const GcHeader)).tag() } == TAG_ACCESSOR)
        .map(|_| v)
}

/// Any overlay entry (accessor pair OR data) at an index (B1f-5: single-locked
/// data indices live here with their attrs; dense holds only all-default
/// data). Presence/value paths use this; getter/setter dispatch paths keep
/// the pair-only version above (data has no accessors to dispatch).
pub(crate) fn array_overlay_entry(arr_ptr: *mut RuneArray, idx: usize) -> Option<Value> {
    let extra = unsafe { RuneArray::extra_props(arr_ptr) };
    if extra.is_null() {
        return None;
    }
    let key = PropertyKey::from_string(&idx.to_string());
    let shape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
    let slot = shape.lookup(&key)?;
    Some(unsafe { JSObject::get_slot(extra as *mut JSObject, slot) })
}

/// IsConstructor (§7.2.4, B1f-6a sync subset): user functions need
/// [[Construct]] — arrows, async functions and generators lack it (classes
/// keep it); known builtin ctor objects construct via the New arms; Smi
/// builtin handles don't (except Test262Error); everything else (including
/// plain objects) doesn't. No realms/Proxy/bound (out of scope).
pub(crate) fn value_is_constructor(vm: &Vm, v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_FUNC {
            let fp = ptr as *mut Func;
            return unsafe {
                !Func::is_arrow(fp) && !Func::is_async_fn(fp) && !Func::is_generator_fn(fp)
            };
        }
        if v == vm.array_constructor
            || v == vm.object_constructor
            || v == vm.number_constructor
            || v == vm.string_constructor
            || v == vm.date_constructor
            || v == vm.regexp_constructor
            || v == vm.promise_constructor
            || v == vm.map_constructor
            || v == vm.set_constructor
            || v == vm.array_buffer_constructor
        {
            return true;
        }
        if vm.error_ctors.contains(&v) || vm.typed_array_ctors.contains(&v) {
            return true;
        }
        return false;
    }
    if let Some(smi) = v.as_smi() {
        if smi < 0 {
            let id = ((-smi) as usize) - 1;
            return id < vm.builtins.len() && vm.builtins[id].name == "Test262Error";
        }
    }
    false
}

pub(crate) fn has_property(obj: Value, raw_key: Value, function_prototype: Option<Value>) -> bool {
    // Builtin handles (negative Smis) are function-like: check Function.prototype
    if let Some(smi) = obj.as_smi() {
        if smi < 0 {
            if let Some(fp) = function_prototype {
                if fp.is_heap_object() {
                    return has_property(fp, raw_key, function_prototype);
                }
            }
            return false;
        }
        return false;
    }
    if let Some(ptr) = obj.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_OBJECT {
            if let Some(key) = value_to_prop_key(raw_key) {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                if shape.lookup(&key).is_some() {
                    return true;
                }
                // Walk prototype chain. B1f-3: non-plain links (e.g. a dense
                // array serving elements to a plain object — 9-3) delegate to
                // the funnel instead of reading absent (load already serves
                // them; `in` must agree).
                let mut current = obj;
                let mut depth = 0;
                loop {
                    if depth >= 5_000 {
                        return false;
                    }
                    depth += 1;
                    let cur_ptr = current.heap_ptr().unwrap();
                    let cur_tag = unsafe { (*(cur_ptr as *const GcHeader)).tag() };
                    if cur_tag == TAG_OBJECT {
                        let proto = unsafe { JSObject::prototype(cur_ptr as *mut JSObject) };
                        if proto.is_null() {
                            return false;
                        }
                        current = Value::from_heap_ptr(proto);
                        if let Some(proto_ptr) = current.heap_ptr() {
                            let proto_tag = unsafe { (*(proto_ptr as *const GcHeader)).tag() };
                            if proto_tag == TAG_OBJECT {
                                let proto_shape =
                                    unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
                                if proto_shape.lookup(&key).is_some() {
                                    return true;
                                }
                            } else {
                                // B1f-3: delegate exotic links (dense arrays,
                                // typed arrays, ...) to the funnel — their own
                                // arms know holes/overlays/exotics and continue
                                // up the chain from there.
                                return has_property(current, raw_key, function_prototype);
                            }
                        } else {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
            false
        } else if tag == TAG_FUNC {
            // Static methods/private elements live on the function's extra_props
            if let Some(key) = value_to_prop_key(raw_key) {
                if key == *PROTOTYPE_KEY {
                    return true;
                }
                let extra = unsafe { Func::extra_props(ptr as *mut Func) };
                if !extra.is_null() {
                    let shape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                    if shape.lookup(&key).is_some() {
                        return true;
                    }
                }
            }
            // Walk the superclass chain (static inheritance), else Function.prototype
            let super_ptr = unsafe { Func::superclass(ptr as *mut Func) };
            if super_ptr.is_null() {
                if let Some(fp) = function_prototype {
                    if fp.is_heap_object() {
                        return has_property(fp, raw_key, function_prototype);
                    }
                }
                return false;
            }
            has_property(Value::from_heap_ptr(super_ptr), raw_key, function_prototype)
        } else if tag == TAG_ARRAY {
            if let Some(index) = value_to_array_index(raw_key) {
                // B1e: an own overlay entry (accessor pair or data) counts
                // as present.
                if array_overlay_entry(ptr as *mut RuneArray, index).is_some() {
                    return true;
                }
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                if index < len as usize {
                    // B1e: a hole (empty sentinel) is NOT an own property —
                    // fall through to the prototype walk below (a proto
                    // getter/setter may serve the index). Unallocated tail
                    // slots (length-extended arrays) are holes too.
                    if index < unsafe { RuneArray::capacity(ptr as *mut RuneArray) } as usize {
                        let elem = unsafe { RuneArray::get_element(ptr as *mut RuneArray, index) };
                        if elem != Value::empty_sentinel() {
                            return true;
                        }
                    }
                    // B1f: huge canonical indices live as named extra_props
                    // (never materialized) — still own properties.
                    if let Some(key) = value_to_prop_key(raw_key) {
                        let extra = unsafe { RuneArray::extra_props(ptr as *mut RuneArray) };
                        if !extra.is_null() {
                            let eshape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                            if eshape.lookup(&key).is_some() {
                                return true;
                            }
                        }
                    }
                } else {
                    // Out of bounds: same named-overflow check as above.
                    if let Some(key) = value_to_prop_key(raw_key) {
                        let extra = unsafe { RuneArray::extra_props(ptr as *mut RuneArray) };
                        if !extra.is_null() {
                            let eshape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                            if eshape.lookup(&key).is_some() {
                                return true;
                            }
                        }
                    }
                    return false;
                }
            }
            if let Some(key_ptr) = raw_key.heap_ptr() {
                let key_tag = unsafe { (*(key_ptr as *const GcHeader)).tag() };
                if key_tag == TAG_STRING {
                    let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                    if key_str == "length" {
                        return true;
                    }
                }
            }
            // Walk prototype chain for non-numeric keys on arrays
            has_property(
                unsafe {
                    let proto = JSObject::prototype(ptr as *mut JSObject);
                    if proto.is_null() {
                        return false;
                    }
                    Value::from_heap_ptr(proto)
                },
                raw_key,
                function_prototype,
            )
        } else if tag == TAG_TYPED_ARRAY {
            // Integer-indexed exotic object: in-bounds numeric index exists;
            // other keys walk the prototype chain.
            if let Some(index) = value_to_array_index(raw_key) {
                let len = unsafe { typedarray::RuneTypedArray::length(ptr) };
                return index < len;
            }
            let proto_ptr = unsafe { typedarray::RuneTypedArray::prototype(ptr) };
            if !proto_ptr.is_null() {
                return has_property(Value::from_heap_ptr(proto_ptr), raw_key, function_prototype);
            }
            false
        } else if tag == TAG_STRING {
            if let Some(index) = value_to_array_index(raw_key) {
                let len = unsafe { HeapString::len(ptr as *mut HeapString) };
                return index < len;
            }
            // Walk String.prototype for non-numeric keys
            false
        } else {
            false
        }
    } else {
        false
    }
}

/// OrdinaryHasInstance per §13.10.2: walk lhs prototype chain looking for rhs_proto.
fn ordinary_has_instance(lhs: Value, rhs_proto_ptr: *mut u8) -> bool {
    let mut current = lhs;
    let mut depth = 0;
    loop {
        if depth >= MAX_PROTOTYPE_DEPTH {
            return false;
        }
        depth += 1;
        if let Some(ptr) = current.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            let proto = if tag == TAG_OBJECT || tag == TAG_ARRAY {
                unsafe { JSObject::prototype(ptr as *mut JSObject) }
            } else if tag == TAG_MAP {
                unsafe { RuneMap::prototype(ptr) }
            } else if tag == TAG_SET {
                unsafe { RuneSet::prototype(ptr) }
            } else if tag == TAG_DATE {
                unsafe { date::RuneDate::prototype(ptr) }
            } else if tag == TAG_TYPED_ARRAY {
                unsafe { typedarray::RuneTypedArray::prototype(ptr) }
            } else if tag == TAG_REGEXP {
                unsafe { rune_core::regexp::RegExp::prototype(ptr) }
            } else {
                return false;
            };
            if proto.is_null() {
                return false;
            }
            if proto == rhs_proto_ptr {
                return true;
            }
            current = Value::from_heap_ptr(proto);
        } else {
            return false;
        }
    }
}

/// Check if a Value is a GC-allocated string.
pub(crate) fn value_is_string(v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        unsafe { (*(ptr as *const GcHeader)).tag() == TAG_STRING }
    } else {
        false
    }
}

/// Convert a Value to an array index if it is a non-negative Smi.
/// Canonical array-index key test (B1f): all digits, no leading zeros
/// (unless the key is exactly "0"), value < 2^32-1. Used by the key model
/// and the move-range quiet check alike.
pub(crate) fn canonical_index_name(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    let n: u64 = s.parse().ok()?;
    if n >= 4_294_967_295 {
        return None;
    }
    if n.to_string() != s {
        return None;
    }
    Some(n)
}

pub(crate) fn value_to_array_index(v: Value) -> Option<usize> {
    if let Some(n) = v.as_smi() {
        if n >= 0 { Some(n as usize) } else { None }
    } else if let Some(f) = v.as_float64() {
        // B1f: integral floats are canonical index keys (arr[1.0] === arr[1]).
        if f.is_nan() || !(0.0..4_294_967_295.0).contains(&f) || f.fract() != 0.0 {
            None
        } else {
            Some(f as usize)
        }
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            // B1f: canonical form only ("01"/"+1"/"4294967295" are names).
            canonical_index_name(&s).map(|n| n as usize)
        } else if tag == TAG_STRING_OBJ {
            let sptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            let s = unsafe { HeapString::to_string(sptr as *mut HeapString) };
            canonical_index_name(&s).map(|n| n as usize)
        } else {
            None
        }
    } else {
        None
    }
}

/// Get the length of an array-like value.
/// Returns None if `this` is not an array-like (neither TAG_ARRAY nor TAG_OBJECT with "length" property).
pub(crate) fn array_like_length(this: Value) -> Option<u32> {
    let ptr = this.heap_ptr()?;
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    match tag {
        TAG_ARRAY => Some(unsafe { RuneArray::length(ptr as *mut RuneArray) }),
        TAG_OBJECT => {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            let key = PropertyKey::from_string("length");
            shape.lookup(&key).map(|slot| {
                let val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                if let Some(n) = val.as_smi() {
                    n.max(0) as u32
                } else if let Some(f) = val.as_float64() {
                    f.max(0.0) as u32
                } else if val.to_boolean().is_some_and(|b| b) {
                    // B1f-3: ToLength(true) is 1 ({length: true} — 3-2).
                    1
                } else {
                    0
                }
            })
        }
        _ => None,
    }
}

/// Get an indexed element from an array-like value ([[Get]](index)).
/// Returns None if the index is out of bounds or the element is a hole.
/// For TAG_ARRAY, returns Some(undefined) for deleted elements.
/// For TAG_OBJECT, returns None if the property doesn't exist (own-property only; no prototype walk).
pub(crate) fn array_like_index(this: Value, i: u32) -> Option<Value> {
    let ptr = this.heap_ptr()?;
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    match tag {
        TAG_ARRAY => {
            let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
            // B1e: unallocated tail slots (length-extended) are holes.
            let cap = unsafe { RuneArray::capacity(ptr as *mut RuneArray) };
            if i < len && i < cap {
                let elem = unsafe { RuneArray::get_element(ptr as *mut RuneArray, i as usize) };
                // B1e: holes read as undefined in value contexts.
                Some(if elem == Value::empty_sentinel() {
                    Value::undefined()
                } else {
                    elem
                })
            } else {
                None
            }
        }
        TAG_OBJECT => {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            let key = PropertyKey::from_string(&i.to_string());
            shape
                .lookup(&key)
                .map(|slot| unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) })
        }
        _ => None,
    }
}

/// Outcome of B1a array-element resolution with getter dispatch.
pub(crate) enum ArrayElemOut {
    /// Synchronous value (data, missing→undefined, undefined/null or
    /// builtin getter result).
    Ready(Value),
    /// A JS getter frame was pushed (pending_array_op carries
    /// awaiting_element); the caller must bail without advancing.
    Wait,
    /// A builtin getter failed synchronously; the error value must be
    /// routed (setup: pending_exception; Return arm: handle_throw).
    SyncErr(Value),
}

/// Push a frame for a JS accessor (getter with no arg, setter with one).
/// Shared by array-element resolution (B1a) and the sort machine (B1e),
/// which record their own resume state instead of pending_accessor_call.
/// Returns false when the function index is invalid (caller reads undefined).
pub(crate) fn push_accessor_frame(
    vm: &mut Vm,
    func_val: Value,
    receiver: Value,
    arg: Option<Value>,
) -> bool {
    let Some(gptr) = func_val.heap_ptr() else {
        return false;
    };
    if unsafe { (*(gptr as *const GcHeader)).tag() } != TAG_FUNC {
        return false;
    }
    let func_idx = unsafe { Func::func_index(gptr as *mut Func) } as usize;
    let creator_prog = unsafe { &*(Func::prog_ptr(gptr as *mut Func) as *const BytecodeProgram) };
    if func_idx >= creator_prog.functions.len() {
        return false;
    }
    let func_prog = &creator_prog.functions[func_idx];
    let func_env = unsafe { Func::env_ptr(gptr as *mut Func) };
    // B1e: see last_popped_callee.
    vm.last_pushed_callee = func_val;
    let mut locals = if func_prog.named_function {
        vec![func_val]
    } else {
        vec![]
    };
    let passed_argc = if arg.is_some() { 1 } else { 0 };
    if let Some(a) = arg {
        locals.push(a);
    }
    vm.frames.push(Frame {
        locals,
        lexical_slots: Vec::new(),
        lexical_tdz: Vec::new(),
        lexical_const: Vec::new(),
        scope_boundaries: Vec::new(),
        passed_argc,
        pc: 0,
        stack_base: vm.stack.len(),
        prog: func_prog as *const BytecodeProgram,
        generator_id: None,
        this: receiver,
        is_constructor_call: false,
        constructed_object: Value::undefined(),
        env: func_env,
        func_ptr: gptr,
        private_name_ids: std::ptr::null_mut(),
    });
    true
}

/// Resolve an array-iteration element with accessor dispatch (B1a).
/// Presence was established by HasProperty; this reads the value, running
/// getters: builtin getters run inline, JS getters push a frame (Wait).
/// `source` is both lookup start and call receiver.
pub(crate) fn array_element_value(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    source: Value,
    index: usize,
) -> ArrayElemOut {
    let raw = load_property_recursive(source, Value::smi(index as i32), None, gc);
    let Some(aptr) = raw.heap_ptr() else {
        return ArrayElemOut::Ready(raw);
    };
    if unsafe { (*(aptr as *const GcHeader)).tag() } != TAG_ACCESSOR {
        return ArrayElemOut::Ready(raw);
    }
    let getter = unsafe { AccessorPair::getter(aptr) };
    if getter.is_undefined() || getter.is_null() {
        return ArrayElemOut::Ready(Value::undefined());
    }
    if getter.as_smi().is_some_and(|s| s < 0) {
        return match call_builtin_sync(vm, gc, getter, source, &[]) {
            Ok(v) => ArrayElemOut::Ready(v),
            Err(_) => {
                let err = crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "getter threw",
                );
                ArrayElemOut::SyncErr(err)
            }
        };
    }
    let Some(gptr) = getter.heap_ptr() else {
        return ArrayElemOut::Ready(Value::undefined());
    };
    if unsafe { (*(gptr as *const GcHeader)).tag() } != TAG_FUNC {
        return ArrayElemOut::Ready(Value::undefined());
    }
    // JS getter: mirror resolve_accessor_for_read's frame push, but record
    // the array-iteration resume (not pending_accessor_call — that would
    // resume the wrong opcode).
    if !push_accessor_frame(vm, getter, source, None) {
        return ArrayElemOut::Ready(Value::undefined());
    }
    ArrayElemOut::Wait
}

/// Push onto a pending machine result array, repairing roots on grow (B1f-3).
/// Factors the op.result forwarding dance previously copy-pasted per arm:
/// on grow the array moves — resolve forwarding and retarget VM roots.
pub(crate) fn array_result_push(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    old_ptr: *mut u8,
    val: Value,
) -> *mut u8 {
    let new_arr = unsafe { RuneArray::push(gc, old_ptr as *mut RuneArray, val) };
    if new_arr as *mut u8 != old_ptr {
        let resolved = if unsafe { (*(old_ptr as *const GcHeader)).is_forwarded() } {
            unsafe { (*(old_ptr as *const GcHeader)).forwarding_addr() }
        } else {
            old_ptr
        };
        if resolved != new_arr as *mut u8 {
            vm.update_heap_reference(resolved, new_arr as *mut u8);
        }
    }
    new_arr as *mut u8
}

/// Outcome of one B1b index-search step.
pub(crate) enum SearchStepOut {
    /// Search complete with the result (index Smi or -1; includes boolean).
    Done(Value),
    /// A JS element getter was pushed (op carries awaiting_element);
    /// the caller must bail (storing the op first where needed).
    Wait,
    /// A builtin element getter failed; route the error value
    /// (setup: pending_exception; Return arm: handle_throw).
    SyncErr(Value),
}

/// Strict Equality Comparison (§7.2.15 lite): numbers compare numerically
/// (+0 == -0, NaN != NaN), strings by content, booleans/null/undefined by
/// identity, objects (incl. symbols/functions) by identity. Cross-type is
/// always false.
pub(crate) fn strict_equal(a: Value, b: Value) -> bool {
    let a_num = a.as_smi().map(|v| v as f64).or_else(|| a.as_float64());
    let b_num = b.as_smi().map(|v| v as f64).or_else(|| b.as_float64());
    match (a_num, b_num) {
        (Some(x), Some(y)) => x == y,
        (None, None) => {
            if a.is_undefined() && b.is_undefined() {
                return true;
            }
            if a.is_null() && b.is_null() {
                return true;
            }
            if let (Some(x), Some(y)) = (a.to_boolean(), b.to_boolean()) {
                return x == y;
            }
            if a.is_symbol() && b.is_symbol() {
                return a.as_symbol_id() == b.as_symbol_id();
            }
            match (a.heap_ptr(), b.heap_ptr()) {
                (Some(ap), Some(bp)) => {
                    if ap == bp {
                        return true;
                    }
                    let at = unsafe { (*(ap as *const GcHeader)).tag() };
                    let bt = unsafe { (*(bp as *const GcHeader)).tag() };
                    if at == TAG_STRING && bt == TAG_STRING {
                        let sa = unsafe { HeapString::to_string(ap as *mut HeapString) };
                        let sb = unsafe { HeapString::to_string(bp as *mut HeapString) };
                        return sa == sb;
                    }
                    false
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// One resumable step of indexOf/lastIndexOf/includes (B1b). Walks
/// op.index in the kind's direction over [0, op.length), resolving each
/// element (getters dispatch via array_element_value, which may push a
/// frame and report Wait). `injected` carries an already-resolved
/// (index, value) from a getter resume (skips re-resolution, so getter
/// side effects run exactly once). Bounds are fixed at setup.
pub(crate) fn array_search_step(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    op: &mut ArrayOpState,
    injected: Option<(usize, Value)>,
) -> SearchStepOut {
    let backward = op.kind == ArrayOpKind::LastIndexOf;
    let same_zero = op.kind == ArrayOpKind::Includes;
    let search = op.accumulator.unwrap_or(Value::undefined());
    // Found-value constructor per kind.
    let found = |i: usize| -> Value {
        match op.kind {
            ArrayOpKind::Includes => Value::boolean(true),
            _ => Value::smi(i as i32),
        }
    };
    let not_found = || -> Value {
        match op.kind {
            ArrayOpKind::Includes => Value::boolean(false),
            _ => Value::smi(-1),
        }
    };
    if let Some((i, v)) = injected {
        if element_matches(search, v, same_zero) {
            return SearchStepOut::Done(found(i));
        }
        op.index = if backward { i.wrapping_sub(1) } else { i + 1 };
    }
    loop {
        let i = op.index;
        let in_range = if backward {
            (i as i64) >= 0 && (i as i64) < op.length as i64
        } else {
            i < op.length as usize
        };
        if !in_range {
            return SearchStepOut::Done(not_found());
        }
        match array_element_value(vm, gc, op.source_val, i) {
            ArrayElemOut::Ready(v) => {
                if element_matches(search, v, same_zero) {
                    return SearchStepOut::Done(found(i));
                }
                op.index = if backward { i.wrapping_sub(1) } else { i + 1 };
            }
            ArrayElemOut::Wait => {
                op.awaiting_element = Some(i);
                return SearchStepOut::Wait;
            }
            ArrayElemOut::SyncErr(e) => return SearchStepOut::SyncErr(e),
        }
    }
}

/// Single-element comparison for the search step: strict equality, or
/// SameValueZero for includes (NaN matches NaN).
fn element_matches(search: Value, v: Value, same_zero: bool) -> bool {
    if strict_equal(search, v) {
        return true;
    }
    if same_zero {
        if let (Some(a), Some(b)) = (
            search
                .as_smi()
                .map(|x| x as f64)
                .or_else(|| search.as_float64()),
            v.as_smi().map(|x| x as f64).or_else(|| v.as_float64()),
        ) {
            if a.is_nan() && b.is_nan() {
                return true;
            }
        }
    }
    false
}

/// Lexical operation codes for the JIT callout helper.
/// Must stay in sync with the LEX_* constants in
/// crates/rune_jit_baseline/src/codegen_aarch64.rs.
const LEX_BLOCK_ENTER: u64 = 0;
const LEX_BLOCK_LEAVE: u64 = 1;
const LEX_DECLARE_LET: u64 = 2;
const LEX_DECLARE_CONST: u64 = 3;
const LEX_LOAD: u64 = 4;
const LEX_STORE: u64 = 5;
const LEX_LOAD_THIS: u64 = 6;
const LEX_COPY_LEXICAL: u64 = 7;
const LEX_MAKE_ENV: u64 = 8;
const LEX_RESTORE_ENV: u64 = 9;
const LEX_LOAD_CAPTURED: u64 = 10;
const LEX_STORE_CAPTURED: u64 = 11;

/// JIT callout for all lexical-scope operations.
/// Called from JIT-compiled code via the `lexical_helper` function pointer
/// stored in `Vm::jit_helpers`.
/// Operates on the top frame — the JIT entry paths (tier-up, call-IC) push a
/// callee Frame whenever `func_prog.needs_frame()` is true, so the top frame
/// here is the executing function's frame.
/// Returns 0 for most ops; returns the loaded Value for LEX_LOAD and
/// LEX_LOAD_CAPTURED.
/// # Safety
/// `vm_ptr` must be a valid pointer to a `Vm`. `gc_ptr` must be a valid
/// pointer to a `SemiSpace` (used only by LEX_MAKE_ENV).
#[unsafe(no_mangle)]
pub extern "C" fn rune_jit_lexical_helper(
    vm_ptr: *mut u8,
    op: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    gc_ptr: *mut u8,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let fi = vm.frames.len() - 1;
    let f = &mut vm.frames[fi];
    match op {
        LEX_BLOCK_ENTER => {
            let count = arg1 as usize;
            // Record the boundary BEFORE extending, so BlockLeave truncates
            // exactly this block's slots (mirrors the interpreter handler).
            f.scope_boundaries.push(f.lexical_slots.len());
            f.lexical_slots
                .extend(std::iter::repeat_n(Value::undefined(), count));
            f.lexical_tdz.extend(std::iter::repeat_n(true, count));
            f.lexical_const.extend(std::iter::repeat_n(false, count));
            0
        }
        LEX_BLOCK_LEAVE => {
            let boundary = f.scope_boundaries.pop().unwrap_or(0);
            f.lexical_slots.truncate(boundary);
            f.lexical_tdz.truncate(boundary);
            f.lexical_const.truncate(boundary);
            0
        }
        LEX_DECLARE_LET => {
            let slot = arg1 as usize;
            let val = Value::from_raw(arg2);
            if slot < f.lexical_slots.len() {
                f.lexical_slots[slot] = val;
                f.lexical_tdz[slot] = false;
            }
            0
        }
        LEX_DECLARE_CONST => {
            let slot = arg1 as usize;
            let val = Value::from_raw(arg2);
            if slot < f.lexical_slots.len() {
                f.lexical_slots[slot] = val;
                f.lexical_tdz[slot] = false;
                f.lexical_const[slot] = true;
            }
            0
        }
        LEX_LOAD => {
            let slot = arg1 as usize;
            if slot < f.lexical_slots.len() {
                if f.lexical_tdz[slot] {
                    return Value::undefined().raw();
                }
                return f.lexical_slots[slot].raw();
            }
            Value::undefined().raw()
        }
        LEX_STORE => {
            let slot = arg1 as usize;
            let val = Value::from_raw(arg2);
            if slot < f.lexical_slots.len() && !f.lexical_const[slot] {
                f.lexical_slots[slot] = val;
            }
            val.raw()
        }
        LEX_LOAD_THIS => f.this.raw(),
        LEX_COPY_LEXICAL => {
            let src_slot = arg1 as usize;
            let dst_slot = arg2 as usize;
            let val = if src_slot < f.lexical_slots.len() {
                f.lexical_slots[src_slot]
            } else {
                Value::undefined()
            };
            if dst_slot >= f.lexical_slots.len() {
                f.lexical_slots.resize(dst_slot + 1, Value::undefined());
                f.lexical_tdz.resize(dst_slot + 1, false);
                f.lexical_const.resize(dst_slot + 1, false);
            }
            f.lexical_slots[dst_slot] = val;
            f.lexical_tdz[dst_slot] = false;
            0
        }
        LEX_MAKE_ENV => {
            let count = arg1 as usize;
            let parent = f.env as *mut EnvObject;
            let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
            let new_env = EnvObject::allocate(gc, count, parent);
            // The allocation may have triggered a GC; resolve forwarding and
            // re-read the parent from the (possibly updated) root.
            unsafe {
                let resolved = if (*(new_env as *const GcHeader)).is_forwarded() {
                    (*(new_env as *const GcHeader)).forwarding_addr() as *mut EnvObject
                } else {
                    new_env
                };
                EnvObject::set_parent(resolved, f.env as *mut EnvObject);
                f.env = resolved as *mut u8;
            }
            0
        }
        LEX_RESTORE_ENV => {
            let env = f.env as *mut EnvObject;
            if !env.is_null() {
                let parent = unsafe { EnvObject::parent(env) };
                f.env = parent as *mut u8;
            }
            0
        }
        LEX_LOAD_CAPTURED => {
            let depth = arg1 as usize;
            let slot = arg2 as usize;
            let env = f.env as *mut EnvObject;
            let target = unsafe { EnvObject::ancestor(env, depth) };
            if target.is_null() {
                return Value::undefined().raw();
            }
            unsafe { EnvObject::get_slot(target, slot).raw() }
        }
        LEX_STORE_CAPTURED => {
            let depth = arg1 as usize;
            let slot = arg2 as usize;
            let val = Value::from_raw(arg3);
            let env = f.env as *mut EnvObject;
            let target = unsafe { EnvObject::ancestor(env, depth) };
            if !target.is_null() {
                unsafe { EnvObject::set_slot(target, slot, val) };
            }
            0
        }
        _ => 0,
    }
}

/// Validate that a bailout snapshot matches the recorded stack depth for the
/// bailout point at `bc_pc` (bailout_design.md §10.4).
///
/// Catches codegen off-by-ones loudly instead of silently corrupting the
/// interpreter stack. Kept enabled in release builds — one lookup + compare
/// per bailout.
#[cfg(feature = "jit")]
fn validate_bailout_snapshot(
    tables: Option<&rune_jit_baseline::BailoutTable>,
    bc_pc: usize,
    snapshot_len: usize,
    site: &str,
) {
    if let Some(table) = tables {
        if let Some(point) = table.points.iter().find(|p| p.bc_pc == bc_pc) {
            assert_eq!(
                snapshot_len, point.stack_depth as usize,
                "bailout stack-depth mismatch ({site}) at bc_pc {bc_pc}: snapshot {snapshot_len} != recorded {}\npoints: {:#?}",
                point.stack_depth, table.points
            );
        }
    }
}

/// Bailout helper called from JIT code when a guard fails.
///
/// Snapshots the JIT value stack (FRAME-RELATIVE: from the frame base `fb`
/// passed in x3 by convention-v2 emitted code, falling back to
/// `jit_stack_base` when null) and records the bailout PC so the `vm.rs`
/// call site can materialise interpreter state after the JIT function
/// returns.
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a `Vm`. `jit_sp` must point into
/// the JIT value stack region owned by the frame (`fb..jit_sp`).
#[cfg(feature = "jit")]
pub extern "C" fn rune_jit_bailout_helper(
    vm_ptr: *mut u8,
    bc_pc: usize,
    jit_sp: *mut u64,
    fb: *mut u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    vm.jit_bailout_count += 1;
    let base = if fb.is_null() {
        vm.jit_stack_base as usize
    } else {
        fb as usize
    };
    let current = jit_sp as usize;
    let count = if current >= base {
        (current - base) / 8
    } else {
        0
    };
    let base_ptr = base as *const u64;
    let mut snapshot = Vec::with_capacity(count);
    for i in 0..count {
        snapshot.push(unsafe { *base_ptr.add(i) });
    }
    vm.jit_bailout = JitBailoutState {
        bc_pc,
        pending: true,
        stack_snapshot: snapshot,
        reason: rune_jit_baseline::BailoutReason::BailOnEntry,
    };
    0
}

/// JIT callout for float64 addition promotion.
///
/// Called when Smi addition would overflow (or inputs are not both Smi).
/// Converts both operands to f64 via `to_number`, adds them, and returns
/// the resulting Value (Smi if the result fits in i31, otherwise HeapFloat64).
///
/// # Safety
///
/// `gc_ptr` must be a valid pointer to a `SemiSpace`. `a_raw`/`b_raw` are raw
/// Value u64s.
pub extern "C" fn rune_jit_float64_add_helper(
    vm_ptr: *mut u8,
    gc_ptr: *mut u8,
    a_raw: u64,
    b_raw: u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
    let a = Value::from_raw(a_raw);
    let b = Value::from_raw(b_raw);
    // §12.8.5 Add semantics (mirror of the interpreter's Opcode::Add arm,
    // sync-only): promote objects to strings, then either-string → concat,
    // else numeric addition.
    let a2 = if a.is_heap_object() {
        let s = crate::builtins::to_primitive_string_sync(a, gc, vm);
        Value::from_heap_ptr(heap_string(gc, &s))
    } else {
        a
    };
    let b2 = if b.is_heap_object() {
        let s = crate::builtins::to_primitive_string_sync(b, gc, vm);
        Value::from_heap_ptr(heap_string(gc, &s))
    } else {
        b
    };
    if value_is_string(a2) || value_is_string(b2) {
        // §7.1.12.1: ToString(Symbol) throws TypeError
        if a.is_symbol() || b.is_symbol() {
            let err = crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Cannot convert a Symbol value to a string",
            );
            vm.set_pending_exception(err);
            return Value::undefined().raw();
        }
        let sa = value_to_debug_string(a2);
        let sb = value_to_debug_string(b2);
        let ptr = HeapString::allocate(gc, &(sa + &sb));
        return Value::from_heap_ptr(ptr as *mut u8).raw();
    }
    let av = to_number(a);
    let bv = to_number(b);
    number_result(gc, av + bv).raw()
}

/// JIT callout for JIT-to-JIT function calls (Phase E T1 + T3).
///
/// Called from JIT-compiled `Call` opcode. Reads the call operands from the
/// JIT stack, sets up the callee's locals buffer, pushes a Frame for the
/// callee (T3), and BLRs to the callee's JIT entry point. Returns the
/// callee's return value.
///
/// The pushed Frame enables correct lexical-scope access and `this` binding
/// inside the JIT-compiled callee. On success the Frame is popped; on bailout
/// it is also popped (the existing bailout-flag mechanism in the caller's
/// codegen then propagates the failure to the interpreter, which re-executes
/// the Call from scratch).
///
/// For non-JIT callees or other incompatibilities, sets `jit_stack[63]` to 1
/// as a bailout flag — the JIT codegen checks this after BLR and exits.
///
/// # Arguments
/// - `vm_ptr`: pointer to the Vm (x0)
/// - `gc_ptr`: pointer to the GC SemiSpace (x1)
/// - `argc`: number of arguments (x2)
/// - `bc_idx`: bytecode PC of the Call opcode (x3)
/// - `args_ptr`: pointer to `arg_{argc-1}` on the JIT stack (x4)
/// - `fb`: caller's JIT frame base (x5, convention v2) — used for
///   frame-relative snapshots in the fallback bailout path.
///
/// # Safety
/// All pointers must be valid and the JIT stack must be in the pre-Call state.
#[cfg(feature = "jit")]
pub unsafe extern "C" fn rune_jit_call_helper(
    vm_ptr: *mut u8,
    gc_ptr: *mut u8,
    argc: u64,
    bc_idx: u64,
    args_ptr: *mut u64,
    fb: *mut u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
    let _ = gc;

    let argc_usize = argc as usize;

    // JIT stack layout (bottom to top):
    //   ..., this, callee, arg0, arg1, ..., arg_{argc-1}
    //   args_ptr points to the slot after arg_{argc-1} (= current JIT SP)
    //
    // Offsets from args_ptr:
    //   args_ptr[-1] = arg_{argc-1}
    //   args_ptr[-argc] = arg0
    //   args_ptr[-(argc+1)] = callee
    //   args_ptr[-(argc+2)] = this

    let callee_raw = unsafe { *args_ptr.sub(argc_usize + 1) };
    let callee = Value::from_raw(callee_raw);

    if let Some(ptr) = callee.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_FUNC {
            let func_ptr = ptr as *mut Func;
            let jit_entry = unsafe { Func::jit_entry(func_ptr) };
            if !jit_entry.is_null() {
                let creator_prog =
                    unsafe { &*(Func::prog_ptr(func_ptr) as *const BytecodeProgram) };
                let func_idx = unsafe { Func::func_index(func_ptr) } as usize;

                if func_idx < creator_prog.functions.len() {
                    let func_prog = &creator_prog.functions[func_idx];

                    // Read this and args from JIT stack directly into
                    // jit_locals_buffer — avoids a per-call Vec allocation.
                    let this_raw = unsafe { *args_ptr.sub(argc_usize + 2) };

                    vm.jit_locals_buffer.clear();
                    if func_prog.named_function {
                        vm.jit_locals_buffer.push(callee);
                    }
                    for i in 0..argc_usize {
                        let raw = unsafe { *args_ptr.sub(argc_usize - i) };
                        vm.jit_locals_buffer.push(Value::from_raw(raw));
                    }
                    let local_count = func_prog.local_names.len();
                    while vm.jit_locals_buffer.len() < local_count {
                        vm.jit_locals_buffer.push(Value::undefined());
                    }

                    // Universal Frame convention (call-IC fix, #4d/#4e):
                    // EVERY native callee gets a real Frame. This fixes two
                    // classes at once:
                    //  1. jit_locals_buffer aliasing — a frameless leaf held
                    //     its locals in the shared Vm buffer (or a scratch
                    //     Vec), unrooted across GC moves and across nested
                    //     take()/refill cycles; misboxed locals then tripped
                    //     spurious guard cascades (the fib recursion panic).
                    //  2. Bailout resumability — every frame in a native
                    //     chain owns rooted locals, so the interpreter can
                    //     resume any of them after a propagated bailout.
                    // The pc is stamped with the CALLER's call-site index so
                    // that if this frame is resumed interpreted after a
                    // deeper unit's bailout propagated through it, the
                    // eventual Return's `pc += 1` lands just past that call.
                    // (Direct bailouts overwrite pc with bailout_bc_pc.)
                    let func_env = unsafe { Func::env_ptr(func_ptr) };
                    let callee_locals = std::mem::take(&mut vm.jit_locals_buffer);
                    let frame_fi = vm.frames.len();
                    vm.frames.push(Frame {
                        locals: callee_locals,
                        lexical_slots: Vec::new(),
                        lexical_tdz: Vec::new(),
                        lexical_const: Vec::new(),
                        scope_boundaries: Vec::new(),
                        passed_argc: argc_usize,
                        pc: bc_idx as usize,
                        stack_base: vm.stack.len(),
                        prog: func_prog as *const BytecodeProgram,
                        generator_id: None,
                        this: Value::from_raw(this_raw),
                        is_constructor_call: false,
                        constructed_object: Value::undefined(),
                        env: func_env,
                        func_ptr: func_ptr as *mut u8,
                        private_name_ids: std::ptr::null_mut(),
                    });
                    let locals_ptr = vm.frames[frame_fi].locals.as_mut_ptr() as *mut u64;

                    // Call JIT entry
                    vm.jit_entry_count += 1;
                    let func: JitEntryFn = unsafe { std::mem::transmute(jit_entry) };
                    vm.jit_bailout.pending = false;
                    JIT_CALL_DEPTH.with(|d| d.set(d.get() + 1));
                    let result_raw = unsafe { func(vm_ptr, gc_ptr, locals_ptr) };
                    JIT_CALL_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));

                    // Pending bailout — UNWIND-TO-OUTERMOST (Phase-E-T2
                    // semantics, made sound by frame-relative windows).
                    //
                    // An intermediate native frame CANNOT resume
                    // interpreted at its stamped call-site: its operand
                    // window lives in its JIT-stack region, which is
                    // released on exit — resuming it pops garbage from
                    // vm.stack (the fib(19+) NaN class). Instead, each
                    // helper level REPLACES the pending record with its own
                    // (call-site, caller-window) so the OUTERMOST record
                    // survives, pops the callee frame (it will not resume),
                    // and lets the flag propagate. The direct-site caller
                    // then resumes the outermost native frame at ITS call
                    // site with the full pre-call window pushed — exactly
                    // the state the Call opcode expects.
                    if vm.jit_bailout.pending {
                        vm.frames.pop();
                        let base = if fb.is_null() {
                            vm.jit_stack_base as usize
                        } else {
                            fb as usize
                        };
                        let current = args_ptr as usize;
                        let count = if current >= base {
                            (current - base) / 8
                        } else {
                            0
                        };
                        let base_ptr = base as *const u64;
                        let mut snapshot = Vec::with_capacity(count);
                        for i in 0..count {
                            snapshot.push(unsafe { *base_ptr.add(i) });
                        }
                        vm.jit_bailout = JitBailoutState {
                            bc_pc: bc_idx as usize,
                            pending: true,
                            stack_snapshot: snapshot,
                            reason: rune_jit_baseline::BailoutReason::BailOnEntry,
                        };
                        unsafe {
                            let flag_ptr =
                                vm_ptr.add(rune_jit_baseline::JIT_FLAG_OFFSET as usize) as *mut u64;
                            *flag_ptr = 1;
                        }
                        return result_raw;
                    }

                    // Normal completion: pop the callee frame.
                    vm.frames.pop();
                    return result_raw;
                }
            }
        }
    }

    // Callee not JIT-compiled or not a function: set bailout flag.
    // The JIT codegen checks this flag after BLR and exits via bailout_helper.
    unsafe {
        let flag_ptr = vm_ptr.add(rune_jit_baseline::JIT_FLAG_OFFSET as usize) as *mut u64;
        *flag_ptr = 1;
    }
    // Record bailout state for the interpreter. FRAME-RELATIVE snapshot:
    // from the caller's frame base (convention v2) so its length equals the
    // caller model's recorded pre-Call depth even when ancestors are live
    // below this frame's region.
    let base = if fb.is_null() {
        vm.jit_stack_base as usize
    } else {
        fb as usize
    };
    let current = args_ptr as usize;
    let count = if current >= base {
        (current - base) / 8
    } else {
        0
    };
    let base_ptr = base as *const u64;
    let mut snapshot = Vec::with_capacity(count);
    for i in 0..count {
        snapshot.push(unsafe { *base_ptr.add(i) });
    }
    vm.jit_bailout = JitBailoutState {
        bc_pc: bc_idx as usize,
        pending: true,
        stack_snapshot: snapshot,
        reason: rune_jit_baseline::BailoutReason::BailOnEntry,
    };
    0
}

thread_local! {
    /// Depth of currently-executing JIT frames that may be using
    /// `jit_locals_buffer`. When >0, a new non-frame callee must NOT alias
    /// the buffer (the outer native frame is still reading it) — it takes
    /// the Frame path instead so the buffer is moved out safely.
    static JIT_CALL_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Indices into Vm::typeof_strings for each typeof result.
const TYPEOF_NUMBER: usize = 0;
const TYPEOF_STRING: usize = 1;
const TYPEOF_BOOLEAN: usize = 2;
const TYPEOF_UNDEFINED: usize = 3;
const TYPEOF_OBJECT: usize = 4;
const TYPEOF_FUNCTION: usize = 5;
const TYPEOF_SYMBOL: usize = 6;

/// JIT callout for `typeof` operator.
///
/// Takes a raw Value, returns the pre-allocated string Value corresponding
/// to the ECMAScript `typeof` result. Reads from `Vm::typeof_strings`.
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a `Vm`. `value_raw` is a raw Value u64.
pub extern "C" fn rune_jit_typeof_helper(vm_ptr: *mut u8, value_raw: u64) -> u64 {
    let vm = unsafe { &*(vm_ptr as *mut Vm) };
    let val = Value::from_raw(value_raw);
    let idx = if val.is_undefined() {
        TYPEOF_UNDEFINED
    } else if val.is_null() {
        TYPEOF_OBJECT
    } else if val.is_boolean() {
        TYPEOF_BOOLEAN
    } else if val.is_smi() {
        // Negative Smis are builtin handles — they are callable.
        if val.as_smi().unwrap() < 0 {
            TYPEOF_FUNCTION
        } else {
            TYPEOF_NUMBER
        }
    } else if val.is_symbol() {
        TYPEOF_SYMBOL
    } else if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        match tag {
            TAG_STRING => TYPEOF_STRING,
            TAG_FUNC => TYPEOF_FUNCTION,
            TAG_FLOAT64 => TYPEOF_NUMBER,
            TAG_OBJECT => {
                if vm
                    .callable_wrappers
                    .iter()
                    .any(|w| w.heap_ptr() == Some(ptr))
                {
                    TYPEOF_FUNCTION
                } else {
                    TYPEOF_OBJECT
                }
            }
            _ => TYPEOF_OBJECT,
        }
    } else {
        TYPEOF_NUMBER
    };
    vm.typeof_strings[idx].raw()
}

/// JIT callout for `LoadStringConst`.
///
/// Looks up the pre-allocated string handle from `Vm::string_cache[prog_ptr][idx]`.
/// If the cache entry is cold (interpreter hasn't seen this string yet), allocates
/// it via the GC and caches it.
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a `Vm`. `gc_ptr` must be a valid pointer
/// to a `SemiSpace`. `prog_ptr` must point to a live `BytecodeProgram`.
pub extern "C" fn rune_jit_string_helper(
    vm_ptr: *mut u8,
    gc_ptr: *mut u8,
    prog_ptr: *const u8,
    string_idx: usize,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
    let cache_key = prog_ptr as usize;
    let handles = vm.string_cache.entry(cache_key).or_insert_with(Vec::new);
    if string_idx >= handles.len() {
        handles.resize(string_idx + 1, Value::undefined());
    }
    let val = &mut handles[string_idx];
    if val.is_undefined() {
        let prog = unsafe { &*(prog_ptr as *const rune_bytecode::opcode::BytecodeProgram) };
        let s = prog
            .string_pool
            .get(string_idx)
            .map(|s| s.as_str())
            .unwrap_or("");
        let ptr = rune_core::string::HeapString::allocate(gc, s);
        *val = Value::from_heap_ptr(ptr as *mut u8);
    }
    val.raw()
}

/// JIT callout for LoadGlobal, StoreGlobal, IncGlobal, DecGlobal.
///
/// # Safety
///
/// `vm_ptr` must be a valid Vm pointer. `gc_ptr` must be a valid SemiSpace.
/// `prog_ptr` must point to a live BytecodeProgram.
pub extern "C" fn rune_jit_global_helper(
    vm_ptr: *mut u8,
    gc_ptr: *mut u8,
    prog_ptr: *const u8,
    op: u64,
    name_idx: u64,
    value_raw: u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
    let prog = unsafe { &*(prog_ptr as *const rune_bytecode::opcode::BytecodeProgram) };
    let name = prog
        .string_pool
        .get(name_idx as usize)
        .map(|s| s.as_str())
        .unwrap_or("");

    match op {
        0 => {
            // LoadGlobal
            let val = vm
                .globals
                .get(name)
                .copied()
                .or_else(|| vm.builtin_wrappers.get(name).copied())
                .or_else(|| vm.get_builtin(name))
                .unwrap_or(Value::undefined());
            val.raw()
        }
        1 => {
            // StoreGlobal
            let val = Value::from_raw(value_raw);
            vm.globals.insert(name.to_string(), val);
            val.raw()
        }
        2 | 3 => {
            // IncGlobal (2) or DecGlobal (3)
            let old_val = vm
                .globals
                .get(name)
                .copied()
                .or_else(|| vm.builtin_wrappers.get(name).copied())
                .or_else(|| vm.get_builtin(name))
                .unwrap_or(Value::undefined());
            let is_prefix = value_raw != 0;
            let n = if op == 2 {
                to_number(old_val) + 1.0
            } else {
                to_number(old_val) - 1.0
            };
            let new_val = number_result(gc, n);
            vm.globals.insert(name.to_string(), new_val);
            let result = if is_prefix { new_val } else { old_val };
            result.raw()
        }
        _ => Value::undefined().raw(),
    }
}

/// JIT helper for float binary ops (J1/J2). One entry point, op id selects:
/// 0 = Div (`a / b`), 1 = Exp (`a ** b`), 2 = Sub, 3 = Mul, 4 = Mod (fmod),
/// 5..8 = Lt/Gt/Le/Ge (boolean-encoded result). Mirrors the
/// float64_add_helper calling convention; allocation-free for numerics.
/// x0=vm_ptr, x1=gc_ptr, x2=op, x3=a_raw, x4=b_raw → x0=result raw.
pub extern "C" fn rune_jit_float64_div_exp_helper(
    vm_ptr: *mut u8,
    gc_ptr: *mut u8,
    op: u64,
    a_raw: u64,
    b_raw: u64,
) -> u64 {
    let _ = vm_ptr;
    let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
    let a = Value::from_raw(a_raw);
    let b = Value::from_raw(b_raw);
    let av = to_number(a);
    let bv = to_number(b);
    match op {
        0 => number_result(gc, av / bv).raw(),
        1 => number_result(gc, av.powf(bv)).raw(),
        2 => number_result(gc, av - bv).raw(),
        3 => number_result(gc, av * bv).raw(),
        4 => number_result(gc, av % bv).raw(),
        5 => Value::boolean(av < bv).raw(),
        6 => Value::boolean(av > bv).raw(),
        7 => Value::boolean(av <= bv).raw(),
        8 => Value::boolean(av >= bv).raw(),
        _ => Value::from_float64(f64::NAN).raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_bytecode::opcode::{BytecodeProgram, Instruction};

    fn run(prog: &BytecodeProgram) -> Result<Value, Value> {
        let mut gc = SemiSpace::new();
        let mut vm = Vm::new();
        vm.execute(&mut gc, prog)
    }

    fn run_ok(prog: &BytecodeProgram) -> Value {
        run(prog).unwrap()
    }

    macro_rules! prog {
        ($($op:expr),* $(,)?) => {
            BytecodeProgram::new(
                vec![$(Instruction::new($op, vec![])),*],
                vec![],
                vec![],
            )
        };
    }

    #[test]
    fn test_load_smi() {
        let p = BytecodeProgram::new(
            vec![Instruction::new(Opcode::LoadSmi, vec![42])],
            vec![],
            vec![],
        );
        let v = run_ok(&p);
        assert_eq!(v.as_smi(), Some(42));
    }

    #[test]
    fn test_load_undefined() {
        let p = prog![Opcode::LoadUndefined];
        assert!(run_ok(&p).is_undefined());
    }

    #[test]
    fn test_load_null() {
        let p = prog![Opcode::LoadNull];
        assert!(run_ok(&p).is_null());
    }

    #[test]
    fn test_load_boolean_true() {
        let p = BytecodeProgram::new(
            vec![Instruction::new(Opcode::LoadBoolean, vec![1])],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_load_boolean_false() {
        let p = BytecodeProgram::new(
            vec![Instruction::new(Opcode::LoadBoolean, vec![0])],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(false));
    }

    #[test]
    fn test_add_smi() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadSmi, vec![20]),
                Instruction::new(Opcode::Add, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(30));
    }

    #[test]
    fn test_sub() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![20]),
                Instruction::new(Opcode::LoadSmi, vec![5]),
                Instruction::new(Opcode::Sub, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(15));
    }

    #[test]
    fn test_mul() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![6]),
                Instruction::new(Opcode::LoadSmi, vec![7]),
                Instruction::new(Opcode::Mul, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(42));
    }

    #[test]
    fn test_div() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadSmi, vec![3]),
                Instruction::new(Opcode::Div, vec![]),
            ],
            vec![],
            vec![],
        );
        let v = run_ok(&p);
        assert!(v.is_float64(), "10/3 should be a float");
        assert!((v.as_float64().unwrap() - 3.3333333333333335).abs() < 1e-10);
    }

    #[test]
    fn test_mod() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadSmi, vec![3]),
                Instruction::new(Opcode::Mod, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_neg() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Neg, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(-42));
    }

    #[test]
    fn test_not() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::Not, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_bitnot() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::BitNot, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(!42));
    }

    #[test]
    fn test_void() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![99]),
                Instruction::new(Opcode::Void, vec![]),
            ],
            vec![],
            vec![],
        );
        assert!(run_ok(&p).is_undefined());
    }

    #[test]
    fn test_jump() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::Jump, vec![2]),    // skip to instr 2
                Instruction::new(Opcode::LoadSmi, vec![0]), // skipped
                Instruction::new(Opcode::LoadSmi, vec![1]), // target
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_jump_if_false_taken() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]), // false
                Instruction::new(Opcode::JumpIfFalse, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![0]), // skipped
                Instruction::new(Opcode::LoadSmi, vec![1]), // target
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_jump_if_true_taken() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]), // true
                Instruction::new(Opcode::JumpIfTrue, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![0]), // skipped
                Instruction::new(Opcode::LoadSmi, vec![1]), // target
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_dup_pop() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::Pop, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(42));
    }

    #[test]
    fn test_eq() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::Eq, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_neq() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![2]),
                Instruction::new(Opcode::Ne, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_lt() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![2]),
                Instruction::new(Opcode::Lt, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_bitwise() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0xFF]),
                Instruction::new(Opcode::LoadSmi, vec![0x0F]),
                Instruction::new(Opcode::BitAnd, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(0x0F));
    }

    #[test]
    fn test_shift() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![8]),
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::Shl, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(16));
    }

    #[test]
    fn test_logical_and_short_circuit() {
        // false && ... → false (short circuit, RHS not evaluated)
        // lhs, Dup, JumpIfFalse→end, Pop, rhs, end:
        // JumpIfFalse POPS and jumps if falsy; Dup preserves lhs copy for result.
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![1]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(false));
    }

    #[test]
    fn test_logical_or_short_circuit() {
        // true || ... → true (short circuit, RHS not evaluated)
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfTrue, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![0]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_logical_and_non_short_circuit() {
        // true && false → false (no short circuit, both evaluated)
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![0]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(false));
    }

    #[test]
    fn test_logical_or_non_short_circuit() {
        // false || true → true (no short circuit, both evaluated)
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfTrue, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![1]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_typeof_smi() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::TypeOf, vec![]),
            ],
            vec![],
            vec![],
        );
        let v = run_ok(&p);
        assert!(v.is_heap_object(), "typeof smi should return heap string");
    }

    #[test]
    fn test_throw_returns_error() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![99]),
                Instruction::new(Opcode::Throw, vec![]),
            ],
            vec![],
            vec![],
        );
        let result = run(&p);
        assert!(result.is_err(), "throw should return Err");
        assert_eq!(result.unwrap_err().as_smi(), Some(99));
    }
}
