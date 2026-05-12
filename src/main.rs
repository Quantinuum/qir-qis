use std::fs;
use std::path::Path;
use std::process::exit;

use qir_qis::{
    DEFAULT_OPT_LEVEL, DEFAULT_TARGET, get_entry_attributes, qir_ll_to_bc, qir_to_qis, validate_qir,
};

use bpaf::Bpaf;

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options)]
struct Args {
    /// Optimization level (0, 1, 2, 3)
    #[bpaf(short('O'), long("opt-level"), fallback(DEFAULT_OPT_LEVEL))]
    opt_level: u32,

    #[allow(
        clippy::doc_markdown,
        reason = "bpaf option docs include quoted target spellings"
    )]
    /// Target architecture (e.g., "aarch64", "x86-64", "native")
    #[bpaf(short('t'), long("target"), fallback(String::from(DEFAULT_TARGET)))]
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
    let ll_text = match fs::read_to_string(ll_path) {
        Ok(ll_text) => ll_text,
        Err(err) => {
            eprintln!("Failed to read input file `{}`: {err}", ll_path.display());
            exit(1);
        }
    };

    let bc_bytes = match qir_ll_to_bc(&ll_text) {
        Ok(bc_bytes) => bc_bytes,
        Err(err) => {
            eprintln!("Failed to convert input LLVM IR to bitcode: {err}");
            exit(1);
        }
    };
    if let Err(err) = validate_qir(&bc_bytes, None) {
        eprintln!("QIR validation failed: {err:?}");
        exit(1);
    }

    println!("{:#?}", get_entry_attributes(&bc_bytes));

    let qis_module = match qir_to_qis(&bc_bytes, args.opt_level, &args.target, None) {
        Ok(qis_module) => qis_module,
        Err(err) => {
            eprintln!("QIR compilation failed: {err}");
            exit(1);
        }
    };
    let qis_path = ll_path.with_extension("qis.bc");
    if let Err(err) = fs::write(&qis_path, qis_module) {
        eprintln!(
            "Failed to write output file `{}`: {err}",
            qis_path.display()
        );
        exit(1);
    }
}
