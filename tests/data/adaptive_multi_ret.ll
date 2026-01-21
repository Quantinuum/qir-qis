%Result = type opaque
%Qubit = type opaque

@0 = internal constant [2 x i8] c"0\00"

define i64 @main() local_unnamed_addr #0 {
entry:
  tail call void @__quantum__qis__h__body(%Qubit* null)
  tail call void @__quantum__qis__mz__body(%Qubit* null, %Result* writeonly null)
  %0 = tail call i1 @__quantum__rt__read_result(%Result* readonly null)
  br i1 %0, label %error, label %exit
error:
  ; qubits should be in a zero state at the end of the program
  ret i64 1
exit:
  call void @__quantum__rt__result_record_output(%Result* null, i8* getelementptr inbounds ([2 x i8], [2 x i8]* @0, i32 0, i32 0))
  ret i64 0
}

declare void @__quantum__qis__h__body(%Qubit*)
declare void @__quantum__qis__mz__body(%Qubit*, %Result* writeonly)
declare i1 @__quantum__rt__read_result(%Result* readonly)
declare void @__quantum__rt__result_record_output(%Result*, i8*)

attributes #0 = { "entry_point" "required_num_qubits"="1" "required_num_results"="1" "qir_profiles"="adaptive" "output_labeling_schema"="labeled" }

!llvm.module.flags = !{!0, !1, !2, !3}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
