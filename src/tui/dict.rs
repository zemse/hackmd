//! macOS Dictionary Services lookup — the same definition data behind the
//! system "Look Up" panel, used to fill the in-TUI definition popover.
//!
//! `DCSCopyTextDefinition` is a plain C function in DictionaryServices (a
//! sub-framework of CoreServices), so this is a thin FFI over CoreFoundation
//! string types, not anything Objective-C — no `objc2` runtime is involved.
//! The CF types come from `core-foundation-sys` (a direct dep; transitively
//! also via `chrono → iana-time-zone` on macOS). On non-macOS platforms
//! [`lookup`] is a stub returning `None`.

/// Whether word lookup is available on this platform (macOS only).
#[cfg(target_os = "macos")]
pub const SUPPORTED: bool = true;
#[cfg(not(target_os = "macos"))]
pub const SUPPORTED: bool = false;

/// Look up `word` in the user's active macOS dictionaries, returning the
/// definition as plain text (headword, phonetics, senses) or `None` when the
/// word has no entry. Blocking; call it off the UI thread.
#[cfg(target_os = "macos")]
pub fn lookup(word: &str) -> Option<String> {
    use core_foundation_sys::base::{CFIndex, CFRange, CFRelease, CFTypeRef};
    use core_foundation_sys::string::{
        CFStringCreateWithBytes, CFStringGetCString, CFStringGetLength,
        CFStringGetMaximumSizeForEncoding, CFStringRef, kCFStringEncodingUTF8,
    };
    use std::os::raw::{c_char, c_void};
    use std::ptr;

    // DictionaryServices ships inside the CoreServices umbrella framework;
    // linking it resolves the symbol (verified vs `-framework CoreServices`).
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn DCSCopyTextDefinition(
            dictionary: *const c_void,
            text: CFStringRef,
            range: CFRange,
        ) -> CFStringRef;
    }

    let word = word.trim();
    if word.is_empty() {
        return None;
    }

    unsafe {
        let bytes = word.as_bytes();
        let cf = CFStringCreateWithBytes(
            ptr::null(),
            bytes.as_ptr(),
            bytes.len() as CFIndex,
            kCFStringEncodingUTF8,
            0, // not an external representation
        );
        if cf.is_null() {
            return None;
        }
        let len = CFStringGetLength(cf);
        // `NULL` dictionary → search the user's active dictionaries, like Look Up.
        let def = DCSCopyTextDefinition(
            ptr::null(),
            cf,
            CFRange {
                location: 0,
                length: len,
            },
        );
        CFRelease(cf as CFTypeRef);
        if def.is_null() {
            return None;
        }
        let def_len = CFStringGetLength(def);
        let max = CFStringGetMaximumSizeForEncoding(def_len, kCFStringEncodingUTF8) + 1;
        let mut buf = vec![0u8; max.max(1) as usize];
        let ok = CFStringGetCString(
            def,
            buf.as_mut_ptr() as *mut c_char,
            max,
            kCFStringEncodingUTF8,
        );
        CFRelease(def as CFTypeRef);
        if ok == 0 {
            return None;
        }
        let text = std::ffi::CStr::from_ptr(buf.as_ptr() as *const c_char)
            .to_string_lossy()
            .into_owned();
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

/// Non-macOS stub: no system dictionary available.
#[cfg(not(target_os = "macos"))]
pub fn lookup(_word: &str) -> Option<String> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn looks_up_a_common_word_and_rejects_gibberish() {
        // Real call through Dictionary Services — also proves the framework
        // link works. "hello" is in every default dictionary.
        let def = lookup("hello").expect("'hello' should have a definition");
        assert!(def.to_lowercase().contains("hello"));
        assert!(lookup("zzqqxxnotaword").is_none());
        assert!(lookup("   ").is_none());
    }
}
