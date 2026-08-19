use crate::gc::{GcHeader, SemiSpace, TAG_ARRAY_BUFFER, TAG_TYPED_ARRAY};
use crate::value::Value;
use std::sync::atomic::Ordering;

pub const ARRAY_BUFFER_SIZE: usize = 32;
pub const TYPED_ARRAY_SIZE: usize = 40;

/// The supported TypedArray element types (Table 71 minus BigInt/Float16 —
/// the engine has no BigInt yet, and Float16 is skipped for now).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TypedArrayKind {
    Int8 = 0,
    Uint8 = 1,
    Uint8Clamped = 2,
    Int16 = 3,
    Uint16 = 4,
    Int32 = 5,
    Uint32 = 6,
    Float32 = 7,
    Float64 = 8,
}

pub const NUM_KINDS: usize = 9;

impl TypedArrayKind {
    pub fn from_index(i: usize) -> TypedArrayKind {
        match i {
            0 => TypedArrayKind::Int8,
            1 => TypedArrayKind::Uint8,
            2 => TypedArrayKind::Uint8Clamped,
            3 => TypedArrayKind::Int16,
            4 => TypedArrayKind::Uint16,
            5 => TypedArrayKind::Int32,
            6 => TypedArrayKind::Uint32,
            7 => TypedArrayKind::Float32,
            _ => TypedArrayKind::Float64,
        }
    }

    /// Element Size value from Table 71.
    pub fn element_size(self) -> usize {
        match self {
            TypedArrayKind::Int8 | TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => 1,
            TypedArrayKind::Int16 | TypedArrayKind::Uint16 => 2,
            TypedArrayKind::Int32 | TypedArrayKind::Uint32 | TypedArrayKind::Float32 => 4,
            TypedArrayKind::Float64 => 8,
        }
    }

    /// The constructor name from Table 71.
    pub fn name(self) -> &'static str {
        match self {
            TypedArrayKind::Int8 => "Int8Array",
            TypedArrayKind::Uint8 => "Uint8Array",
            TypedArrayKind::Uint8Clamped => "Uint8ClampedArray",
            TypedArrayKind::Int16 => "Int16Array",
            TypedArrayKind::Uint16 => "Uint16Array",
            TypedArrayKind::Int32 => "Int32Array",
            TypedArrayKind::Uint32 => "Uint32Array",
            TypedArrayKind::Float32 => "Float32Array",
            TypedArrayKind::Float64 => "Float64Array",
        }
    }
}

/// Heap-allocated ArrayBuffer object.
/// Layout: [GcHeader(8) | data(8) | byte_length:u64(8) | prototype(8)] = 32 bytes
/// `data` points to a `Box<[u8]>` allocation OUTSIDE the semi-space (the GC
/// copies the 32-byte header only; the byte block is not traced).
#[repr(C)]
pub struct RuneArrayBuffer {
    header: GcHeader,
    data: *mut u8,
    byte_length: u64,
    prototype: *mut u8,
}

/// Heap-allocated TypedArray object (integer-indexed exotic object).
/// Layout: [GcHeader(8) | buffer(8) | byte_offset:u32(4) | length:u32(4)
///          | kind:u8(1) | pad(7) | prototype(8)] = 40 bytes
/// `buffer` (offset 8) and `prototype` (offset 32) are traced by the GC.
#[repr(C)]
pub struct RuneTypedArray {
    header: GcHeader,
    buffer: *mut u8,
    byte_offset: u32,
    length: u32,
    kind: u8,
    _pad: [u8; 7],
    prototype: *mut u8,
}

impl RuneArrayBuffer {
    pub fn allocate(gc: &mut SemiSpace, byte_length: usize, prototype: *mut u8) -> *mut u8 {
        let ptr = gc.alloc(ARRAY_BUFFER_SIZE);
        let block = vec![0u8; byte_length].into_boxed_slice();
        let data = Box::into_raw(block) as *mut u8;
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_ARRAY_BUFFER, Ordering::Relaxed);
            let b = ptr as *mut RuneArrayBuffer;
            (*b).data = data;
            (*b).byte_length = byte_length as u64;
            (*b).prototype = prototype;
        }
        ptr
    }

    pub unsafe fn data(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneArrayBuffer)).data }
    }

    pub unsafe fn byte_length(ptr: *mut u8) -> usize {
        unsafe { (*(ptr as *mut RuneArrayBuffer)).byte_length as usize }
    }

    pub unsafe fn prototype(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneArrayBuffer)).prototype }
    }

    pub unsafe fn set_prototype(ptr: *mut u8, proto: *mut u8) {
        unsafe {
            (*(ptr as *mut RuneArrayBuffer)).prototype = proto;
        }
    }

    /// Copy `src_len` bytes from `src` at `src_off` into this buffer at `dst_off`.
    /// Replace the backing byte block (used by the ArrayBuffer ctor to set
    /// the real byte length on the pre-allocated zero-length buffer).
    pub unsafe fn set_data_and_length(ptr: *mut u8, data: *mut u8, byte_length: usize) {
        unsafe {
            let b = ptr as *mut RuneArrayBuffer;
            (*b).data = data;
            (*b).byte_length = byte_length as u64;
        }
    }

    pub unsafe fn copy_from(
        ptr: *mut u8,
        dst_off: usize,
        src: *mut u8,
        src_off: usize,
        src_len: usize,
    ) {
        unsafe {
            let dst = (*(ptr as *mut RuneArrayBuffer)).data.add(dst_off);
            std::ptr::copy_nonoverlapping(src.add(src_off), dst, src_len);
        }
    }
}

impl RuneTypedArray {
    pub fn allocate(gc: &mut SemiSpace, prototype: *mut u8) -> *mut u8 {
        let ptr = gc.alloc(TYPED_ARRAY_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_TYPED_ARRAY, Ordering::Relaxed);
            let t = ptr as *mut RuneTypedArray;
            (*t).buffer = std::ptr::null_mut();
            (*t).byte_offset = 0;
            (*t).length = 0;
            (*t).kind = 0;
            (*t)._pad = [0; 7];
            (*t).prototype = prototype;
        }
        ptr
    }

    pub unsafe fn buffer(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneTypedArray)).buffer }
    }

    pub unsafe fn set_buffer(ptr: *mut u8, buffer: *mut u8) {
        unsafe {
            (*(ptr as *mut RuneTypedArray)).buffer = buffer;
        }
    }

    pub unsafe fn byte_offset(ptr: *mut u8) -> usize {
        unsafe { (*(ptr as *mut RuneTypedArray)).byte_offset as usize }
    }

    pub unsafe fn set_byte_offset(ptr: *mut u8, offset: usize) {
        unsafe {
            (*(ptr as *mut RuneTypedArray)).byte_offset = offset as u32;
        }
    }

    pub unsafe fn length(ptr: *mut u8) -> usize {
        unsafe { (*(ptr as *mut RuneTypedArray)).length as usize }
    }

    pub unsafe fn set_length(ptr: *mut u8, length: usize) {
        unsafe {
            (*(ptr as *mut RuneTypedArray)).length = length as u32;
        }
    }

    pub unsafe fn kind(ptr: *mut u8) -> TypedArrayKind {
        unsafe { TypedArrayKind::from_index((*(ptr as *mut RuneTypedArray)).kind as usize) }
    }

    pub unsafe fn set_kind(ptr: *mut u8, kind: TypedArrayKind) {
        unsafe {
            (*(ptr as *mut RuneTypedArray)).kind = kind as u8;
        }
    }

    pub unsafe fn prototype(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneTypedArray)).prototype }
    }

    /// Byte length of the viewed region: length × element size.
    pub unsafe fn byte_length(ptr: *mut u8) -> usize {
        unsafe { RuneTypedArray::length(ptr) * RuneTypedArray::kind(ptr).element_size() }
    }
}

/// §7.1.6 ToFixedSizeInteger: truncate, take mod 2^bits, wrap to signed range.
fn to_fixed_size_int(v: f64, signed: bool, bits: u32) -> i64 {
    if v.is_nan() || v.is_infinite() {
        return 0;
    }
    let m: i128 = 1i128 << bits;
    let mut fixed = (v.trunc() as i128).rem_euclid(m) as i64;
    if signed && fixed >= (1i64 << (bits - 1)) {
        fixed -= m as i64;
    }
    fixed
}

/// §7.1.13 ToUint8Clamp — clamp 0..255 with round-half-to-even.
fn to_uint8_clamp(v: f64) -> f64 {
    if v.is_nan() {
        return 0.0;
    }
    let clamped = v.clamp(0.0, 255.0);
    let f = clamped.floor();
    if clamped < f + 0.5 {
        f
    } else if clamped > f + 0.5 {
        f + 1.0
    } else if (f as i64) % 2 == 0 {
        f
    } else {
        f + 1.0
    }
}

/// The Table 71 conversion operation for a Number written into a TypedArray.
/// The result is the exact stored value (round-trip: reading it back yields
/// this number).
pub fn convert_number(kind: TypedArrayKind, v: f64) -> f64 {
    match kind {
        TypedArrayKind::Int8 => to_fixed_size_int(v, true, 8) as f64,
        TypedArrayKind::Uint8 => to_fixed_size_int(v, false, 8) as f64,
        TypedArrayKind::Uint8Clamped => to_uint8_clamp(v),
        TypedArrayKind::Int16 => to_fixed_size_int(v, true, 16) as f64,
        TypedArrayKind::Uint16 => to_fixed_size_int(v, false, 16) as f64,
        TypedArrayKind::Int32 => to_fixed_size_int(v, true, 32) as f64,
        TypedArrayKind::Uint32 => {
            let u = to_fixed_size_int(v, false, 32) as u32;
            u as f64
        }
        TypedArrayKind::Float32 => (v as f32) as f64,
        TypedArrayKind::Float64 => v,
    }
}

/// Write `number` (already converted) as the element at `index` (little-endian).
pub unsafe fn write_element(ptr: *mut u8, index: usize, number: f64) {
    unsafe {
        let kind = RuneTypedArray::kind(ptr);
        let size = kind.element_size();
        let off = RuneTypedArray::byte_offset(ptr) + index * size;
        let data = RuneArrayBuffer::data(RuneTypedArray::buffer(ptr)).add(off);
        match kind {
            TypedArrayKind::Int8 => *(data as *mut i8) = number as i8,
            TypedArrayKind::Uint8 => *data = number as u8,
            TypedArrayKind::Uint8Clamped => *data = number as u8,
            TypedArrayKind::Int16 => {
                let v = number as i16;
                *(data as *mut u16) = v as u16;
            }
            TypedArrayKind::Uint16 => *(data as *mut u16) = number as u16,
            TypedArrayKind::Int32 => {
                let v = number as i32;
                *(data as *mut u32) = v as u32;
            }
            TypedArrayKind::Uint32 => *(data as *mut u32) = number as u32,
            TypedArrayKind::Float32 => *(data as *mut f32) = number as f32,
            TypedArrayKind::Float64 => *(data as *mut f64) = number,
        }
    }
}

/// Read the element at `index` as a JS Value (integer kinds → Smi when they
/// fit in i32, else float; float kinds → float).
pub unsafe fn read_element(ptr: *mut u8, index: usize) -> Value {
    unsafe {
        let kind = RuneTypedArray::kind(ptr);
        let size = kind.element_size();
        let off = RuneTypedArray::byte_offset(ptr) + index * size;
        let data = RuneArrayBuffer::data(RuneTypedArray::buffer(ptr)).add(off);
        match kind {
            TypedArrayKind::Int8 => Value::smi(*(data as *const i8) as i32),
            TypedArrayKind::Uint8 => Value::smi(*(data as *const u8) as i32),
            TypedArrayKind::Uint8Clamped => Value::smi(*(data as *const u8) as i32),
            TypedArrayKind::Int16 => Value::smi(*(data as *const i16) as i32),
            TypedArrayKind::Uint16 => Value::smi(*(data as *const u16) as i32),
            TypedArrayKind::Int32 => Value::smi(*(data as *const i32)),
            TypedArrayKind::Uint32 => {
                let v = *(data as *const u32);
                if v <= i32::MAX as u32 {
                    Value::smi(v as i32)
                } else {
                    Value::from_float64(v as f64)
                }
            }
            TypedArrayKind::Float32 => Value::from_float64(*(data as *const f32) as f64),
            TypedArrayKind::Float64 => Value::from_float64(*(data as *const f64)),
        }
    }
}
