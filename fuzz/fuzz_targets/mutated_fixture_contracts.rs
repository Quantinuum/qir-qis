#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};
use std::{ops::Range, sync::LazyLock};

const FIXTURES: &[&str] = &[
    include_str!("../../tests/data/base.ll"),
    include_str!("../../tests/data/base_array.ll"),
    include_str!("../../tests/data/adaptive.ll"),
    include_str!("../../tests/data/qir2_base.ll"),
    include_str!("../../tests/data/qir2_adaptive.ll"),
    include_str!("../../tests/data/dynamic_qubit_alloc.ll"),
    include_str!("../../tests/data/dynamic_qubit_alloc_checked.ll"),
    include_str!("../../tests/data/dynamic_qubit_array_checked.ll"),
    include_str!("../../tests/data/dynamic_qubit_array_ssa.ll"),
    include_str!("../../tests/data/dynamic_result_alloc.ll"),
    include_str!("../../tests/data/dynamic_result_array.ll"),
    include_str!("../../tests/data/dynamic_result_mixed_array_output.ll"),
];
const PROFILE_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz_";
const SCHEMA_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz_";
const NUMERIC_ALPHABET: &[u8] = b"0123456789";
const BOOL_ALPHABET: &[u8] = b"falserut";

#[derive(Clone)]
struct SpanSpec {
    range: Range<usize>,
    alphabet: &'static [u8],
}

static FIXTURE_BYTES: LazyLock<Vec<Vec<u8>>> =
    LazyLock::new(|| FIXTURES.iter().map(|fixture| fixture.as_bytes().to_vec()).collect());
static FIXTURE_SPANS: LazyLock<Vec<Vec<SpanSpec>>> =
    LazyLock::new(|| FIXTURES.iter().map(|fixture| collect_spans(fixture)).collect());

fn find_attr_value_span(text: &str, attr_name: &str) -> Option<Range<usize>> {
    let needle = format!("\"{attr_name}\"=\"");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..].find('"')? + start;
    Some(start..end)
}

fn find_module_flag_value_span(text: &str, flag_name: &str) -> Option<Range<usize>> {
    let needle = format!("!\"{flag_name}\", i32 ");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..]
        .find(|ch: char| !ch.is_ascii_digit())
        .map_or(text.len(), |idx| start + idx);
    Some(start..end)
}

fn find_boolean_attr_value_span(text: &str, attr_name: &str) -> Option<Range<usize>> {
    let needle = format!("\"{attr_name}\"=\"");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..]
        .find(|ch: char| ch != 't' && ch != 'r' && ch != 'u' && ch != 'e' && ch != 'f' && ch != 'a' && ch != 'l' && ch != 's')
        .map_or(text.len(), |idx| start + idx);
    Some(start..end)
}

fn find_boolean_module_flag_value_span(text: &str, flag_name: &str) -> Option<Range<usize>> {
    let needle = format!("!\"{flag_name}\", i1 ");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..]
        .find(|ch: char| {
            ch != 't'
                && ch != 'r'
                && ch != 'u'
                && ch != 'e'
                && ch != 'f'
                && ch != 'a'
                && ch != 'l'
                && ch != 's'
        })
        .map_or(text.len(), |idx| start + idx);
    Some(start..end)
}

fn collect_spans(text: &str) -> Vec<SpanSpec> {
    let mut spans = Vec::new();

    for attr_name in ["qir_profiles", "output_labeling_schema"] {
        if let Some(range) = find_attr_value_span(text, attr_name) {
            let alphabet = if attr_name == "qir_profiles" {
                PROFILE_ALPHABET
            } else {
                SCHEMA_ALPHABET
            };
            spans.push(SpanSpec { range, alphabet });
        }
    }

    for attr_name in ["required_num_qubits", "required_num_results"] {
        if let Some(range) = find_attr_value_span(text, attr_name) {
            spans.push(SpanSpec {
                range,
                alphabet: NUMERIC_ALPHABET,
            });
        }
    }

    if let Some(range) = find_module_flag_value_span(text, "qir_major_version") {
        spans.push(SpanSpec {
            range,
            alphabet: NUMERIC_ALPHABET,
        });
    }

    for flag_name in [
        "dynamic_qubit_management",
        "dynamic_result_management",
        "arrays",
    ] {
        if let Some(range) = find_boolean_module_flag_value_span(text, flag_name)
            .or_else(|| find_boolean_attr_value_span(text, flag_name))
        {
            spans.push(SpanSpec {
                range,
                alphabet: BOOL_ALPHABET,
            });
        }
    }

    spans
}

fn mutate_fixture_contracts(seed: &[u8]) -> Option<Vec<u8>> {
    let fixture_idx = usize::from(*seed.first()?) % FIXTURES.len();
    let mut ll_bytes = FIXTURE_BYTES.get(fixture_idx)?.clone();
    let spans = FIXTURE_SPANS.get(fixture_idx)?;

    if spans.is_empty() {
        return None;
    }

    for chunk in seed[1..].chunks(3).take(8) {
        if chunk.len() < 3 {
            break;
        }
        let span = spans[usize::from(chunk[0]) % spans.len()].clone();
        if span.range.is_empty() {
            continue;
        }
        let offset = span.range.start + (usize::from(chunk[1]) % span.range.len());
        let replacement = span.alphabet[usize::from(chunk[2]) % span.alphabet.len()];
        ll_bytes[offset] = replacement;
    }

    let ll_text = std::str::from_utf8(&ll_bytes).ok()?;
    qir_qis::qir_ll_to_bc(ll_text).ok()
}

fuzz_target!(|data: &[u8]| -> Corpus {
    let Some(mutated_bc) = mutate_fixture_contracts(data) else {
        return Corpus::Reject;
    };

    if qir_qis::validate_qir(&mutated_bc, None).is_ok() {
        let _ = qir_qis::qir_to_qis(&mutated_bc, 0, "native", None);
    }
    Corpus::Keep
});
