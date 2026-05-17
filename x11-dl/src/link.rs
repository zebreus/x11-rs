// x11-rs: Rust bindings for X11 libraries
// The X11 libraries are available under the MIT license.
// These bindings are public domain.

use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_void, CStr};

#[cfg(feature = "std")]
use std::path::Path;

use super::error::{OpenError, OpenErrorKind};

include!(concat!(env!("OUT_DIR"), "/config.rs"));

//
// x11_link!
//

macro_rules! x11_link {
  { $struct_name:ident, $pkg_name:ident, [$($lib_name:expr),*], $nsyms:expr,
    $(pub fn $fn_name:ident ($($param_name:ident : $param_type:ty),*) -> $ret_type:ty,)*
    variadic:
    $(pub fn $vfn_name:ident ($($vparam_name: ident : $vparam_type:ty),+) -> $vret_type:ty,)*
    globals:
    $(pub static $var_name:ident : $var_type:ty,)*
  } => {
    #[allow(clippy::manual_non_exhaustive)]
    pub struct $struct_name {
      _private: (),
      $(pub $fn_name: unsafe extern "C" fn ($($param_type),*) -> $ret_type,)*
      $(pub $vfn_name: unsafe extern "C" fn ($($vparam_type),+, ...) -> $vret_type,)*
      $(pub $var_name: *mut $var_type,)*
    }

    unsafe impl Send for $struct_name {}
    unsafe impl Sync for $struct_name {}

    impl $struct_name {
      pub fn open () -> Result<$struct_name, $crate::error::OpenError> {
        #[cfg(feature = "std")]
        {
          /// Cached function pointers and global variables for X11 libraries.
          static CACHED: once_cell::sync::OnceCell<($crate::link::DynamicLibrary, $struct_name)> = once_cell::sync::OnceCell::new();

          // Use the cached library or open a new one.
          let (_, funcs) = CACHED.get_or_try_init(|| {
            unsafe {
              let libdir = $crate::link::config::libdir::$pkg_name;
              let lib = $crate::link::DynamicLibrary::open_multi(libdir, &[$($lib_name),*])?;

              // Load every function pointer.
              let funcs = $struct_name {
                _private: (),
                $($fn_name: ::core::mem::transmute(lib.symbol(stringify!($fn_name))?),)*
                $($vfn_name: ::core::mem::transmute(lib.symbol(stringify!($vfn_name))?),)*
                $($var_name: ::core::mem::transmute(lib.symbol(stringify!($var_name))?),)*
              };

              Ok((lib, funcs))
            }
          })?;

          Ok($struct_name {
            _private: (),
            $($fn_name: funcs.$fn_name,)*
            $($vfn_name: funcs.$vfn_name,)*
            $($var_name: funcs.$var_name,)*
          })
        }

        #[cfg(not(feature = "std"))]
        {
          use alloc::boxed::Box;
          use core::sync::atomic::{AtomicPtr, Ordering};

          /// Cached function pointers and global variables (no-std path).
          /// The pointer stored here is allocated via `Box::into_raw` and intentionally
          /// never freed — it lives for the entire process lifetime. Do not drop or
          /// replace the pointer once it has been stored.
          static CACHED: AtomicPtr<$struct_name> = AtomicPtr::new(core::ptr::null_mut());

          // Fast path: return a field-by-field copy from the already-initialised cache.
          let existing = CACHED.load(Ordering::Acquire);
          if !existing.is_null() {
            return Ok(unsafe {
              $struct_name {
                _private: (),
                $($fn_name: (*existing).$fn_name,)*
                $($vfn_name: (*existing).$vfn_name,)*
                $($var_name: (*existing).$var_name,)*
              }
            });
          }

          unsafe {
            let libdir = $crate::link::config::libdir::$pkg_name;
            let lib = $crate::link::DynamicLibrary::open_multi(libdir, &[$($lib_name),*])?;

            // Load every function pointer.
            let funcs = $struct_name {
              _private: (),
              $($fn_name: ::core::mem::transmute(lib.symbol(stringify!($fn_name))?),)*
              $($vfn_name: ::core::mem::transmute(lib.symbol(stringify!($vfn_name))?),)*
              $($var_name: ::core::mem::transmute(lib.symbol(stringify!($var_name))?),)*
            };

            // Keep the library loaded for the lifetime of the process.
            ::core::mem::forget(lib);

            let new_ptr = Box::into_raw(Box::new(funcs));
            match CACHED.compare_exchange(
              core::ptr::null_mut(),
              new_ptr,
              Ordering::AcqRel,
              Ordering::Acquire,
            ) {
              Ok(_) => Ok($struct_name {
                _private: (),
                $($fn_name: (*new_ptr).$fn_name,)*
                $($vfn_name: (*new_ptr).$vfn_name,)*
                $($var_name: (*new_ptr).$var_name,)*
              }),
              Err(winner) => {
                // Another thread raced and stored its pointer first; discard ours.
                // (The extra dlopen reference from our forgotten lib handle is
                // harmless: dlopen refcounts, and we never dlclose it.)
                drop(Box::from_raw(new_ptr));
                Ok($struct_name {
                  _private: (),
                  $($fn_name: (*winner).$fn_name,)*
                  $($vfn_name: (*winner).$vfn_name,)*
                  $($var_name: (*winner).$var_name,)*
                })
              }
            }
          }
        }
      }
    }
  };
}

//
// DynamicLibrary
//

pub struct DynamicLibrary {
    handle: *mut c_void,
}

impl DynamicLibrary {
    pub fn open(name: &str) -> Result<DynamicLibrary, OpenError> {
        unsafe {
            let cname = match CString::new(name) {
                Ok(cname) => cname,
                Err(_) => {
                    return Err(OpenError::new(
                        OpenErrorKind::Library,
                        String::from("library name contains NUL byte(s)"),
                    ));
                }
            };

            let handle = libc::dlopen(cname.as_ptr(), libc::RTLD_LAZY);

            if handle.is_null() {
                let msg = libc::dlerror();

                if msg.is_null() {
                    return Err(OpenError::new(OpenErrorKind::Library, String::new()));
                }

                let cmsg = CStr::from_ptr(msg as *const c_char);
                let detail = cmsg.to_string_lossy().into_owned();
                return Err(OpenError::new(OpenErrorKind::Library, detail));
            }

            Ok(DynamicLibrary {
                handle: handle as *mut c_void,
            })
        }
    }

    pub fn open_multi(
        libdir: Option<&'static str>,
        names: &[&str],
    ) -> Result<DynamicLibrary, OpenError> {
        assert!(!names.is_empty());

        #[cfg(feature = "std")]
        let paths: Vec<String> = libdir.map_or(Vec::new(), |dir| {
            let path = Path::new(dir);
            names
                .iter()
                .map(|name| String::from(path.join(name).to_str().unwrap()))
                .collect()
        });

        #[cfg(not(feature = "std"))]
        let paths: Vec<String> = libdir.map_or(Vec::new(), |dir| {
            names
                .iter()
                .map(|name| format!("{}/{}", dir, name))
                .collect()
        });

        let mut msgs = Vec::new();

        for name in names.iter().copied().chain(paths.iter().map(|x| &**x)) {
            match DynamicLibrary::open(name) {
                Ok(lib) => {
                    return Ok(lib);
                }
                Err(err) => {
                    msgs.push(format!("{}", err));
                }
            }
        }

        let mut detail = String::new();

        for (i, msg) in msgs.iter().enumerate() {
            if i != 0 {
                detail.push_str("; ");
            }
            detail.push_str(msg.as_ref());
        }

        Err(OpenError::new(OpenErrorKind::Library, detail))
    }

    pub fn symbol(&self, name: &str) -> Result<*mut c_void, OpenError> {
        unsafe {
            let cname = match CString::new(name) {
                Ok(cname) => cname,
                Err(_) => {
                    return Err(OpenError::new(
                        OpenErrorKind::Symbol,
                        String::from("symbol name contains NUL byte(s)"),
                    ));
                }
            };

            let sym = libc::dlsym(self.handle as *mut _, cname.as_ptr());

            if sym.is_null() {
                let msg = libc::dlerror();

                if msg.is_null() {
                    return Err(OpenError::new(OpenErrorKind::Symbol, String::from(name)));
                }

                let cmsg = CStr::from_ptr(msg as *const c_char);
                let detail = format!("{} - {}", name, cmsg.to_string_lossy().into_owned());
                return Err(OpenError::new(OpenErrorKind::Symbol, detail));
            }

            Ok(sym as *mut c_void)
        }
    }
}

impl Drop for DynamicLibrary {
  fn drop (&mut self) {
    unsafe {
      libc::dlclose(self.handle as *mut _);
    }
  }
}

unsafe impl Send for DynamicLibrary {}
unsafe impl Sync for DynamicLibrary {}
