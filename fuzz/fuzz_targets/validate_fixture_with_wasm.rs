#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};
use std::sync::LazyLock;
use wasm_encoder::{ExportKind, ExportSection, Module};

const FIXTURE: &str = include_str!("../../tests/data/ArithOps_switch.ll");
const MAX_EXPORT_NAME_BYTES: usize = 256;
static FIXTURE_BITCODE: LazyLock<Vec<u8>> =
    LazyLock::new(|| qir_qis::qir_ll_to_bc(FIXTURE).expect("fixture should compile to bitcode"));

fn encode_wasm_with_exported_name(name: &str) -> Vec<u8> {
    let mut module = Module::new();
    let mut exports = ExportSection::new();
    exports.export(name, ExportKind::Func, 0);
    module.section(&exports);
    module.finish()
}

fuzz_target!(|data: &[u8]| -> Corpus {
    if data.len() > MAX_EXPORT_NAME_BYTES {
        return Corpus::Reject;
    }

    let Ok(export_name) = std::str::from_utf8(data) else {
        return Corpus::Reject;
    };

    let wasm = encode_wasm_with_exported_name(export_name);
    let _ = qir_qis::validate_qir(&FIXTURE_BITCODE, Some(&wasm));
    Corpus::Keep
});
