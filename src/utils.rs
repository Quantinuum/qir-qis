#[cfg(feature = "wasm")]
use std::collections::BTreeMap;

use inkwell::{
    context::Context,
    module::Module,
    values::{BasicValueEnum, InstructionValue},
};
#[cfg(feature = "wasm")]
use wasmparser::{Export, ExternalKind, Payload};

/// Add metadata to the generator section
///
/// # Errors
/// Returns an error if the metadata could not be added.
pub fn add_generator_metadata<'c>(
    ctx: &'c Context,
    module: &Module<'c>,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let md_type = ctx
        .i8_type()
        .array_type(u32::try_from(value.len()).map_err(|_| "Value length too large")?);
    let md_value = ctx.const_string(value.as_bytes(), false);
    let gen_global = module.add_global(md_type, None, key);
    gen_global.set_initializer(&md_value);
    gen_global.set_section(Some(",generator"));
    Ok(())
}

/// Extracts operands from an instruction value.
///
/// # Errors
/// Returns an error if the operands could not be extracted.
pub fn extract_operands<'a>(
    instr: &'a InstructionValue<'a>,
) -> Result<Vec<BasicValueEnum<'a>>, String> {
    (0..instr.get_num_operands())
        .map(|i| {
            let op = instr
                .get_operand(i)
                .ok_or_else(|| format!("Failed to get operand at index {i}"))?;
            match op {
                inkwell::values::Operand::Value(bv) => Ok(bv),
                inkwell::values::Operand::Block(_) => {
                    Err(format!("Operand is not a value at index {i}"))
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()
}

/// Parses the WASM functions from the given bytes.
///
/// # Errors
/// Returns an error if the WASM functions could not be parsed.
#[cfg(feature = "wasm")]
pub fn parse_wasm_functions(wasm_bytes: &[u8]) -> Result<BTreeMap<String, u64>, String> {
    let mut wasm_fns: BTreeMap<String, u64> = BTreeMap::new();
    let parser = wasmparser::Parser::new(0);
    for payload in parser.parse_all(wasm_bytes) {
        let payload = payload.map_err(|e| format!("Failed to parse WASM: {e}"))?;
        if let Payload::ExportSection(exports) = payload {
            for export in exports {
                let export = export.map_err(|e| format!("Failed to parse WASM export: {e}"))?;
                if let Export {
                    name,
                    kind: ExternalKind::Func,
                    index,
                } = export
                {
                    wasm_fns.insert(name.to_string(), u64::from(index));
                }
            }
        }
    }
    Ok(wasm_fns)
}
#[cfg(all(test, feature = "wasm"))]
mod tests {
    #![allow(clippy::expect_used)]

    use super::parse_wasm_functions;
    use proptest::prelude::*;
    use wasm_encoder::{ExportKind, ExportSection, Module};

    fn build_wasm_exports(exports: &[(String, u32)]) -> Vec<u8> {
        let mut module = Module::new();
        let mut export_section = ExportSection::new();
        for (name, index) in exports {
            export_section.export(name, ExportKind::Func, *index);
        }
        module.section(&export_section);
        module.finish()
    }

    proptest! {
        #[test]
        fn prop_parse_wasm_functions_round_trips_exports(
            exports in proptest::collection::btree_map("[A-Za-z_][A-Za-z0-9_]{0,8}", 0u32..32u32, 0..8)
        ) {
            let exports_vec = exports
                .iter()
                .map(|(name, index)| (name.clone(), *index))
                .collect::<Vec<_>>();
            let wasm = build_wasm_exports(&exports_vec);
            let parsed = parse_wasm_functions(&wasm)
                .map_err(|err| TestCaseError::fail(format!("wasm parsing failed unexpectedly: {err}")))?;

            prop_assert_eq!(parsed.len(), exports.len());
            for (name, index) in exports {
                prop_assert_eq!(parsed.get(&name), Some(&u64::from(index)));
            }
        }

        #[test]
        fn prop_parse_wasm_functions_rejects_malformed_input(
            bytes in proptest::collection::vec(any::<u8>(), 0..256)
        ) {
            prop_assume!(bytes.len() < 4 || bytes[..4] != *b"\0asm");
            prop_assert!(parse_wasm_functions(&bytes).is_err());
        }
    }
}
