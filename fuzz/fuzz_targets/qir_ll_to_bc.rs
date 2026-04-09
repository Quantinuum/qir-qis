#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(ll_text) = std::str::from_utf8(data) {
        let _ = qir_qis::qir_ll_to_bc(ll_text);
    }
});
