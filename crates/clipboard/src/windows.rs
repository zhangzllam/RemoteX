use crate::{ClipboardBackend, ClipboardError};
use remotex_protocol::MAX_CLIPBOARD_TEXT_SIZE;
use windows::Win32::{
    Foundation::{GlobalFree, HANDLE, HGLOBAL},
    System::{DataExchange, Memory},
};

const CF_UNICODETEXT: u32 = 13;

#[derive(Default)]
pub struct WindowsClipboardBackend;

struct OpenClipboardGuard;

impl OpenClipboardGuard {
    fn open() -> Result<Self, ClipboardError> {
        let mut last_error = None;
        for attempt in 0..5 {
            // SAFETY: Passing no owner window is supported. The guard closes the process-global
            // clipboard before every return path.
            match unsafe { DataExchange::OpenClipboard(None) } {
                Ok(()) => return Ok(Self),
                Err(error) => last_error = Some(error),
            }
            if attempt < 4 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        Err(ClipboardError::Platform(last_error.map_or_else(
            || "clipboard is busy".to_owned(),
            |error| error.to_string(),
        )))
    }
}

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: This guard exists only after OpenClipboard succeeds on this thread.
        let _result = unsafe { DataExchange::CloseClipboard() };
    }
}

struct LockedGlobalMemory(HGLOBAL);

impl Drop for LockedGlobalMemory {
    fn drop(&mut self) {
        // SAFETY: The handle was successfully locked by GlobalLock.
        let _result = unsafe { Memory::GlobalUnlock(self.0) };
    }
}

struct OwnedGlobalMemory {
    handle: HGLOBAL,
    transferred: bool,
}

impl Drop for OwnedGlobalMemory {
    fn drop(&mut self) {
        if !self.transferred {
            // SAFETY: Ownership remains local unless SetClipboardData succeeds.
            let _result = unsafe { GlobalFree(Some(self.handle)) };
        }
    }
}

impl ClipboardBackend for WindowsClipboardBackend {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        // The Win32 wrapper represents an unavailable format as an error. M6 treats a clipboard
        // without Unicode text as an empty plain-text clipboard.
        if unsafe { DataExchange::IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_err() {
            return Ok(String::new());
        }
        let _clipboard = OpenClipboardGuard::open()?;
        // SAFETY: The clipboard is open and the returned handle remains owned by Windows.
        let handle = unsafe { DataExchange::GetClipboardData(CF_UNICODETEXT) }
            .map_err(|error| ClipboardError::Platform(error.to_string()))?;
        let global = HGLOBAL(handle.0);
        // SAFETY: GetClipboardData(CF_UNICODETEXT) returns a global-memory handle.
        let byte_length = unsafe { Memory::GlobalSize(global) };
        let maximum_bytes = MAX_CLIPBOARD_TEXT_SIZE
            .checked_mul(2)
            .and_then(|value| value.checked_add(2))
            .ok_or(ClipboardError::TextTooLarge)?;
        if byte_length < std::mem::size_of::<u16>()
            || byte_length % std::mem::size_of::<u16>() != 0
            || byte_length > maximum_bytes
        {
            return Err(ClipboardError::TextTooLarge);
        }
        // SAFETY: The handle is valid while the clipboard is open.
        let pointer = unsafe { Memory::GlobalLock(global) };
        if pointer.is_null() {
            return Err(ClipboardError::Platform(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let _locked = LockedGlobalMemory(global);
        let unit_length = byte_length / std::mem::size_of::<u16>();
        // SAFETY: GlobalSize bounds this read and the memory remains locked for the slice lifetime.
        let units = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), unit_length) };
        let text_length = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(unit_length);
        let text =
            String::from_utf16(&units[..text_length]).map_err(|_| ClipboardError::InvalidUtf16)?;
        if text.len() > MAX_CLIPBOARD_TEXT_SIZE {
            return Err(ClipboardError::TextTooLarge);
        }
        Ok(text)
    }

    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        if text.len() > MAX_CLIPBOARD_TEXT_SIZE {
            return Err(ClipboardError::TextTooLarge);
        }
        let mut wide = text.encode_utf16().collect::<Vec<_>>();
        wide.push(0);
        let byte_length = wide
            .len()
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or(ClipboardError::TextTooLarge)?;
        // SAFETY: GHND requests movable, zero-initialized global memory for clipboard ownership.
        let handle = unsafe { Memory::GlobalAlloc(Memory::GHND, byte_length) }
            .map_err(|error| ClipboardError::Platform(error.to_string()))?;
        let mut allocation = OwnedGlobalMemory {
            handle,
            transferred: false,
        };
        // SAFETY: The allocation has byte_length writable bytes.
        let pointer = unsafe { Memory::GlobalLock(handle) };
        if pointer.is_null() {
            return Err(ClipboardError::Platform(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        // SAFETY: Both buffers are valid for wide.len() u16 elements and do not overlap.
        unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer.cast::<u16>(), wide.len()) };
        // SAFETY: GlobalLock succeeded for this allocation.
        let _result = unsafe { Memory::GlobalUnlock(handle) };

        let _clipboard = OpenClipboardGuard::open()?;
        // SAFETY: The clipboard is open. EmptyClipboard transfers ownership context to this task.
        unsafe { DataExchange::EmptyClipboard() }
            .map_err(|error| ClipboardError::Platform(error.to_string()))?;
        // SAFETY: The movable allocation contains a terminated UTF-16 string. On success Windows
        // owns it and RemoteX must not free it.
        unsafe { DataExchange::SetClipboardData(CF_UNICODETEXT, Some(HANDLE(handle.0))) }
            .map_err(|error| ClipboardError::Platform(error.to_string()))?;
        allocation.transferred = true;
        Ok(())
    }
}
