#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};
use std::sync::LazyLock;

const FIXTURES: &[&str] = &[
    include_str!("../../tests/data/base.ll"),
    include_str!("../../tests/data/adaptive.ll"),
    include_str!("../../tests/data/qir2_base.ll"),
];
static FIXTURE_BITCODE: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
    FIXTURES
        .iter()
        .map(|fixture| qir_qis::qir_ll_to_bc(fixture).expect("fixture should compile to bitcode"))
        .collect()
});

fn mutate_fixture_bitcode(seed: &[u8]) -> Option<Vec<u8>> {
    let fixture_idx = usize::from(*seed.first()?) % FIXTURES.len();
    let mut bc = FIXTURE_BITCODE.get(fixture_idx)?.clone();

    for chunk in seed[1..].chunks(2).take(16) {
        if chunk.len() < 2 || bc.is_empty() {
            break;
        }
        let offset = usize::from(chunk[0]) % bc.len();
        bc[offset] ^= chunk[1];
    }
    Some(bc)
}

fuzz_target!(|data: &[u8]| -> Corpus {
    let Some(mutated_bc) = mutate_fixture_bitcode(data) else {
        return Corpus::Reject;
    };

    let _ = qir_qis::validate_qir(&mutated_bc, None);
    let _ = qir_qis::qir_to_qis(&mutated_bc, 0, "native", None);
    Corpus::Keep
});
