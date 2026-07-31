use pyo3_stub_gen::Result;
use std::path::Path;

const STUB_PATH: &str = "qir_qis.pyi";
const COPYRIGHT_HEADER: &str = "# Copyright (c) 2026 Quantinuum\n";
const LEGACY_COPYRIGHT_HEADER: &str = "# Copyright (c) Quantinuum\n";

fn main() -> Result<()> {
    // `stub_info` is a function defined by `define_stub_info_gatherer!` macro.
    let stub = qir_qis::stub_info()?;
    stub.generate()?;
    let stub_path = Path::new(STUB_PATH);
    let content = std::fs::read_to_string(stub_path)?;
    if content.starts_with(COPYRIGHT_HEADER) {
        return Ok(());
    }
    if let Some(stripped) = content.strip_prefix(LEGACY_COPYRIGHT_HEADER) {
        std::fs::write(stub_path, format!("{COPYRIGHT_HEADER}{stripped}"))?;
    } else {
        std::fs::write(stub_path, format!("{COPYRIGHT_HEADER}{content}"))?;
    }
    Ok(())
}
