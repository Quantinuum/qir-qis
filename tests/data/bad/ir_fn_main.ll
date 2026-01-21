%Qubit = type opaque

; global constants (labels for output recording)

@0 = internal constant [5 x i8] c"int0\00"

; IR defined quantum function
define void @main(%Qubit* %arg1, %Qubit* %arg2) {
  call void @__quantum__qis__cnot__body(%Qubit* %arg1, %Qubit* %arg2)
  call void @__quantum__qis__cnot__body(%Qubit* %arg2, %Qubit* %arg1)
  call void @__quantum__qis__cnot__body(%Qubit* %arg1, %Qubit* %arg2)
  ret void
}

define i64 @entry() local_unnamed_addr #0 {
entry:
  ; calls to initialize the execution environment
  call void @__quantum__rt__initialize(i8* null)
  br label %body

body:                                       ; preds = %entry
  ; calling an IR defined quantum function
  call void @main(%Qubit* null, %Qubit* nonnull inttoptr (i64 1 to %Qubit*))

  br label %exit

exit:
  ret i64 0
}

; declarations of QIS functions

declare void @__quantum__qis__cnot__body(%Qubit*, %Qubit*) local_unnamed_addr

; declarations of runtime functions

declare void @__quantum__rt__initialize(i8*)

; attributes

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="6" "required_num_results"="6" }

attributes #1 = { "irreversible" }

; module flags

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5, !6, !7, !8, !9}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 5, !"int_computations", !10}
!5 = !{i32 5, !"float_computations", !11}
!6 = !{i32 1, !"ir_functions", i1 false}
!7 = !{i32 1, !"backwards_branching", i2 0}
!8 = !{i32 1, !"multiple_target_branching", i1 false}
!9 = !{i32 1, !"multiple_return_points", i1 false}
!10 = !{!"i32", !"i64"}
!11 = !{!"float", !"double"}
