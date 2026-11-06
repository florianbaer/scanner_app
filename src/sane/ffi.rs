//! Raw declarations mirroring `sane/sane.h`. Nothing here is safe to use directly;
//! see the wrapper in `super`.

#![allow(dead_code)]

use std::ffi::{c_char, c_int, c_void};

pub type Word = c_int;
pub type Handle = *mut c_void;

pub const CAP_SOFT_SELECT: Word = 1 << 0;
pub const CAP_INACTIVE: Word = 1 << 5;

pub const TYPE_BOOL: c_int = 0;
pub const TYPE_INT: c_int = 1;
pub const TYPE_FIXED: c_int = 2;
pub const TYPE_STRING: c_int = 3;
pub const TYPE_BUTTON: c_int = 4;
pub const TYPE_GROUP: c_int = 5;

pub const CONSTRAINT_RANGE: c_int = 1;
pub const CONSTRAINT_WORD_LIST: c_int = 2;
pub const CONSTRAINT_STRING_LIST: c_int = 3;

pub const ACTION_GET_VALUE: c_int = 0;
pub const ACTION_SET_VALUE: c_int = 1;

pub const STATUS_GOOD: c_int = 0;
pub const STATUS_INVAL: c_int = 4;
pub const STATUS_EOF: c_int = 5;
pub const STATUS_JAMMED: c_int = 6;
pub const STATUS_NO_DOCS: c_int = 7;
pub const STATUS_COVER_OPEN: c_int = 8;

pub const FRAME_GRAY: c_int = 0;
pub const FRAME_RGB: c_int = 1;

#[repr(C)]
pub struct RawDevice {
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub model: *const c_char,
    pub kind: *const c_char,
}

#[repr(C)]
pub struct RawRange {
    pub min: Word,
    pub max: Word,
    pub quant: Word,
}

#[repr(C)]
pub struct RawOptionDescriptor {
    pub name: *const c_char,
    pub title: *const c_char,
    pub desc: *const c_char,
    pub value_type: c_int,
    pub unit: c_int,
    pub size: Word,
    pub cap: Word,
    pub constraint_type: c_int,
    pub constraint: *const c_void,
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct RawParameters {
    pub format: c_int,
    pub last_frame: Word,
    pub bytes_per_line: Word,
    pub pixels_per_line: Word,
    pub lines: Word,
    pub depth: Word,
}

extern "C" {
    pub fn sane_init(version_code: *mut Word, authorize: *const c_void) -> c_int;
    pub fn sane_exit();
    pub fn sane_get_devices(device_list: *mut *const *const RawDevice, local_only: Word) -> c_int;
    pub fn sane_open(name: *const c_char, handle: *mut Handle) -> c_int;
    pub fn sane_close(handle: Handle);
    pub fn sane_get_option_descriptor(handle: Handle, option: Word) -> *const RawOptionDescriptor;
    pub fn sane_control_option(
        handle: Handle,
        option: Word,
        action: c_int,
        value: *mut c_void,
        info: *mut Word,
    ) -> c_int;
    pub fn sane_get_parameters(handle: Handle, params: *mut RawParameters) -> c_int;
    pub fn sane_start(handle: Handle) -> c_int;
    pub fn sane_read(handle: Handle, data: *mut u8, max_length: Word, length: *mut Word) -> c_int;
    pub fn sane_cancel(handle: Handle);
    pub fn sane_strstatus(status: c_int) -> *const c_char;
}
