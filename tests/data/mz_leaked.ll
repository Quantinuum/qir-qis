%Qubit = type opaque

@0 = private constant [7 x i8] c"leaked\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %0 = call i64 @__quantum__qis__mz_leaked__body(%Qubit* null)
  call void @__quantum__rt__int_record_output(i64 %0, i8* getelementptr inbounds ([7 x i8], [7 x i8]* @0, i64 0, i64 0))
  ret i64 0
}

declare i64 @__quantum__qis__mz_leaked__body(%Qubit*)
declare void @__quantum__rt__int_record_output(i64, i8*)

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="0" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
