import argparse  # noqa: D100
import sys
from pathlib import Path

from qir_formatter.labeled_formatter import QirLabeledFormatter
from rich import print as rprint
from selene_helios_qis_plugin import HeliosInterface, LogLevel
from selene_sim import Quest
from selene_sim.build import BitcodeString, build
from selene_sim.result_handling.parse_shot import postprocess_unparsed_stream

from qir_qis import (
    ValidationError,
    get_entry_attributes,
    qir_ll_to_bc,
    qir_to_qis,
    validate_qir,
)

DEFAULT_N_QUBITS = 6  # Default number of qubits for the quantum program


def main() -> None:
    """Run QIR to QIS conversion and emulate the quantum program."""
    parser = argparse.ArgumentParser(
        description="Run qir_qis and emulate using selene."
    )
    parser.add_argument("ll_file", type=Path, help="Path to the LLVM IR (.ll) file")
    parser.add_argument("-d", "--debug", action="store_true", help="Enable debug mode")
    parser.add_argument(
        "-s", "--spec", action="store_true", help="Enable spec compliant output"
    )
    args = parser.parse_args()
    ll_file = args.ll_file

    with ll_file.open("r") as f:
        llvm_ir = f.read()

    bc_bytes = qir_ll_to_bc(llvm_ir)
    try:
        validate_qir(bc_bytes)
    except ValidationError:  # noqa: TRY203
        # ensure we can catch ValidationError
        raise

    attrs = get_entry_attributes(bc_bytes)
    rprint(attrs)
    nqubits_str = attrs.get("required_num_qubits")
    nqubits = int(nqubits_str) if nqubits_str else DEFAULT_N_QUBITS

    bc_bytes = qir_to_qis(bc_bytes)

    interface = (
        HeliosInterface(
            log_level=LogLevel.DIAGNOSTIC,
        )
        if args.debug
        else None
    )

    runner = build(BitcodeString(bc_bytes), interface=interface)
    (results, error) = postprocess_unparsed_stream(
        [
            list(shot)
            for shot in runner.run_shots(
                simulator=Quest(random_seed=0),
                n_qubits=nqubits,
                n_shots=5,
                parse_results=False,
            )
        ]
    )

    if args.spec:
        results = QirLabeledFormatter().qir_labeled_output(results, attrs)
    rprint(results)
    if error:
        rprint(error)
        sys.exit(1)


if __name__ == "__main__":
    main()
