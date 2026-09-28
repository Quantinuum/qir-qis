#![allow(
    clippy::expect_used,
    reason = "the fixed benchmark fixture must fail loudly when construction or validation changes"
)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use qir_qis::{qir_ll_to_bc, validate_qir};
use std::fmt::Write;

const GATE_COUNT: usize = 10_000;

fn static_gate_qir() -> String {
    let mut qir = String::from(
        r"declare void @__quantum__qis__h__body(ptr)

define i64 @Entry_Point_Name() #0 {
entry:
",
    );
    for index in 0..GATE_COUNT {
        let qubit = index % 8;
        writeln!(
            qir,
            "  call void @__quantum__qis__h__body(ptr inttoptr (i64 {qubit} to ptr))"
        )
        .expect("writing to a String cannot fail");
    }
    qir.push_str(
        r#"  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="8" "required_num_results"="0" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 1}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#,
    );
    qir
}

fn benchmark_static_gate_validation(criterion: &mut Criterion) {
    let bitcode = qir_ll_to_bc(&static_gate_qir()).expect("benchmark QIR should parse");

    criterion.bench_function("validate_qir/10000_static_h_gates", |bencher| {
        bencher.iter(|| {
            validate_qir(black_box(&bitcode), None).expect("benchmark QIR should validate");
        });
    });
}

criterion_group!(benches, benchmark_static_gate_validation);
criterion_main!(benches);
