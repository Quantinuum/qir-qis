use qir_qis::{get_entry_attributes, qir_ll_to_bc, qir_to_qis, validate_qir};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read input
    let ll_text = fs::read_to_string("tests/data/adaptive.ll")?;

    // Convert LLVM IR text to bitcode
    let bc_bytes = qir_ll_to_bc(&ll_text)?;

    // Validate QIR
    validate_qir(&bc_bytes, None)?;

    // Get entry point metadata
    let attributes = get_entry_attributes(&bc_bytes)?;
    println!("Required qubits: {attributes:#?}");

    // Windows LLVM builds are only validated against the host target in CI.
    #[cfg(windows)]
    let (opt_level, target) = (0, "native");
    #[cfg(not(windows))]
    let (opt_level, target) = (2, "aarch64");
    let qis_bytes = qir_to_qis(&bc_bytes, opt_level, target, None)?;

    // Write output
    fs::write("output.qis.bc", qis_bytes)?;

    println!("Successfully compiled to output.qis.bc");

    Ok(())
}
