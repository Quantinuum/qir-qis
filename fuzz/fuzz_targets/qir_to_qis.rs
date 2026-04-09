#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = qir_qis::qir_to_qis(data, 0, "native", None);
});
