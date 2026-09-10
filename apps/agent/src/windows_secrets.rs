//! Current-user Windows DPAPI protection for the Agent device identity.

#![allow(unsafe_code)]

use anyhow::Context;
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    },
    core::w,
};

pub fn protect(secret: &[u8]) -> anyhow::Result<String> {
    let input = blob(secret)?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points to a live byte slice. DPAPI owns `output` until it is copied and
    // released by `copy_and_free`; interactive UI is disabled.
    unsafe {
        CryptProtectData(
            &raw const input,
            w!("RemoteX device identity"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
        .context("protect device identity with Windows DPAPI")?;
    }
    copy_and_free(output).map(hex::encode)
}

pub fn unprotect(protected: &str) -> anyhow::Result<Vec<u8>> {
    let encrypted = hex::decode(protected).context("decode protected device identity")?;
    let input = blob(&encrypted)?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points to the live decoded buffer. DPAPI allocates `output` for the current
    // user; it is copied and freed before this function returns. Interactive UI is disabled.
    unsafe {
        CryptUnprotectData(
            &raw const input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
        .context("unprotect device identity with Windows DPAPI")?;
    }
    copy_and_free(output)
}

fn blob(data: &[u8]) -> anyhow::Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: data.len().try_into().context("DPAPI input is too large")?,
        pbData: data.as_ptr().cast_mut(),
    })
}

fn copy_and_free(output: CRYPT_INTEGER_BLOB) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        !output.pbData.is_null() && output.cbData != 0,
        "Windows DPAPI returned empty output"
    );
    // SAFETY: DPAPI returned `cbData` readable bytes allocated with LocalAlloc. The allocation is
    // copied before LocalFree and is not accessed afterwards.
    let bytes = unsafe {
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _result = LocalFree(Some(HLOCAL(output.pbData.cast())));
        bytes
    };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_round_trip_is_scoped_to_the_current_windows_user() {
        let secret = [0x5a; 32];
        let protected = protect(&secret).expect("protect secret");
        assert_ne!(protected, hex::encode(secret));
        assert_eq!(unprotect(&protected).expect("unprotect secret"), secret);
    }
}
