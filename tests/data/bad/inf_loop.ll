; Compiles but doesn't return
%Result = type opaque
%Qubit = type opaque

@0 = internal constant [5 x i8] c"0_t0\00"

; entry point definition
define i64 @bad_loop() local_unnamed_addr #0 {
entry:
  ; call to the infinite loop function
  call void @infinite_loop()
  br label %exit

exit:
  call void @__quantum__rt__int_record_output(i64 1, i8* getelementptr inbounds ([5 x i8], [5 x i8]* @0, i32 0, i32 0))
  ret i64 0
}

define void @infinite_loop() {
entry:
  br label %loop

loop:
  br label %loop
}


declare void @__quantum__rt__int_record_output(i64, i8*)

; attributes

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

attributes #1 = { "irreversible" }

; module flags

!llvm.module.flags = !{!0, !1, !2, !3, !6, !7}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!6 = !{i32 1, !"ir_functions", i1 true}
!7 = !{i32 1, !"backwards_branching", i2 2}
