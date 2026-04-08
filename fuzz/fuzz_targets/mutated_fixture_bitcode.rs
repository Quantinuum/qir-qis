#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};
use std::sync::LazyLock;

const FIXTURES: &[&str] = &[
    include_str!("../../tests/data/base.ll"),
    include_str!("../../tests/data/adaptive.ll"),
    include_str!("../../tests/data/qir2_base.ll"),
];
const ASCII_MUTATIONS: &[u8] = b" \n01%@_abcdefghijklmnopqrstuvwxyz";
static FIXTURE_BYTES: LazyLock<Vec<Vec<u8>>> =
    LazyLock::new(|| FIXTURES.iter().map(|fixture| fixture.as_bytes().to_vec()).collect());

fn mutate_fixture_bitcode(seed: &[u8]) -> Option<Vec<u8>> {
    let fixture_idx = usize::from(*seed.first()?) % FIXTURES.len();
    let mut ll_bytes = FIXTURE_BYTES.get(fixture_idx)?.clone();

    for chunk in seed[1..].chunks(2).take(16) {
        if chunk.len() < 2 || ll_bytes.is_empty() {
            break;
        }
        let offset = usize::from(chunk[0]) % ll_bytes.len();
        let replacement = ASCII_MUTATIONS[usize::from(chunk[1]) % ASCII_MUTATIONS.len()];
        ll_bytes[offset] = replacement;
    }
    let ll_text = std::str::from_utf8(&ll_bytes).ok()?;
    qir_qis::qir_ll_to_bc(ll_text).ok()
}

fuzz_target!(|data: &[u8]| -> Corpus {
    let Some(mutated_bc) = mutate_fixture_bitcode(data) else {
        return Corpus::Reject;
    };

    if qir_qis::validate_qir(&mutated_bc, None).is_ok() {
        let _ = qir_qis::qir_to_qis(&mutated_bc, 0, "native", None);
    }
    Corpus::Keep
});
