use std::path::Path;
use std::process::exit;
use std::{borrow::Cow::Borrowed, fs};

use pyo3::Python;
use qir_qis::qir_qis::{get_entry_attributes, qir_ll_to_bc, qir_to_qis, validate_qir};

use bpaf::Bpaf;

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options)]
struct Args {
    /// Optimization level (0, 1, 2, 3)
    #[bpaf(short('O'), long("opt-level"), fallback(2u32))]
    opt_level: u32,

    #[allow(clippy::doc_markdown)]
    /// Target architecture (e.g., "aarch64", "x86-64", "native")
    #[bpaf(short('t'), long("target"), fallback(String::from("aarch64")))]
    target: String,

    /// Path to input LLVM IR file (.ll)
    #[bpaf(positional)]
    ll_path: String,
}

fn main() {
    // Initialize logging
    env_logger::init();

    let args = args().run();

    let ll_path = Path::new(&args.ll_path);
    let ll_text = fs::read_to_string(ll_path).expect("Failed to read input file");

    Python::initialize();
    let bc_bytes = qir_ll_to_bc(&ll_text).unwrap();
    if let Err(err) = validate_qir(Borrowed(&bc_bytes), None) {
        eprintln!("QIR validation failed: {err:?}");
        exit(1);
    }

    println!("{:#?}", get_entry_attributes(Borrowed(&bc_bytes)));

    let qis_module = qir_to_qis(bc_bytes, args.opt_level, &args.target, None).unwrap();
    let qis_path = ll_path.with_extension("qis.bc");
    fs::write(&qis_path, qis_module).expect("Failed to write output file");
}
