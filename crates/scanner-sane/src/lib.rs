//! SANE backend for scanner-core. This crate is the only place in the workspace
//! with `unsafe` code: raw declarations in `ffi`, the safe wrapper here.

mod adapter;
mod backend;
mod ffi;
mod mdns;

pub use adapter::SaneScanner;
pub use backend::{SaneBackend, SaneSource};
pub use mdns::MdnsEpsonSource;

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use anyhow::{anyhow, bail};

use scanner_core::raster::{Channels, RasterFormat};

/// A non-GOOD status returned by SANE.
#[derive(Debug)]
pub struct SaneError {
    pub status: c_int,
    pub context: String,
}

impl SaneError {
    pub fn is_no_docs(&self) -> bool {
        self.status == ffi::STATUS_NO_DOCS
    }
}

impl fmt::Display for SaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: sane_strstatus returns a pointer to a static string.
        let msg = unsafe { cstr(ffi::sane_strstatus(self.status)) };
        write!(f, "{}: {}", self.context, msg)
    }
}

impl std::error::Error for SaneError {}

pub type SaneResult<T> = Result<T, SaneError>;

fn check(status: c_int, context: impl Into<String>) -> SaneResult<()> {
    if status == ffi::STATUS_GOOD {
        Ok(())
    } else {
        Err(SaneError {
            status,
            context: context.into(),
        })
    }
}

/// # Safety
/// `p` must be null or point to a valid NUL-terminated string.
unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn fix(v: f64) -> ffi::Word {
    (v * 65536.0).round() as ffi::Word
}

fn unfix(v: ffi::Word) -> f64 {
    v as f64 / 65536.0
}

// ---------------------------------------------------------------------------
// Library context
// ---------------------------------------------------------------------------

/// Keeps libsane initialised; `sane_exit` runs when the last reference is dropped.
struct Lib;

impl Drop for Lib {
    fn drop(&mut self) {
        // SAFETY: only reached once, after every Device holding an Arc<Lib> is gone.
        unsafe { ffi::sane_exit() }
    }
}

static LIB: Mutex<Weak<Lib>> = Mutex::new(Weak::new());

/// Handle to the SANE library. Cheap to clone; SANE is initialised once per process.
#[derive(Clone)]
pub struct Sane {
    lib: Arc<Lib>,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub vendor: String,
    pub model: String,
    pub kind: String,
}

impl Sane {
    pub fn init() -> SaneResult<Self> {
        let mut slot = LIB.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(lib) = slot.upgrade() {
            return Ok(Sane { lib });
        }
        let mut version: ffi::Word = 0;
        // SAFETY: no auth callback; version is a valid out-pointer.
        check(
            unsafe { ffi::sane_init(&mut version, std::ptr::null()) },
            "initialising SANE",
        )?;
        let lib = Arc::new(Lib);
        *slot = Arc::downgrade(&lib);
        Ok(Sane { lib })
    }

    /// Lists scanners SANE knows about. Network autodiscovery can take a few seconds.
    pub fn devices(&self) -> SaneResult<Vec<DeviceInfo>> {
        let mut list: *const *const ffi::RawDevice = std::ptr::null();
        // SAFETY: list is a valid out-pointer; SANE owns the returned array.
        check(
            unsafe { ffi::sane_get_devices(&mut list, 0) },
            "listing devices",
        )?;
        let mut out = Vec::new();
        if list.is_null() {
            return Ok(out);
        }
        // SAFETY: SANE returns a NULL-terminated array of valid device pointers.
        unsafe {
            let mut i = 0;
            while !(*list.add(i)).is_null() {
                let d = &**list.add(i);
                out.push(DeviceInfo {
                    name: cstr(d.name),
                    vendor: cstr(d.vendor),
                    model: cstr(d.model),
                    kind: cstr(d.kind),
                });
                i += 1;
            }
        }
        Ok(out)
    }

    pub fn open(&self, name: &str) -> SaneResult<Device> {
        let c = CString::new(name).map_err(|_| SaneError {
            status: ffi::STATUS_INVAL,
            context: format!("invalid device name {name:?}"),
        })?;
        let mut handle: ffi::Handle = std::ptr::null_mut();
        // SAFETY: c is a valid C string; handle is a valid out-pointer.
        check(
            unsafe { ffi::sane_open(c.as_ptr(), &mut handle) },
            format!("opening {name}"),
        )?;
        Ok(Device {
            handle,
            _lib: self.lib.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Bool,
    Int,
    Fixed,
    String,
    Button,
    Group,
    Unknown,
}

#[derive(Debug, Clone)]
pub enum Constraint {
    None,
    Range { min: f64, max: f64, quant: f64 },
    List(Vec<f64>),
    Strings(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct OptionInfo {
    index: ffi::Word,
    pub name: String,
    pub title: String,
    pub value_type: ValueType,
    size: ffi::Word,
    cap: ffi::Word,
    pub constraint: Constraint,
}

impl OptionInfo {
    pub fn is_active(&self) -> bool {
        self.cap & ffi::CAP_INACTIVE == 0
    }
    pub fn is_settable(&self) -> bool {
        self.cap & ffi::CAP_SOFT_SELECT != 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Fixed(f64),
    Str(String),
    None,
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Fixed(x) => Some(*x),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{}", if *b { "yes" } else { "no" }),
            Value::Int(i) => write!(f, "{i}"),
            Value::Fixed(x) => write!(f, "{}", fmt_num(*x)),
            Value::Str(s) => write!(f, "{s}"),
            Value::None => write!(f, "-"),
        }
    }
}

/// Formats SANE fixed-point numbers without float noise (215.899… → 215.9).
pub fn fmt_num(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ---------------------------------------------------------------------------
// Device
// ---------------------------------------------------------------------------

/// An open scanner. Closed on drop.
pub struct Device {
    handle: ffi::Handle,
    _lib: Arc<Lib>,
}

// SAFETY: a SANE handle may be moved between threads as long as it is not used
// concurrently; `Device` has no `&self` methods that could run in parallel safely,
// so it is Send but deliberately not Sync.
unsafe impl Send for Device {}

impl Device {
    pub fn options(&self) -> Vec<OptionInfo> {
        let mut count: ffi::Word = 0;
        let st = self.control(
            0,
            ffi::ACTION_GET_VALUE,
            (&mut count as *mut ffi::Word).cast(),
        );
        if st != ffi::STATUS_GOOD {
            return Vec::new();
        }
        (1..count).filter_map(|i| self.descriptor(i)).collect()
    }

    pub fn find(&self, name: &str) -> Option<OptionInfo> {
        self.options().into_iter().find(|o| o.name == name)
    }

    fn descriptor(&self, index: ffi::Word) -> Option<OptionInfo> {
        // SAFETY: handle is open; SANE returns null or a descriptor valid until the
        // next option change. We copy everything out immediately.
        let p = unsafe { ffi::sane_get_option_descriptor(self.handle, index) };
        if p.is_null() {
            return None;
        }
        let d = unsafe { &*p };
        let value_type = match d.value_type {
            ffi::TYPE_BOOL => ValueType::Bool,
            ffi::TYPE_INT => ValueType::Int,
            ffi::TYPE_FIXED => ValueType::Fixed,
            ffi::TYPE_STRING => ValueType::String,
            ffi::TYPE_BUTTON => ValueType::Button,
            ffi::TYPE_GROUP => ValueType::Group,
            _ => ValueType::Unknown,
        };
        let conv = |w: ffi::Word| {
            if value_type == ValueType::Fixed {
                unfix(w)
            } else {
                w as f64
            }
        };
        // SAFETY: the constraint pointer's type is given by constraint_type (sane.h).
        let constraint = unsafe {
            match d.constraint_type {
                ffi::CONSTRAINT_RANGE if !d.constraint.is_null() => {
                    let r = &*(d.constraint as *const ffi::RawRange);
                    Constraint::Range {
                        min: conv(r.min),
                        max: conv(r.max),
                        quant: conv(r.quant),
                    }
                }
                ffi::CONSTRAINT_WORD_LIST if !d.constraint.is_null() => {
                    let w = d.constraint as *const ffi::Word;
                    let n = (*w).max(0) as usize;
                    Constraint::List((1..=n).map(|i| conv(*w.add(i))).collect())
                }
                ffi::CONSTRAINT_STRING_LIST if !d.constraint.is_null() => {
                    let s = d.constraint as *const *const c_char;
                    let mut v = Vec::new();
                    let mut i = 0;
                    while !(*s.add(i)).is_null() {
                        v.push(cstr(*s.add(i)));
                        i += 1;
                    }
                    Constraint::Strings(v)
                }
                _ => Constraint::None,
            }
        };
        Some(OptionInfo {
            index,
            // SAFETY: descriptor strings are NULL or valid C strings.
            name: unsafe { cstr(d.name) },
            title: unsafe { cstr(d.title) },
            value_type,
            size: d.size,
            cap: d.cap,
            constraint,
        })
    }

    pub fn get(&self, opt: &OptionInfo) -> SaneResult<Value> {
        if !opt.is_active() {
            return Ok(Value::None);
        }
        let ctx = || format!("reading option {}", opt.name);
        match opt.value_type {
            ValueType::Bool | ValueType::Int | ValueType::Fixed => {
                // Array options are reported by their first element.
                let n = (opt.size as usize / std::mem::size_of::<ffi::Word>()).max(1);
                let mut buf = vec![0 as ffi::Word; n];
                check(
                    self.control(opt.index, ffi::ACTION_GET_VALUE, buf.as_mut_ptr().cast()),
                    ctx(),
                )?;
                Ok(match opt.value_type {
                    ValueType::Bool => Value::Bool(buf[0] != 0),
                    ValueType::Int => Value::Int(buf[0]),
                    _ => Value::Fixed(unfix(buf[0])),
                })
            }
            ValueType::String => {
                let mut buf = vec![0u8; opt.size.max(1) as usize];
                check(
                    self.control(opt.index, ffi::ACTION_GET_VALUE, buf.as_mut_ptr().cast()),
                    ctx(),
                )?;
                let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                Ok(Value::Str(
                    String::from_utf8_lossy(&buf[..end]).into_owned(),
                ))
            }
            _ => Ok(Value::None),
        }
    }

    /// Sets an option from text, interpreted according to the option's type.
    /// String-list values match case-insensitively (`color` → `Color`).
    pub fn set_str(&self, name: &str, raw: &str) -> anyhow::Result<()> {
        let opt = self
            .find(name)
            .ok_or_else(|| anyhow!("scanner has no option '{name}'"))?;
        if !opt.is_active() {
            bail!("option '{name}' is currently inactive");
        }
        if !opt.is_settable() {
            bail!("option '{name}' is read-only");
        }
        let st = match opt.value_type {
            ValueType::Bool => {
                let mut w: ffi::Word = match raw.to_ascii_lowercase().as_str() {
                    "1" | "yes" | "true" | "on" => 1,
                    "0" | "no" | "false" | "off" => 0,
                    _ => bail!("'{raw}' is not a boolean for {name} (use yes/no)"),
                };
                self.control(
                    opt.index,
                    ffi::ACTION_SET_VALUE,
                    (&mut w as *mut ffi::Word).cast(),
                )
            }
            ValueType::Int | ValueType::Fixed => {
                let x: f64 = raw
                    .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                    .parse()
                    .map_err(|_| anyhow!("'{raw}' is not a number for {name}"))?;
                let mut w = if opt.value_type == ValueType::Fixed {
                    fix(x)
                } else {
                    x.round() as ffi::Word
                };
                self.control(
                    opt.index,
                    ffi::ACTION_SET_VALUE,
                    (&mut w as *mut ffi::Word).cast(),
                )
            }
            ValueType::String => {
                let value = match &opt.constraint {
                    Constraint::Strings(list) => list
                        .iter()
                        .find(|s| s.eq_ignore_ascii_case(raw))
                        .cloned()
                        .ok_or_else(|| {
                            anyhow!(
                                "'{raw}' is not valid for {name}; choose one of: {}",
                                list.join(", ")
                            )
                        })?,
                    _ => raw.to_string(),
                };
                let mut buf = vec![0u8; (opt.size.max(0) as usize).max(value.len() + 1)];
                buf[..value.len()].copy_from_slice(value.as_bytes());
                self.control(opt.index, ffi::ACTION_SET_VALUE, buf.as_mut_ptr().cast())
            }
            ValueType::Button => {
                self.control(opt.index, ffi::ACTION_SET_VALUE, std::ptr::null_mut())
            }
            _ => bail!("option '{name}' cannot be set"),
        };
        check(st, format!("setting {name}={raw}"))?;
        Ok(())
    }

    fn control(&self, index: ffi::Word, action: c_int, value: *mut c_void) -> c_int {
        let mut info: ffi::Word = 0;
        // SAFETY: callers pass a buffer matching the option's type and size.
        unsafe { ffi::sane_control_option(self.handle, index, action, value, &mut info) }
    }

    pub fn start(&self) -> SaneResult<()> {
        // SAFETY: handle is open.
        check(unsafe { ffi::sane_start(self.handle) }, "starting scan")
    }

    /// Format of the frame currently being acquired.
    pub fn raster_format(&self) -> anyhow::Result<RasterFormat> {
        let mut p = ffi::RawParameters::default();
        // SAFETY: handle is open; p is a valid out-pointer.
        check(
            unsafe { ffi::sane_get_parameters(self.handle, &mut p) },
            "reading scan parameters",
        )?;
        let channels = match p.format {
            ffi::FRAME_GRAY => Channels::Gray,
            ffi::FRAME_RGB => Channels::Rgb,
            other => {
                bail!("unsupported SANE frame format {other} (only gray and RGB are supported)")
            }
        };
        Ok(RasterFormat {
            channels,
            depth: p.depth.max(0) as u32,
            bytes_per_line: p.bytes_per_line.max(0) as usize,
            width: p.pixels_per_line.max(0) as u32,
        })
    }

    /// Reads the current frame until EOF.
    pub fn read_frame(&self) -> SaneResult<Vec<u8>> {
        let mut data = Vec::new();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let mut len: ffi::Word = 0;
            // SAFETY: buf is valid for buf.len() bytes.
            let st = unsafe {
                ffi::sane_read(
                    self.handle,
                    buf.as_mut_ptr(),
                    buf.len() as ffi::Word,
                    &mut len,
                )
            };
            match st {
                ffi::STATUS_GOOD => data.extend_from_slice(&buf[..len.max(0) as usize]),
                ffi::STATUS_EOF => return Ok(data),
                _ => {
                    return Err(SaneError {
                        status: st,
                        context: "reading image data".into(),
                    })
                }
            }
        }
    }

    pub fn cancel(&self) {
        // SAFETY: handle is open; cancel is always allowed.
        unsafe { ffi::sane_cancel(self.handle) }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: handle is open and never used after this.
        unsafe {
            ffi::sane_cancel(self.handle);
            ffi::sane_close(self.handle);
        }
    }
}
