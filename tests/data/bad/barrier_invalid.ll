; QIR test for invalid barrier (arity exceeds required_num_qubits)

%Result = type opaque
%Qubit = type opaque

define i64 @test_invalid_barrier() #0 {
entry:
  call void @__quantum__rt__initialize(i8* null)
  br label %body

body:
  ; Try to use barrier3 but module only has 2 qubits
  call void @__quantum__qis__barrier3__body(%Qubit* null, %Qubit* inttoptr (i64 1 to %Qubit*), %Qubit* inttoptr (i64 2 to %Qubit*))
  ret i64 0
}

declare void @__quantum__rt__initialize(i8*)
declare void @__quantum__qis__barrier3__body(%Qubit*, %Qubit*, %Qubit*)

attributes #0 = { "entry_point" "output_labeling_schema" "qir_profiles"="base_profile" "required_num_qubits"="2" "required_num_results"="0" }

!llvm.module.flags = !{!0, !1, !2, !3}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
